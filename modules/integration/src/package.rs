use std::{
    fmt::Write,
    io,
    path::{Path, PathBuf},
};

use windows::{
    ApplicationModel::Package,
    Foundation::Uri,
    Management::Deployment::{AddPackageOptions, DeploymentResult, PackageManager},
    System::ProcessorArchitecture,
    core::HSTRING,
};

use crate::{PACKAGE_NAME, invalid, security::user_sid, win_error};

pub(crate) fn modern_supported() -> io::Result<bool> {
    use windows::{
        Wdk::System::SystemServices::RtlGetVersion,
        Win32::System::SystemInformation::{
            GetNativeSystemInfo, OSVERSIONINFOW, PROCESSOR_ARCHITECTURE_AMD64, SYSTEM_INFO,
        },
    };
    let mut info = SYSTEM_INFO::default();
    let mut version = OSVERSIONINFOW {
        dwOSVersionInfoSize: u32::try_from(std::mem::size_of::<OSVERSIONINFOW>())
            .map_err(|error| invalid(format!("version structure is oversized: {error}")))?,
        ..Default::default()
    };
    // SAFETY: the initialized native system information structure is writable.
    unsafe { GetNativeSystemInfo(&raw mut info) };
    // SAFETY: GetNativeSystemInfo populated the union's processor architecture member.
    let architecture = unsafe { info.Anonymous.Anonymous.wProcessorArchitecture };
    if architecture != PROCESSOR_ARCHITECTURE_AMD64 {
        return Err(invalid("native x64 Windows is required"));
    }
    // SAFETY: the version output is initialized and advertises its exact checked size.
    unsafe { RtlGetVersion(&raw mut version) }
        .ok()
        .map_err(win_error)?;
    if version.dwMajorVersion < 10 {
        return Err(invalid("Windows 10 or newer is required"));
    }
    Ok(version.dwBuildNumber >= 22000)
}

pub(crate) fn find(publisher: &str) -> io::Result<Vec<Package>> {
    let manager = PackageManager::new().map_err(win_error)?;
    let packages = manager
        .FindPackagesByUserSecurityId(&HSTRING::from(user_sid()?))
        .map_err(win_error)?;
    let mut result = Vec::new();
    for package in packages {
        let id = package.Id().map_err(win_error)?;
        // Package names are ASCII package strings and compare without case; Publisher does not.
        if !id
            .Name()
            .map_err(win_error)?
            .to_string()
            .eq_ignore_ascii_case(PACKAGE_NAME)
        {
            continue;
        }
        if id.Publisher().map_err(win_error)? != publisher
            || id.Architecture().map_err(win_error)? != ProcessorArchitecture::X64
        {
            return Err(invalid("package identity ownership conflict"));
        }
        result.push(package);
    }
    Ok(result)
}

fn external(package: &Package) -> io::Result<PathBuf> {
    Ok(PathBuf::from(
        package
            .EffectiveExternalPath()
            .map_err(win_error)?
            .to_os_string(),
    ))
}

pub(crate) fn verify(publisher: &str, version: &str, directory: &Path) -> io::Result<bool> {
    let packages = find(publisher)?;
    if packages.len() != 1 {
        return Ok(false);
    }
    let id = packages[0].Id().map_err(win_error)?;
    let v = id.Version().map_err(win_error)?;
    Ok(
        format!("{}.{}.{}.{}", v.Major, v.Minor, v.Build, v.Revision) == version
            && external(&packages[0])? == directory,
    )
}

fn deployment(result: &DeploymentResult) -> io::Result<()> {
    let error = result.ExtendedErrorCode().map_err(win_error)?;
    if error.is_err() {
        return Err(io::Error::other(format!(
            "package deployment HRESULT {:#010x}: {}",
            error.0,
            result.ErrorText().map_err(win_error)?
        )));
    }
    Ok(())
}

pub(crate) fn remove(publisher: &str, root: &Path) -> io::Result<()> {
    let manager = PackageManager::new().map_err(win_error)?;
    for package in find(publisher)? {
        let location = external(&package)?;
        if !crate::maintenance::owns_package_location(root, &location)? {
            return Err(invalid(
                "package external location is outside owned version directories",
            ));
        }
        let full_name = package
            .Id()
            .map_err(win_error)?
            .FullName()
            .map_err(win_error)?;
        deployment(
            &manager
                .RemovePackageAsync(&full_name)
                .map_err(win_error)?
                .join()
                .map_err(win_error)?,
        )?;
    }
    if !find(publisher)?.is_empty() {
        return Err(invalid("package removal has not completed"));
    }
    Ok(())
}

pub(crate) fn add(publisher: &str, version: &str, directory: &Path, root: &Path) -> io::Result<()> {
    if verify(publisher, version, directory)? {
        return Ok(());
    }
    remove(publisher, root)?;
    let options = AddPackageOptions::new().map_err(win_error)?;
    options.SetAllowUnsigned(false).map_err(win_error)?;
    options.SetForceAppShutdown(false).map_err(win_error)?;
    options
        .SetForceTargetAppShutdown(false)
        .map_err(win_error)?;
    options
        .SetForceUpdateFromAnyVersion(false)
        .map_err(win_error)?;
    options
        .SetDeferRegistrationWhenPackagesAreInUse(false)
        .map_err(win_error)?;
    options
        .SetExternalLocationUri(&file_uri(directory, true)?)
        .map_err(win_error)?;
    let manager = PackageManager::new().map_err(win_error)?;
    deployment(
        &manager
            .AddPackageByUriAsync(
                &file_uri(&directory.join("Unlinka.ShellIntegration.msix"), false)?,
                &options,
            )
            .map_err(win_error)?
            .join()
            .map_err(win_error)?,
    )?;
    if !verify(publisher, version, directory)? {
        return Err(invalid(
            "registered package identity, version or external location does not match",
        ));
    }
    Ok(())
}

fn file_uri(path: &Path, directory: bool) -> io::Result<Uri> {
    let text = path
        .to_str()
        .ok_or_else(|| invalid("package location is not valid Unicode"))?
        .replace('\\', "/");
    let mut uri = String::from("file:///");
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'_' | b'.' | b'~') {
            uri.push(char::from(byte));
        } else {
            write!(uri, "%{byte:02X}")
                .map_err(|error| io::Error::other(format!("URI encoding failed: {error}")))?;
        }
    }
    if directory && !uri.ends_with('/') {
        uri.push('/');
    }
    Uri::CreateUri(&HSTRING::from(uri)).map_err(win_error)
}
