//! Current-user maintenance with durable intent records and compensating rollback.
mod artifacts;
mod diagnostics;
mod files;
mod uninstall;
use artifacts::{cleanup_artifacts, file_identity, remove_internal_file};
use diagnostics::purge_logs;
pub use diagnostics::{integration_status, log_diagnostic, runtime_copy};
pub use uninstall::uninstall;
mod journal;
mod payload;
mod user_path;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    mem::size_of,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::OpenOptionsExt,
        io::AsRawHandle,
    },
    path::{Component, Path, PathBuf},
    process::Command,
};

use files::{copy_file, publish, remove_owned, replace_owned};
use journal::{begin, commit, recover, rollback};
pub use payload::validate_version;
use payload::{
    clsid_key, handler_key, hash, hash_reader, load_ownership, move_file, owned_payload, payload,
    read_json, relative, save_json, validate_owned_path, validate_ownership, validate_registry_key,
    validate_upgrade_policy,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use user_path::{compensation, edit_path, path_present, rollback_path};

use crate::{
    CLASSIC_CLSID, INSTALL_KEY, Installation, InstallationState, SETTINGS_KEY, Settings,
    check_file, check_no_reparse, guid, install_root, invalid, lock, notify, package, pcw,
    read_settings,
    registry::{self, Value},
    request, security, validate_language, wide,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub schema_version: u32,
    pub package_version: String,
    pub publisher: String,
    #[serde(default = "legacy_signed")]
    pub signed: bool,
    pub protocol_version: u32,
    pub files: Vec<PayloadFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Ownership {
    schema_version: u32,
    publisher: String,
    #[serde(default = "legacy_signed")]
    signed: bool,
    files: BTreeMap<String, String>,
    registry: Vec<OwnedValue>,
    path_owned: Option<Vec<u16>>,
    package_version: Option<String>,
    pending: Vec<String>,
}

fn legacy_signed() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedValue {
    key: String,
    name: String,
    value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryChange {
    key: String,
    name: String,
    before: Option<Value>,
    after: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileChange {
    path: String,
    before: Option<String>,
    after: String,
    backup: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PathChange {
    entry: Vec<u16>,
    added: bool,
    #[serde(default)]
    original_missing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    path: String,
    expected: Option<String>,
    #[serde(default)]
    identity: Option<FileIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileIdentity {
    volume: u32,
    high: u32,
    low: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    schema_version: u32,
    id: String,
    removing: bool,
    committed: bool,
    before_install: Option<Installation>,
    before: Ownership,
    after: Ownership,
    registry: Vec<RegistryChange>,
    files: Vec<FileChange>,
    path: Vec<PathChange>,
    package_touched: bool,
    #[serde(default)]
    artifacts: Vec<Artifact>,
}

fn classic(root: &Path, tx: &mut Transaction, version: &str, enabled: bool) -> io::Result<()> {
    let clsid = clsid_key();
    let handler = handler_key();
    let inproc = format!("{clsid}\\InprocServer32");
    let desired = [
        OwnedValue {
            key: clsid.clone(),
            name: String::new(),
            value: Value::string("Unlinka Classic Context Menu"),
        },
        OwnedValue {
            key: inproc.clone(),
            name: String::new(),
            value: Value::string(
                root.join("versions")
                    .join(version)
                    .join("unlinka-shell.dll"),
            ),
        },
        OwnedValue {
            key: inproc.clone(),
            name: "ThreadingModel".into(),
            value: Value::string("Apartment"),
        },
        OwnedValue {
            key: handler.clone(),
            name: String::new(),
            value: Value::string(CLASSIC_CLSID),
        },
    ];
    // Disable association before class mutation, so loaded Explorer cannot invoke a partial registration.
    if enabled {
        for value in &desired {
            let current = registry::read(&value.key, &value.name)?;
            let owned = tx
                .after
                .registry
                .iter()
                .find(|v| v.key == value.key && v.name == value.name);
            if current.is_some() && !owned.is_some_and(|v| current == Some(v.value.clone())) {
                return Err(invalid("classic registry ownership conflict"));
            }
            tx.write(root, &value.key, &value.name, Some(value.value.clone()))?;
        }
        tx.after.registry = desired.to_vec();
    } else {
        let mut old = tx.after.registry.clone();
        old.sort_by_key(|v| i32::from(v.key != handler));
        for value in old {
            if registry::read(&value.key, &value.name)? != Some(value.value.clone()) {
                return Err(invalid("owned classic registration was changed externally"));
            }
            tx.write(root, &value.key, &value.name, None)?;
        }
        tx.after.registry.clear();
        registry::remove_empty(&handler);
        registry::remove_empty(&inproc);
        registry::remove_empty(&clsid);
    }
    tx.save(root)
}

fn settings(root: &Path, tx: &mut Transaction, settings: &Settings) -> io::Result<()> {
    validate_language(&settings.language)?;
    tx.write(
        root,
        SETTINGS_KEY,
        "Language",
        Some(Value::string(&settings.language)),
    )?;
    tx.write(
        root,
        SETTINGS_KEY,
        "MenusEnabled",
        Some(Value::number(u32::from(settings.menus_enabled))),
    )?;
    tx.write(
        root,
        SETTINGS_KEY,
        "PathEnabled",
        Some(Value::number(u32::from(settings.path_enabled))),
    )
}

fn apply_integrations(
    root: &Path,
    tx: &mut Transaction,
    version: &str,
    options: &Settings,
) -> io::Result<()> {
    let modern = tx.after.signed && package::modern_supported()?;
    if !options.menus_enabled {
        tx.write(root, SETTINGS_KEY, "MenusEnabled", Some(Value::number(0)))?;
    }
    classic(root, tx, version, options.menus_enabled)?;
    if modern && (options.menus_enabled || tx.after.package_version.is_some()) {
        if options.menus_enabled {
            let directory = root.join("versions").join(version);
            if !package::verify(&tx.after.publisher, version, &directory)? {
                tx.package_touched = true;
                tx.save(root)?;
                package::add(&tx.after.publisher, version, &directory, root)?;
            }
            tx.after.package_version = Some(version.into());
        } else {
            tx.package_touched = true;
            tx.save(root)?;
            package::remove(&tx.after.publisher, root)?;
            tx.after.package_version = None;
        }
        tx.save(root)?;
    }
    edit_path(root, tx, options.path_enabled)
}

fn prepare_root() -> io::Result<(PathBuf, File, File)> {
    let root = install_root()?;
    check_no_reparse(&root)?;
    fs::create_dir_all(
        root.parent()
            .ok_or_else(|| invalid("install root has no parent"))?,
    )?;
    let guard = security::private_dir(&root)?;
    let lease = lock(&root, true)?;
    Ok((root, guard, lease))
}

/// Install, upgrade or same-version repair from a payload matching this helper's build policy.
/// # Errors
/// Fails if maintenance is busy, source hashes or signatures are invalid, the
/// source is a downgrade, disk space is insufficient, or publication or registration
/// fails. Incomplete changes are rolled back or recorded as requiring repair.
pub fn install(
    source: &Path,
    launcher: &Path,
    maintenance_binary: &Path,
    initial: Settings,
) -> io::Result<()> {
    check_no_reparse(source)?;
    let source = source.canonicalize()?;
    check_no_reparse(&source)?;
    let payload = self::payload(&source, true)?;
    if option_env!("UNLINKA_PACKAGE_VERSION")
        .is_some_and(|version| version != payload.package_version)
    {
        return Err(invalid(
            "installer build version does not match payload version",
        ));
    }
    if payload.signed
        && (security::signed_publisher(launcher)? != payload.publisher
            || security::signed_publisher(maintenance_binary)? != payload.publisher)
    {
        return Err(invalid("stable binary signer does not match Publisher"));
    }
    validate_language(&initial.language)?;
    package::modern_supported()?;
    check_disk_space(&source, &payload, launcher, maintenance_binary)?;
    let (root, _guard, _lease) = prepare_root()?;
    restore_pending_uninstall_helper(&root, maintenance_binary, &payload)?;
    recover(&root)?;
    let installation = Installation::current()?;
    let before = load_ownership(&root)?;
    if installation.is_some() && before.files.is_empty() {
        return Err(invalid("installed product ownership metadata is missing"));
    }
    for (source, name) in [
        (launcher, "bin/unlinka.exe"),
        (maintenance_binary, "maintenance/unlinka-maintenance.exe"),
    ] {
        if source.starts_with(&root)
            && (source != root.join(name) || before.files.get(name) != Some(&hash(source)?))
        {
            return Err(invalid(
                "installed stable repair source failed ownership verification",
            ));
        }
    }
    if let Some(current) = &installation {
        if validate_version(&payload.package_version)? < validate_version(&current.active_version)?
        {
            return Err(invalid("installing an older release is not supported"));
        }
        validate_upgrade_policy(&before, &payload, &current.active_version)?;
    }
    let options = initial;
    verify_existing_package(&root, &before, &payload)?;

    let previous_version = installation.as_ref().map(|i| i.active_version.clone());
    let mut tx = begin(&root, installation, before, false)?;
    let result = (|| {
        // Remove the old signed identity before adopting an unsigned policy. Recovery
        // retains the prior Publisher until this deployment intent is completed.
        if !payload.signed && tx.after.package_version.is_some() {
            tx.package_touched = true;
            tx.save(&root)?;
            package::remove(&tx.before.publisher, &root)?;
            tx.after.package_version = None;
        }
        tx.after.signed = payload.signed;
        tx.after.publisher.clone_from(&payload.publisher);
        tx.save(&root)?;
        tx.write(
            &root,
            INSTALL_KEY,
            "InstallRoot",
            Some(Value::string(&root)),
        )?;
        tx.state(
            &root,
            if tx.before_install.is_some() {
                InstallationState::Updating
            } else {
                InstallationState::Installing
            },
        )?;
        replace_owned(
            &root,
            &mut tx,
            maintenance_binary,
            "maintenance/unlinka-maintenance.exe",
        )?;
        publish(&root, &mut tx, &source, &payload)?;
        replace_owned(&root, &mut tx, launcher, "bin/unlinka.exe")?;
        apply_integrations(&root, &mut tx, &payload.package_version, &options)?;
        commit(&root, &mut tx, &payload.package_version, &options)
    })();
    if let Err(error) = result {
        if let Err(rollback_error) = rollback(&root, &tx) {
            return Err(io::Error::other(format!("{error}; {rollback_error}")));
        }
        return Err(error);
    }
    cleanup_old(&root, &payload.package_version, previous_version.as_deref())?;
    let _ignored = request::cleanup_stale();
    Ok(())
}

/// Check package ownership before either mode updates or removes its registration.
fn verify_existing_package(root: &Path, before: &Ownership, payload: &Payload) -> io::Result<()> {
    if !package::modern_supported()? || !(before.signed || payload.signed) {
        return Ok(());
    }
    let publisher = if before.signed {
        &before.publisher
    } else {
        &payload.publisher
    };
    if !package::find(publisher)?.is_empty() {
        let previous = before
            .package_version
            .as_deref()
            .ok_or_else(|| invalid("existing identity package has no ownership record"))?;
        if !package::verify(
            &before.publisher,
            previous,
            &root.join("versions").join(previous),
        )? {
            return Err(invalid(
                "existing identity package external location or version was changed externally",
            ));
        }
    }
    Ok(())
}

fn restore_pending_uninstall_helper(
    root: &Path,
    source: &Path,
    payload: &Payload,
) -> io::Result<()> {
    let mut tx: Transaction = match read_json(&root.join("state/transaction.json")) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !tx.removing {
        return Ok(());
    }
    if tx.schema_version != 1
        || !request::valid_id(&tx.id)
        || tx.after.publisher != payload.publisher
        || tx.after.signed != payload.signed
    {
        return Err(invalid(
            "pending uninstall ownership does not match trusted recovery source",
        ));
    }
    validate_ownership(&tx.before)?;
    validate_ownership(&tx.after)?;
    let target = root.join("maintenance/unlinka-maintenance.exe");
    if !target.exists() {
        copy_file(source, &target)?;
        tx.after
            .files
            .insert("maintenance/unlinka-maintenance.exe".into(), hash(&target)?);
        tx.save(root)?;
        save_json(&root.join("state/ownership.json"), &tx.after)?;
        registry::write(INSTALL_KEY, "InstallRoot", Some(&Value::string(root)))?;
        registry::write(INSTALL_KEY, "State", Some(&Value::string("RemovalPending")))?;
        registry::write(
            INSTALL_KEY,
            "ActiveVersion",
            Some(&Value::string(
                tx.before_install
                    .as_ref()
                    .map_or("", |i| i.active_version.as_str()),
            )),
        )?;
    }
    Err(io::Error::other(
        "A previous uninstall is pending. Its maintenance entry is available again; run Unlinka uninstall to finish cleanup before installing.",
    ))
}

pub(crate) fn owns_package_location(root: &Path, location: &Path) -> io::Result<bool> {
    check_no_reparse(location)?;
    let versions = root.join("versions");
    if location.parent() != Some(versions.as_path()) {
        return Ok(false);
    }
    let version = location
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| invalid("package location has no valid version"))?;
    validate_version(version)?;
    let prefix = format!("versions/{version}/");
    let ownership = load_ownership(root)?;
    if ownership.files.keys().any(|path| path.starts_with(&prefix)) {
        return Ok(true);
    }
    let tx: Transaction = match read_json(&root.join("state/transaction.json")) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if tx.schema_version != 1 || !request::valid_id(&tx.id) {
        return Err(invalid("invalid package ownership transaction"));
    }
    validate_ownership(&tx.before)?;
    validate_ownership(&tx.after)?;
    Ok(tx
        .before
        .files
        .keys()
        .chain(tx.after.files.keys())
        .any(|path| path.starts_with(&prefix)))
}

fn check_disk_space(
    source: &Path,
    payload: &Payload,
    launcher: &Path,
    maintenance_binary: &Path,
) -> io::Result<()> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut required = 32_u64 * 1024 * 1024;
    for path in payload
        .files
        .iter()
        .map(|entry| source.join(&entry.path))
        .chain([
            source.join("payload.json"),
            launcher.to_path_buf(),
            maintenance_binary.to_path_buf(),
        ])
    {
        required = required
            .checked_add(fs::metadata(path)?.len().saturating_mul(2))
            .ok_or_else(|| invalid("payload size overflow"))?;
    }
    let volume = wide(crate::user_data()?);
    let mut available = 0;
    // SAFETY: the terminated directory and output byte count live for the synchronous query.
    unsafe { GetDiskFreeSpaceExW(pcw(&volume), Some(&raw mut available), None, None) }
        .map_err(crate::win_error)?;
    if available < required {
        return Err(io::Error::other(format!(
            "insufficient free disk space: need {required} bytes, available {available}"
        )));
    }
    Ok(())
}

/// # Errors
/// Fails if maintenance is busy, installation is not Ready, settings are invalid,
/// or owned registration or PATH values conflict with external changes.
pub fn configure(options: &Settings) -> io::Result<()> {
    validate_language(&options.language)?;
    let (root, _guard, _lease) = prepare_root()?;
    recover(&root)?;
    let installation =
        Installation::current()?.ok_or_else(|| invalid("installation is missing"))?;
    if installation.state != InstallationState::Ready {
        return Err(invalid("repair installation before changing settings"));
    }
    let before = load_ownership(&root)?;
    owned_payload(&root, &installation.active_version, &before)?;
    let version = installation.active_version.clone();
    let mut tx = begin(&root, Some(installation), before, false)?;
    let result = (|| {
        tx.state(&root, InstallationState::Updating)?;
        apply_integrations(&root, &mut tx, &version, options)?;
        commit(&root, &mut tx, &version, options)
    })();
    if let Err(error) = result {
        rollback(&root, &tx)?;
        return Err(error);
    }
    Ok(())
}

/// # Errors
/// Fails if maintenance is busy, trusted repair files are unavailable, or file,
/// registry or package restoration cannot complete.
pub fn repair(source: Option<&Path>) -> io::Result<()> {
    if let Some(source) = source {
        let root = install_root()?;
        let installation =
            Installation::current()?.ok_or_else(|| invalid("installation is missing"))?;
        let payload = self::payload(source, true)?;
        if payload.package_version != installation.active_version {
            return Err(invalid("repair source must match active version"));
        }
        return install(
            source,
            &root.join("bin/unlinka.exe"),
            &root.join("maintenance/unlinka-maintenance.exe"),
            read_settings()?,
        );
    }
    let (root, _guard, _lease) = prepare_root()?;
    recover(&root)?;
    let installation =
        Installation::current()?.ok_or_else(|| invalid("installation is missing"))?;
    let before = load_ownership(&root)?;
    if let Err(error) = owned_payload(&root, &installation.active_version, &before) {
        registry::write(INSTALL_KEY, "State", Some(&Value::string("RepairRequired")))?;
        return Err(io::Error::other(format!(
            "{error}; repair requires the same-version installer matching the installed signing policy"
        )));
    }
    let options = read_settings()?;
    let version = installation.active_version.clone();
    let mut tx = begin(&root, Some(installation), before, false)?;
    let result = (|| {
        tx.state(&root, InstallationState::Updating)?;
        apply_integrations(&root, &mut tx, &version, &options)?;
        commit(&root, &mut tx, &version, &options)
    })();
    if let Err(error) = result {
        rollback(&root, &tx)?;
        return Err(error);
    }
    cleanup_old(&root, &version, None)?;
    let _ignored = request::cleanup_stale();
    Ok(())
}

fn cleanup_old(root: &Path, active: &str, previous: Option<&str>) -> io::Result<()> {
    let mut owned = load_ownership(root)?;
    let versions: BTreeSet<String> = owned
        .files
        .keys()
        .filter_map(|p| {
            let mut split = p.split('/');
            (split.next()? == "versions")
                .then(|| split.next().map(str::to_owned))
                .flatten()
        })
        .collect();
    let previous = previous.map(str::to_owned).or_else(|| {
        versions
            .iter()
            .filter(|v| v.as_str() != active)
            .filter_map(|v| validate_version(v).ok().map(|n| (n, v.clone())))
            .max()
            .map(|(_, v)| v)
    });
    let names: Vec<(String, String)> = owned
        .files
        .iter()
        .filter(|(name, _)| {
            name.starts_with("versions/")
                && !name.starts_with(&format!("versions/{active}/"))
                && !previous
                    .as_ref()
                    .is_some_and(|v| name.starts_with(&format!("versions/{v}/")))
        })
        .map(|(n, h)| (n.clone(), h.clone()))
        .collect();
    owned.pending.clear();
    for (name, expected) in names {
        match remove_owned(root, &name, &expected) {
            Ok(()) => {
                owned.files.remove(&name);
            },
            Err(error) => {
                let _ignored = log_diagnostic(&format!("{name}: {error}"));
                owned.pending.push(name);
            },
        }
        save_json(&root.join("state/ownership.json"), &owned)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UninstallOutcome {
    Complete,
    RemovalPending,
    CompleteWithRetainedFiles,
}

#[cfg(test)]
mod tests;
