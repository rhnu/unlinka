use super::{
    File, INSTALL_KEY, Installation, InstallationState, Path, SETTINGS_KEY, Transaction,
    UninstallOutcome, Value, begin, check_no_reparse, classic, cleanup_artifacts, copy_file,
    edit_path, fs, hash, invalid, io, load_ownership, log_diagnostic, notify, prepare_root,
    purge_logs, read_json, recover, registry, remove_internal_file, remove_owned, request,
    save_json, validate_ownership,
};

/// Leave the Inno uninstaller and ARP registration untouched until this returns Complete.
/// # Errors
/// Fails if maintenance is busy, ownership metadata is invalid, or registry or PATH
/// removal fails. Busy owned files return `RemovalPending`.
pub fn uninstall(purge_settings: bool) -> io::Result<UninstallOutcome> {
    let (root, _guard, lease) = prepare_root()?;
    let mut tx = resume_uninstall(&root)?;
    validate_uninstall_transaction(&tx)?;
    if tx.before.files.is_empty() && tx.before_install.is_some() {
        return Err(invalid("cannot uninstall without ownership metadata"));
    }
    record_purge_settings(&mut tx, purge_settings);
    // Purging can partly succeed before final cleanup needs a retry. Preserve
    // that choice before any removal, since the deleted preferences cannot be restored.
    tx.save(&root)?;
    tx.state(&root, InstallationState::Removing)?;
    tx.write(&root, SETTINGS_KEY, "MenusEnabled", Some(Value::number(0)))?;
    classic(&root, &mut tx, "", false)?;
    edit_path(&root, &mut tx, false)?;
    notify();
    tx.after.pending.clear();
    let names: Vec<_> = tx
        .after
        .files
        .iter()
        .map(|(n, h)| (n.clone(), h.clone()))
        .filter(|(n, _)| !n.starts_with("maintenance/"))
        .collect();
    for (name, expected) in names {
        match remove_owned(&root, &name, &expected) {
            Ok(()) => {
                tx.after.files.remove(&name);
            },
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                let _ignored = log_diagnostic(&format!(
                    "Preserved changed or unowned file {name}: {error}"
                ));
                tx.after.files.remove(&name);
            },
            Err(error) => {
                let _ignored = log_diagnostic(&format!("{name}: {error}"));
                tx.after.pending.push(name);
            },
        }
        tx.save(&root)?;
        save_json(&root.join("state/ownership.json"), &tx.after)?;
    }
    if !tx.after.pending.is_empty() {
        tx.state(&root, InstallationState::RemovalPending)?;
        tx.save(&root)?;
        return Ok(UninstallOutcome::RemovalPending);
    }
    if let Err(error) = cleanup_artifacts(&root, &tx) {
        let _ignored = log_diagnostic(&error.to_string());
        tx.state(&root, InstallationState::RemovalPending)?;
        tx.save(&root)?;
        return Ok(UninstallOutcome::RemovalPending);
    }
    let helper_hash = tx
        .after
        .files
        .get("maintenance/unlinka-maintenance.exe")
        .cloned();
    for (name, expected) in tx.after.files.clone() {
        if let Err(error) = remove_owned(&root, &name, &expected) {
            if error.kind() == io::ErrorKind::InvalidData {
                let _ignored = log_diagnostic(&format!(
                    "Preserved changed maintenance file {name}: {error}"
                ));
                tx.after.files.remove(&name);
                tx.save(&root)?;
                continue;
            }
            let _ignored = log_diagnostic(&format!("{name}: {error}"));
            tx.after.pending.push(name);
            save_json(&root.join("state/ownership.json"), &tx.after)?;
            tx.state(&root, InstallationState::RemovalPending)?;
            tx.save(&root)?;
            return Ok(UninstallOutcome::RemovalPending);
        }
        tx.after.files.remove(&name);
        tx.save(&root)?;
    }
    finalize_removal(&root, &mut tx, helper_hash, lease)
}

/// Read the durable purge choice without starting or recovering maintenance.
/// # Errors
/// Fails if the fixed installation path or pending transaction metadata is invalid.
pub fn uninstall_purge_pending() -> io::Result<bool> {
    let root = super::install_root()?;
    let tx: Transaction = match read_json(&root.join("state/transaction.json")) {
        Ok(tx) => tx,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    pending_purge_settings(&tx)
}

fn validate_uninstall_transaction(tx: &Transaction) -> io::Result<()> {
    if tx.schema_version != 1 || !request::valid_id(&tx.id) {
        return Err(invalid("invalid uninstall transaction"));
    }
    validate_ownership(&tx.before)?;
    validate_ownership(&tx.after)
}

pub(super) fn pending_purge_settings(tx: &Transaction) -> io::Result<bool> {
    validate_uninstall_transaction(tx)?;
    Ok(tx.removing && tx.preferences == super::PreferencesDisposition::Purge)
}

/// An irreversible purge choice survives later retries requesting preservation.
pub(super) fn record_purge_settings(tx: &mut Transaction, selected: bool) {
    if selected {
        tx.preferences = super::PreferencesDisposition::Purge;
    }
}

fn finalize_removal(
    root: &Path,
    tx: &mut Transaction,
    helper_hash: Option<String>,
    lease: File,
) -> io::Result<UninstallOutcome> {
    let mut retained_unknown = cleanup_user_requests();
    let mut lease = Some(lease);
    let cleanup = (|| {
        // The helper has already been removed. Every remaining fallible step must
        // share the recovery path that restores its retry entry.
        finalize_preferences(tx)?;
        for name in [
            "InstallRoot",
            "ActiveVersion",
            "State",
            "PackagePublisher",
            "PackageName",
            "Architecture",
            "PathOwned",
        ] {
            registry::write(INSTALL_KEY, name, None)?;
        }
        registry::remove_empty(INSTALL_KEY);
        // Release the lease only before deleting its file; earlier failures
        // retain maintenance exclusion while the retry entry is restored.
        drop(lease.take());
        remove_internal_file(root, Path::new("state/ownership.json"), None, None)?;
        remove_internal_file(root, Path::new("state/transaction.json"), None, None)?;
        remove_internal_file(
            root,
            Path::new("state/activity.lock"),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
            None,
        )?;
        for dir in [
            "state/backups",
            "state",
            "staging",
            "versions",
            "maintenance",
            "bin",
        ] {
            let dir = root.join(dir);
            if let Err(error) = check_no_reparse(&dir) {
                retained_unknown = true;
                let _ignored = log_diagnostic(&format!(
                    "Preserved unowned directory {}: {error}",
                    dir.display()
                ));
                continue;
            }
            if dir.exists()
                && let Err(error) = fs::remove_dir(&dir)
            {
                if error.raw_os_error() == Some(145) {
                    retained_unknown = true;
                    let _ignored =
                        log_diagnostic(&format!("Preserved unowned files in {}", dir.display()));
                } else {
                    return Err(error);
                }
            }
        }
        Ok::<_, io::Error>(())
    })();
    if let Err(error) = cleanup {
        // Preserve a truthful retry entry even if metadata cleanup fails after payload removal.
        fs::create_dir_all(root.join("state"))?;
        if let Some(expected) = helper_hash {
            let executable = std::env::current_exe()?;
            if hash(&executable)? != expected {
                return Err(invalid(
                    "cannot restore retained maintenance helper from this runtime version",
                ));
            }
            let target = root.join("maintenance/unlinka-maintenance.exe");
            if !target.exists() {
                copy_file(&executable, &target)?;
            }
            tx.after
                .files
                .insert("maintenance/unlinka-maintenance.exe".into(), expected);
        }
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
        tx.save(root)?;
        save_json(&root.join("state/ownership.json"), &tx.after)?;
        let _ignored = log_diagnostic(&error.to_string());
        return Ok(UninstallOutcome::RemovalPending);
    }
    Ok(if retained_unknown {
        UninstallOutcome::CompleteWithRetainedFiles
    } else {
        UninstallOutcome::Complete
    })
}

fn resume_uninstall(root: &Path) -> io::Result<Transaction> {
    let tx: Transaction = match read_json(&root.join("state/transaction.json")) {
        Ok(tx) => {
            let tx: Transaction = tx;
            if tx.removing {
                tx
            } else {
                recover(root)?;
                begin(root, Installation::current()?, load_ownership(root)?, true)?
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            begin(root, Installation::current()?, load_ownership(root)?, true)?
        },
        Err(error) => return Err(error),
    };
    Ok(tx)
}

fn cleanup_user_requests() -> bool {
    let mut retained_unknown = match request::cleanup_owned() {
        Ok(retained) => retained,
        Err(error) => {
            let _ignored =
                log_diagnostic(&format!("Shell request cleanup retained files: {error}"));
            true
        },
    };
    if retained_unknown {
        let _ignored = log_diagnostic(
            "Preserved unknown or unavailable files in the private shell request directory.",
        );
    }
    match super::diagnostics::retained_runtime_files() {
        Ok(retained) => retained_unknown |= retained,
        Err(error) => {
            retained_unknown = true;
            let _ignored = log_diagnostic(&format!(
                "Maintenance runtime cleanup retained files: {error}"
            ));
        },
    }
    retained_unknown
}

fn finalize_preferences(tx: &Transaction) -> io::Result<()> {
    // Preferences survive uninstall; restore the pre-uninstall menu preference for the next installation.
    if tx.preferences == super::PreferencesDisposition::Purge {
        for name in ["Language", "MenusEnabled", "PathEnabled"] {
            registry::write(SETTINGS_KEY, name, None)?;
        }
        registry::remove_empty(SETTINGS_KEY);
        purge_logs()?;
    } else if let Some(change) = menu_preference_restore(&tx.registry) {
        registry::write(SETTINGS_KEY, "MenusEnabled", change.before.as_ref())?;
    }
    Ok(())
}

/// No recorded change means an already-disabled preference must remain intact.
pub(super) fn menu_preference_restore(
    changes: &[super::RegistryChange],
) -> Option<&super::RegistryChange> {
    changes
        .iter()
        .find(|change| change.key == SETTINGS_KEY && change.name == "MenusEnabled")
}
