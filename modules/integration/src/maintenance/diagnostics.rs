use std::os::windows::process::CommandExt;

use super::{
    BTreeMap, Command, File, Installation, OpenOptions, OpenOptionsExt, OsStrExt, OsString, Path,
    PathBuf, Write, check_no_reparse, copy_file, fs, guid, hash, invalid, io, load_ownership,
    package, path_present, read_json, read_settings, registry, request, save_json, security,
};

/// # Errors
/// Fails if COM initialization, installation metadata, registry queries or package
/// deployment status cannot be read or validated.
pub fn integration_status() -> io::Result<serde_json::Value> {
    let _apartment = Apartment::new()?;
    let Some(installation) = Installation::current()? else {
        return Ok(serde_json::json!({"installed":false,"state":"Portable"}));
    };
    let settings = read_settings()?;
    let owned = load_ownership(&installation.root)?;
    let classic = owned
        .registry
        .iter()
        .map(|v| registry::read(&v.key, &v.name).map(|current| current == Some(v.value.clone())))
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .all(|ok| ok)
        && !owned.registry.is_empty();
    let modern_supported = owned.signed && package::modern_supported()?;
    let package = if modern_supported && settings.menus_enabled {
        Some(package::verify(
            &owned.publisher,
            &installation.active_version,
            &installation.active_dir(),
        )?)
    } else {
        None
    };
    let programs = ["unlinka.exe", "unlinka-desktop.exe", "unlinka-shell.dll"]
        .into_iter()
        .map(|name| {
            let path = format!("versions/{}/{name}", installation.active_version);
            let valid = owned.files.get(&path).is_some_and(|expected| {
                hash(&installation.root.join(&path)).is_ok_and(|actual| &actual == expected)
            });
            (name, valid)
        })
        .collect::<BTreeMap<_, _>>();
    Ok(
        serde_json::json!({"installed":true,"root_display":installation.root.to_string_lossy(),"root_utf16":installation.root.as_os_str().encode_wide().collect::<Vec<_>>(),"active_version":installation.active_version,"state":installation.state,"settings":settings,"programs":programs,"signed":owned.signed,"classic_menu":classic,"modern_menu_supported":modern_supported,"modern_menu":package,"package_name":crate::PACKAGE_NAME,"package_publisher":owned.publisher,"path_present":path_present(&installation.root)?,"path_owned":owned.path_owned.is_some(),"pending_cleanup":owned.pending,"transaction_pending":installation.root.join("state/transaction.json").exists()}),
    )
}

pub(super) fn logs_dir() -> io::Result<PathBuf> {
    let base = crate::user_data()?.join(crate::DATA_NAME);
    let _guard = security::private_dir(&base)?;
    let logs = base.join("logs");
    let _guard = security::private_dir(&logs)?;
    Ok(logs)
}

/// # Errors
/// Fails if the protected log directory cannot be opened, the bounded log cannot
/// be written, or existing owned logs cannot be inspected.
pub fn log_diagnostic(message: &str) -> io::Result<PathBuf> {
    let directory = logs_dir()?;
    let path = directory.join(format!("{}.log", guid()?));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(&path)?;
    let bounded: String = message.chars().take(16384).collect();
    writeln!(file, "{bounded}")?;
    file.sync_all()?;
    drop(file);
    let mut entries: Vec<_> = fs::read_dir(&directory)?
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_suffix(".log"))
                .is_some_and(request::valid_id)
        })
        .collect();
    entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
    let excess = entries.len().saturating_sub(8);
    for entry in entries.into_iter().take(excess) {
        check_no_reparse(&entry.path())?;
        let _ignored = fs::remove_file(entry.path());
    }
    Ok(path)
}

pub(super) fn purge_logs() -> io::Result<()> {
    let directory = logs_dir()?;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .and_then(|n| n.strip_suffix(".log"))
            .is_some_and(request::valid_id)
        {
            check_no_reparse(&entry.path())?;
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// Re-execute a verified copy outside the installation so stable binaries can be replaced.
/// # Errors
/// Fails if the runtime path, signature or copied hash is invalid, or the verified
/// child process cannot be started.
pub fn runtime_copy(args: &[OsString]) -> io::Result<Option<i32>> {
    let executable = std::env::current_exe()?;
    let (runtime_root, _base, _runtime) = prepare_runtime_root()?;
    if executable.starts_with(&runtime_root) {
        check_no_reparse(&executable)?;
        let parent = executable
            .parent()
            .ok_or_else(|| invalid("runtime helper has no parent"))?;
        if executable
            .file_name()
            .is_none_or(|name| name != "unlinka-maintenance.exe")
            || parent.parent() != Some(runtime_root.as_path())
            || !parent
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(request::valid_id)
        {
            return Err(invalid("invalid runtime helper path"));
        }
        let _directory = security::private_dir(parent)?;
        let expected: String = read_json(&parent.join("runtime.json"))?;
        if hash(&executable)? != expected {
            return Err(invalid("runtime helper hash does not match verified copy"));
        }
        verify_runtime_signature(&executable)?;
        return Ok(None);
    }
    let owned_hash = verify_installed_helper(&executable)?;
    verify_runtime_signature(&executable)?;
    let expected = match owned_hash {
        Some(expected) => expected,
        None => hash(&executable)?,
    };
    run_runtime(&executable, args, &runtime_root, &expected).map(Some)
}

/// Runs an owned maintenance copy without keeping the installed helper mapped.
///
/// # Errors
/// Fails for a missing installation, reparse path, unowned helper, invalid copied
/// hash or a runtime process that cannot be started.
pub fn run_installed(args: &[OsString]) -> io::Result<i32> {
    let installation =
        Installation::current()?.ok_or_else(|| invalid("installation is missing"))?;
    let executable = installation
        .root
        .join("maintenance/unlinka-maintenance.exe");
    let expected = verify_installed_helper(&executable)?
        .ok_or_else(|| invalid("maintenance helper is outside the installation"))?;
    let (runtime_root, _base, _runtime) = prepare_runtime_root()?;
    // The owned helper may belong to a pending upgrade with a different signing
    // policy than this desktop. The copied helper verifies its own build policy.
    run_runtime(&executable, args, &runtime_root, &expected)
}

fn prepare_runtime_root() -> io::Result<(PathBuf, File, File)> {
    let base = crate::user_data()?.join(crate::DATA_NAME);
    let base_guard = security::private_dir(&base)?;
    let runtime_root = base.join("maintenance");
    let runtime_guard = security::private_dir(&runtime_root)?;
    Ok((runtime_root, base_guard, runtime_guard))
}

fn run_runtime(
    executable: &Path,
    args: &[OsString],
    runtime_root: &Path,
    expected: &str,
) -> io::Result<i32> {
    let directory = runtime_root.join(guid()?);
    let private_guard = security::private_dir(&directory)?;
    let copied = directory.join("unlinka-maintenance.exe");
    copy_file(executable, &copied)?;
    if expected != hash(&copied)? {
        return Err(invalid("maintenance runtime copy verification failed"));
    }
    save_json(&directory.join("runtime.json"), &expected)?;

    let exit = Command::new(&copied)
        .args(args)
        .creation_flags(0x0800_0000)
        .status();
    // Parent owns the copy and waits until it is no longer running before removing it.
    if let Err(error) = fs::remove_file(&copied) {
        let _ignored = log_diagnostic(&format!(
            "Retained maintenance runtime {}: {error}",
            copied.display()
        ));
    }
    let marker = directory.join("runtime.json");
    if read_json::<String>(&marker).is_ok_and(|value| value == expected) {
        let _ignored = fs::remove_file(&marker);
    } else {
        let _ignored = log_diagnostic("Preserved changed maintenance runtime hash metadata.");
    }
    drop(private_guard);
    if let Err(error) = fs::remove_dir(&directory) {
        let _ignored = log_diagnostic(&format!(
            "Retained maintenance runtime directory {}: {error}",
            directory.display()
        ));
    }
    exit.map(|status| status.code().unwrap_or(2))
}

fn verify_runtime_signature(executable: &std::path::Path) -> io::Result<()> {
    if crate::SIGNED_BUILD {
        let publisher = security::signed_publisher(executable)?;
        if option_env!("UNLINKA_PACKAGE_PUBLISHER") != Some(publisher.as_str()) {
            return Err(invalid(
                "runtime helper signer differs from this build's Publisher",
            ));
        }
    }
    Ok(())
}

/// An installed unsigned helper must still be the exact owned stable binary.
fn verify_installed_helper(executable: &Path) -> io::Result<Option<String>> {
    let root = crate::install_root()?;
    if !executable.starts_with(&root) {
        return Ok(None);
    }
    if executable != root.join("maintenance/unlinka-maintenance.exe") {
        return Err(invalid(
            "maintenance helper is not the owned stable entry point",
        ));
    }
    check_no_reparse(executable)?;
    let actual = hash(executable)?;
    let ownership = load_ownership(&root)?;
    let transaction: Option<super::Transaction> =
        match read_json(&root.join("state/transaction.json")) {
            Ok(value) => Some(value),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
    if !installed_helper_hash_owned(&ownership, transaction.as_ref(), &actual)? {
        return Err(invalid(
            "installed maintenance helper failed ownership verification",
        ));
    }
    Ok(Some(actual))
}

/// A replaced helper can reach recovery before the new ownership record is committed.
pub(super) fn installed_helper_hash_owned(
    ownership: &super::Ownership,
    transaction: Option<&super::Transaction>,
    actual: &str,
) -> io::Result<bool> {
    super::validate_ownership(ownership)?;
    let helper = "maintenance/unlinka-maintenance.exe";
    if let Some(tx) = transaction {
        if tx.schema_version != 1 || !request::valid_id(&tx.id) {
            return Err(invalid("invalid maintenance helper recovery transaction"));
        }
        super::validate_ownership(&tx.before)?;
        super::validate_ownership(&tx.after)?;
        for change in &tx.files {
            super::validate_owned_path(&change.path)?;
            if let Some(backup) = &change.backup
                && !super::relative(backup)?
                    .starts_with(std::path::Path::new("state/backups").join(&tx.id))
            {
                return Err(invalid("helper recovery backup is outside its transaction"));
            }
        }
        for change in &tx.registry {
            super::validate_registry_key(&change.key)?;
        }
        for artifact in &tx.artifacts {
            super::artifacts::validate_artifact(tx, &super::relative(&artifact.path)?)?;
        }
        if tx
            .before
            .files
            .get(helper)
            .is_some_and(|expected| expected == actual)
        {
            return Ok(true);
        }
        if tx
            .after
            .files
            .get(helper)
            .is_some_and(|expected| expected == actual)
            && tx
                .files
                .iter()
                .any(|change| change.path == helper && change.after == actual)
        {
            return Ok(true);
        }
    }
    Ok(ownership
        .files
        .get(helper)
        .is_some_and(|expected| expected == actual))
}

struct Apartment(bool);

impl Apartment {
    fn new() -> io::Result<Self> {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        // SAFETY: initializes COM for this thread; a successful initialization is balanced by Drop.
        let status = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if status == windows::Win32::Foundation::RPC_E_CHANGED_MODE {
            return Ok(Self(false));
        }
        status.ok().map_err(crate::win_error)?;
        Ok(Self(true))
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: this thread owns exactly one successful COM initialization recorded by new.
            unsafe {
                windows::Win32::System::Com::CoUninitialize();
            }
        }
    }
}

// Previous crashed or concurrent runtimes cannot safely be deleted by this process.
pub(super) fn retained_runtime_files() -> io::Result<bool> {
    let current = std::env::current_exe()?;
    let directory = crate::user_data()?
        .join(crate::DATA_NAME)
        .join("maintenance");
    check_no_reparse(&directory)?;
    if !directory.exists() {
        return Ok(false);
    }
    let mut retained = false;
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        if Some(entry.path().as_path()) == current.parent() {
            continue;
        }
        retained = true;
        let _ignored = log_diagnostic(&format!(
            "Preserved previous or concurrent maintenance runtime files at {}",
            entry.path().display()
        ));
    }
    Ok(retained)
}
