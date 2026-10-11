//! Immutable Windows installers with optional signing and modern Shell identity.
use std::{
    env,
    fmt::Write as _,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod flags;

pub(crate) struct Options {
    pub publisher: Option<String>,
    pub certificate_thumbprint: Option<String>,
    pub timestamp_url: Option<String>,
    pub build_number: u16,
    pub iscc: Option<PathBuf>,
    pub sdk_bin: Option<PathBuf>,
}

impl Options {
    fn signed(&self) -> bool { self.certificate_thumbprint.is_some() }

    fn publisher(&self) -> &str { self.publisher.as_deref().unwrap_or_default() }
}

#[derive(Deserialize)]
struct Identity {
    app_id: String,
    #[serde(rename = "classic_clsid")]
    _classic_clsid: String,
    modern_clsid: String,
    #[serde(rename = "action_guid")]
    _action_guid: String,
    package_name: String,
    application_id: String,
    install_name: String,
}

#[derive(Serialize)]
struct Payload {
    schema_version: u32,
    signed: bool,
    package_version: String,
    publisher: String,
    protocol_version: u32,
    files: Vec<PayloadFile>,
}

#[derive(Serialize)]
struct PayloadFile {
    path: String,
    sha256: String,
}

/// Isolate packaging metadata from ordinary workspace builds. Both signing modes
/// share one product identity and the same immutable package output layout.
pub(crate) fn run(options: &Options) -> io::Result<()> {
    validate_options(options)?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let identity: Identity =
        serde_json::from_slice(&fs::read(root.join("packaging/identity.json"))?)?;
    let version = package_version(env!("CARGO_PKG_VERSION"), options.build_number)?;
    let output = root.join("target/packages").join(&version);
    if output.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "refusing to overwrite immutable package output {}; choose a new build number or explicitly remove the previous output",
                output.display()
            ),
        ));
    }
    let sdk = sdk_bin(options.sdk_bin.as_deref(), options.signed())?;
    let iscc = options
        .iscc
        .clone()
        .or_else(|| env::var_os("UNLINKA_ISCC").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("ISCC.exe"));
    if options.signed() {
        verify_certificate(options)?;
    }
    fs::create_dir_all(&output)?;
    let assets = output.join("assets");
    crate::assets::generate(&root.join("assets/Unlinka.svg"), &assets)?;
    let context = PackageBuild {
        root: &root,
        output: &output,
        assets: &assets,
        sdk: &sdk,
        options,
        identity: &identity,
        version: &version,
    };
    let binaries = build_binaries(&context)?;
    let payload = build_payload(&context, &binaries)?;
    let setup = build_installer(&context, &payload, &iscc)?;
    eprintln!("Package built: {}", setup.display());
    eprintln!(
        "{}",
        if options.signed() {
            "Signed installer: classic menus and Windows 11 modern menu are available."
        } else {
            "Unsigned installer: classic menus are available; no certificate or MSIX registration is required."
        }
    );
    Ok(())
}

struct PackageBuild<'a> {
    root: &'a Path,
    output: &'a Path,
    assets: &'a Path,
    sdk: &'a Path,
    options: &'a Options,
    identity: &'a Identity,
    version: &'a str,
}

fn build_binaries(context: &PackageBuild<'_>) -> io::Result<PathBuf> {
    let PackageBuild {
        root,
        output,
        assets,
        sdk,
        options,
        version,
        ..
    } = context;
    let app_resource = compile_resource(assets, output, sdk, version, false)?;
    let dll_resource = compile_resource(assets, output, sdk, version, true)?;
    let target = root
        .join("target/installer-build")
        .join(if options.signed() {
            "signed"
        } else {
            "unsigned"
        });
    let mut build = Command::new("cargo");
    flags::configure(&mut build, context, &target, &app_resource, &dll_resource)?;
    checked(&mut build)?;
    Ok(target.join("x86_64-pc-windows-msvc/release"))
}

fn build_payload(context: &PackageBuild<'_>, binaries: &Path) -> io::Result<PathBuf> {
    let PackageBuild {
        root,
        output,
        sdk,
        options,
        identity,
        version,
        ..
    } = context;
    let signtool = sdk.join("signtool.exe");
    let payload = output.join("payload");
    fs::create_dir(&payload)?;
    for (source, destination) in [
        ("unlinka.exe", "unlinka.exe"),
        ("unlinka-desktop.exe", "unlinka-desktop.exe"),
        ("unlinka_shell.dll", "unlinka-shell.dll"),
    ] {
        fs::copy(binaries.join(source), payload.join(destination))?;
    }
    for name in ["unlinka-launcher.exe", "unlinka-maintenance.exe"] {
        fs::copy(binaries.join(name), output.join(name))?;
    }
    copy_assets(context.assets, &payload.join("Assets"))?;
    let signed_files = [
        payload.join("unlinka.exe"),
        payload.join("unlinka-desktop.exe"),
        payload.join("unlinka-shell.dll"),
        output.join("unlinka-launcher.exe"),
        output.join("unlinka-maintenance.exe"),
    ];
    for path in &signed_files {
        sign(&signtool, options, path)?;
    }
    if options.signed() {
        let sparse = output.join("sparse");
        fs::create_dir(&sparse)?;
        let template = fs::read_to_string(root.join("packaging/AppxManifest.xml.in"))?;
        let manifest = render_manifest(&template, identity, version, options.publisher());
        fs::write(sparse.join("AppxManifest.xml"), manifest)?;
        let package = payload.join("Unlinka.ShellIntegration.msix");
        checked(
            Command::new(sdk.join("makeappx.exe"))
                .args(["pack", "/nv", "/o", "/d"])
                .arg(&sparse)
                .arg("/p")
                .arg(&package),
        )?;
        sign(&signtool, options, &package)?;
    }
    let files = inventory(&payload)?;
    fs::write(
        payload.join("payload.json"),
        serde_json::to_vec_pretty(&Payload {
            schema_version: 1,
            signed: options.signed(),
            package_version: (*version).to_owned(),
            publisher: options.publisher().to_owned(),
            protocol_version: 1,
            files,
        })?,
    )?;
    Ok(payload)
}

fn build_installer(context: &PackageBuild<'_>, payload: &Path, iscc: &Path) -> io::Result<PathBuf> {
    let PackageBuild {
        root,
        output,
        options,
        identity,
        version,
        ..
    } = context;
    let script_dir = output.join("installer");
    fs::create_dir(&script_dir)?;
    fs::copy(
        root.join("packaging/unlinka.iss"),
        script_dir.join("unlinka.iss"),
    )?;
    copy_assets(context.assets, &script_dir.join("Assets"))?;
    generate_installer_messages(
        root,
        &script_dir.join("installer-messages.iss"),
        options.signed(),
    )?;
    let settings_key = "Software\\Unlinka\\Settings";
    let installation_key = "Software\\Unlinka\\Installation";
    let mut compile = Command::new(iscc);
    configure_installer_signing(&mut compile, context, &script_dir)?;
    for (key, value) in [
        ("PayloadDir", payload.display().to_string()),
        (
            "MaintenanceExe",
            output.join("unlinka-maintenance.exe").display().to_string(),
        ),
        (
            "LauncherExe",
            output.join("unlinka-launcher.exe").display().to_string(),
        ),
        ("AppVersion", (*version).to_owned()),
        ("AppId", identity.app_id.clone()),
        ("InstallName", identity.install_name.clone()),
        ("SettingsKey", settings_key.to_owned()),
        ("InstallationKey", installation_key.to_owned()),
        (
            "SignedBuild",
            if options.signed() { "1" } else { "0" }.to_owned(),
        ),
        ("OutputDir", output.display().to_string()),
    ] {
        compile.arg(format!("/D{key}={value}"));
    }
    compile.arg(script_dir.join("unlinka.iss"));
    checked(&mut compile)?;
    fs::read_dir(output)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("Unlinka-Setup-"))
        })
        .ok_or_else(|| io::Error::other("Inno did not produce Setup"))
}

fn configure_installer_signing(
    compile: &mut Command,
    context: &PackageBuild<'_>,
    script_dir: &Path,
) -> io::Result<()> {
    compile.arg("--no-ide-signtools");
    if !context.options.signed() {
        return Ok(());
    }
    let script = script_dir.join("sign-artifact.ps1");
    fs::copy(context.root.join("packaging/sign-artifact.ps1"), &script)?;
    // Inno's $q/$f substitutions quote paths; $$ preserves literal dollars in
    // the source checkout path. No shell concatenation or private-key argument.
    let script = script.display().to_string().replace('$', "$$");
    compile
        .arg(format!("--signtool=unlinka_signed_artifact=pwsh -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $q{script}$q $f"))
        .env("UNLINKA_SIGN_TOOL", context.sdk.join("signtool.exe"))
        .env("UNLINKA_SIGN_THUMBPRINT", context.options.certificate_thumbprint.as_deref().unwrap_or_default())
        .env("UNLINKA_SIGN_TIMESTAMP", context.options.timestamp_url.as_deref().unwrap_or_default());
    Ok(())
}

fn validate_options(options: &Options) -> io::Result<()> {
    let supplied = [
        options.publisher.is_some(),
        options.certificate_thumbprint.is_some(),
        options.timestamp_url.is_some(),
    ];
    if supplied.iter().any(|present| *present) && !supplied.iter().all(|present| *present) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "supply --publisher, --certificate-thumbprint and --timestamp-url together, or omit all three for an unsigned installer",
        ));
    }
    if !options.signed() {
        return Ok(());
    }
    if options.publisher().trim().is_empty() || options.publisher().contains(['\n', '\r', '\0']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a certificate Publisher subject is required",
        ));
    }
    let timestamp = options.timestamp_url.as_deref().unwrap_or_default();
    if !(timestamp.starts_with("https://") || timestamp.starts_with("http://"))
        || timestamp.chars().any(|ch| ch.is_whitespace() || ch == '\0')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "timestamp URL must be an HTTP or HTTPS RFC3161 endpoint",
        ));
    }
    if let Some(thumbprint) = &options.certificate_thumbprint {
        if thumbprint.len() != 40
            || !thumbprint
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "certificate thumbprint must be 40 hexadecimal characters",
            ));
        }
    }
    Ok(())
}

fn package_version(version: &str, revision: u16) -> io::Result<String> {
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() != 3 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "package version requires a stable major.minor.patch Cargo version",
        ));
    }
    let parts = parts
        .iter()
        .map(|part| part.parse::<u16>().map_err(io::Error::other))
        .collect::<io::Result<Vec<_>>>()?;
    Ok(format!(
        "{}.{}.{}.{}",
        parts[0], parts[1], parts[2], revision
    ))
}

fn checked(command: &mut Command) -> io::Result<()> {
    let status = command.stdin(Stdio::null()).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "build tool exited {status}: {:?}",
            command.get_program()
        )))
    }
}

fn sdk_bin(supplied: Option<&Path>, signed: bool) -> io::Result<PathBuf> {
    let available = |directory: &Path| {
        directory.join("rc.exe").is_file()
            && (!signed
                || ["makeappx.exe", "signtool.exe"]
                    .iter()
                    .all(|name| directory.join(name).is_file()))
    };
    if let Some(path) = supplied {
        let path = path.canonicalize()?;
        if available(&path) {
            return Ok(path);
        }
        return Err(io::Error::other(
            "supplied SDK directory is missing required build tools",
        ));
    }
    let base = env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("ProgramFiles(x86) unavailable; pass --sdk-bin"))?
        .join("Windows Kits/10/bin");
    let mut versions = fs::read_dir(base)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| available(&path.join("x64")))
        .collect::<Vec<_>>();
    versions.sort_by_key(|path| {
        path.file_name().map(|name| {
            name.to_string_lossy()
                .split('.')
                .map(|part| part.parse::<u32>().unwrap_or(0))
                .collect::<Vec<_>>()
        })
    });
    versions
        .pop()
        .map(|path| path.join("x64"))
        .ok_or_else(|| io::Error::other("Windows SDK build tools unavailable; pass --sdk-bin"))
}

fn verify_certificate(options: &Options) -> io::Result<()> {
    let output = Command::new("pwsh").args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command",
        "$ErrorActionPreference='Stop'; $cert=Get-Item -LiteralPath ('Cert:\\CurrentUser\\My\\'+$env:UNLINKA_SIGN_THUMBPRINT); if(-not $cert.HasPrivateKey){throw 'No signing private key'}; [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $cert.Subject | ConvertTo-Json -Compress"])
        .env("UNLINKA_SIGN_THUMBPRINT", options.certificate_thumbprint.as_deref().unwrap_or_default()).output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            "signing certificate unavailable in CurrentUser/My",
        ));
    }
    let subject: String = serde_json::from_slice(&output.stdout)?;
    if subject != options.publisher() {
        return Err(io::Error::other(
            "Publisher must exactly match the signing certificate Subject",
        ));
    }
    Ok(())
}

fn sign(tool: &Path, options: &Options, path: &Path) -> io::Result<()> {
    if !options.signed() {
        return Ok(());
    }
    checked(
        Command::new(tool)
            .args([
                "sign",
                "/fd",
                "SHA256",
                "/sha1",
                options
                    .certificate_thumbprint
                    .as_deref()
                    .unwrap_or_default(),
                "/s",
                "My",
                "/tr",
                options.timestamp_url.as_deref().unwrap_or_default(),
                "/td",
                "SHA256",
            ])
            .arg(path),
    )?;
    checked(Command::new(tool).args(["verify", "/pa", "/all"]).arg(path))
}

fn compile_resource(
    assets: &Path,
    output: &Path,
    sdk: &Path,
    version: &str,
    dll: bool,
) -> io::Result<PathBuf> {
    let name = if dll { "dll" } else { "application" };
    let resource = output.join(format!("{name}.rc"));
    let result = output.join(format!("{name}.res"));
    let numeric = version.replace('.', ",");
    let icon = assets
        .join("Unlinka.ico")
        .display()
        .to_string()
        .replace('\\', "\\\\");
    fs::write(
        &resource,
        format!(
            r#"#include <winver.h>
1 ICON "{icon}"
1 VERSIONINFO
 FILEVERSION {numeric}
 PRODUCTVERSION {numeric}
 FILEFLAGSMASK 0x3fL
 FILEFLAGS 0
 FILEOS 0x40004L
 FILETYPE {}
BEGIN
 BLOCK "StringFileInfo"
 BEGIN
  BLOCK "040904b0"
  BEGIN
   VALUE "FileDescription", "Unlinka\0"
   VALUE "FileVersion", "{version}\0"
   VALUE "ProductName", "Unlinka\0"
   VALUE "ProductVersion", "{version}\0"
  END
 END
 BLOCK "VarFileInfo"
 BEGIN
  VALUE "Translation", 0x409, 1200
 END
END
"#,
            if dll { 2 } else { 1 }
        ),
    )?;
    let sdk_version = sdk
        .parent()
        .and_then(Path::file_name)
        .ok_or_else(|| io::Error::other("invalid SDK bin path"))?;
    let sdk_root = sdk
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| io::Error::other("invalid SDK bin path"))?;
    checked(
        Command::new(sdk.join("rc.exe"))
            .arg("/nologo")
            .arg("/c65001")
            .arg("/I")
            .arg(sdk_root.join("Include").join(sdk_version).join("um"))
            .arg("/I")
            .arg(sdk_root.join("Include").join(sdk_version).join("shared"))
            .arg("/fo")
            .arg(&result)
            .arg(&resource),
    )?;
    Ok(result)
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
}

fn render_manifest(template: &str, identity: &Identity, version: &str, publisher: &str) -> String {
    let mut rendered = template.to_owned();
    for (key, value) in [
        ("PACKAGE_NAME", identity.package_name.as_str()),
        ("PUBLISHER", publisher),
        ("VERSION", version),
        ("INSTALL_NAME", &identity.install_name),
        ("APPLICATION_ID", &identity.application_id),
        ("MODERN_CLSID", &identity.modern_clsid),
    ] {
        rendered = rendered.replace(&format!("@{key}@"), &xml(value));
    }
    rendered
}

fn copy_assets(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::copy(entry.path(), destination.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn inventory(root: &Path) -> io::Result<Vec<PayloadFile>> {
    fn visit(root: &Path, directory: &Path, files: &mut Vec<PayloadFile>) -> io::Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                visit(root, &entry.path(), files)?;
            } else if entry.file_type()?.is_file() {
                let path = entry.path();
                let relative = path
                    .strip_prefix(root)
                    .map_err(io::Error::other)?
                    .to_string_lossy()
                    .replace('\\', "/");
                files.push(PayloadFile {
                    path: relative,
                    sha256: Sha256::digest(fs::read(path)?).iter().fold(
                        String::with_capacity(64),
                        |mut text, byte| {
                            write!(text, "{byte:02x}")
                                .expect("writing a hash to a String cannot fail");
                            text
                        },
                    ),
                });
            } else {
                return Err(io::Error::other("payload may contain only regular files"));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    visit(root, root, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn generate_installer_messages(root: &Path, output: &Path, signed: bool) -> io::Result<()> {
    let mut messages = String::from("[CustomMessages]\n");
    for (language, locale) in [("en", "en-US"), ("zh", "zh-CN")] {
        let source = fs::read_to_string(root.join(format!("modules/i18n/locales/{locale}.ftl")))?;
        for (key, id) in [
            ("Menus", if signed { "menus" } else { "menus-classic" }),
            ("UserPath", "user-path"),
            ("LanguageTitle", "language-title"),
            ("LanguagePrompt", "language-prompt"),
            ("LanguageAuto", "language-auto"),
            ("MaintenanceFailed", "maintenance-failed"),
            ("RepairFirst", "repair-first"),
            ("RemovalPending", "removal-pending"),
        ] {
            let prefix = format!("installer-{id} = ");
            let value = source
                .lines()
                .find_map(|line| line.strip_prefix(&prefix))
                .ok_or_else(|| {
                    io::Error::other(format!(
                        "missing one-line installer translation {prefix} in {locale}"
                    ))
                })?;
            writeln!(messages, "{language}.{key}={value}")
                .expect("writing messages to String cannot fail");
        }
    }
    fs::write(output, messages)
}

#[cfg(test)]
mod tests;
