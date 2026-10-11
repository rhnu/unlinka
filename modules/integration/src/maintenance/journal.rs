use std::io::{Seek, SeekFrom};

use super::{
    Artifact, INSTALL_KEY, Installation, InstallationState, OpenOptions, OpenOptionsExt, Ownership,
    Path, RegistryChange, Settings, Transaction, Value, check_file, check_no_reparse,
    cleanup_artifacts, compensation, copy_file, file_identity, fs, guid, hash, hash_reader,
    invalid, io, move_file, notify, owned_payload, package, path_present, read_json, registry,
    relative, remove_internal_file, remove_owned, request, rollback_path, save_json, settings,
    validate_owned_path, validate_ownership, validate_registry_key,
};

pub(super) fn rollback(root: &Path, tx: &Transaction) -> io::Result<()> {
    let mut recovery = tx.clone();
    let mut failures = Vec::new();
    let mut package_removed = true;
    if tx.package_touched
        && let Err(error) =
            rollback_package_publisher(tx).and_then(|publisher| package::remove(publisher, root))
    {
        package_removed = false;
        failures.push(error.to_string());
    }
    // Never remove an external-location payload while its package remains registered.
    if package_removed {
        rollback_files(root, tx, &mut recovery, &mut failures);
    }
    if tx.package_touched && package_removed {
        let result = (|| {
            if let Some(version) = &tx.before.package_version {
                let directory = root.join("versions").join(version);
                owned_payload(root, version, &tx.before)?;
                package::add(&tx.before.publisher, version, &directory, root)?;
            }
            Ok::<_, io::Error>(())
        })();
        if let Err(error) = result {
            failures.push(error.to_string());
        }
    }
    for change in tx.path.iter().rev() {
        if let Err(error) = rollback_path(change) {
            failures.push(error.to_string());
        }
    }
    for change in tx.registry.iter().rev() {
        let result = (|| {
            validate_registry_key(&change.key)?;
            let current = registry::read(&change.key, &change.name)?;
            let restored = compensation(
                current.as_ref(),
                change.before.as_ref(),
                change.after.as_ref(),
            )?;
            registry::write(&change.key, &change.name, restored.as_ref())
        })();
        if let Err(error) = result {
            failures.push(error.to_string());
        }
    }
    // Keep compensating backups until all restorations succeed so a later
    // recovery can retry package removal or file restoration without data loss.
    if failures.is_empty()
        && let Err(error) = cleanup_artifacts(root, &recovery)
    {
        failures.push(error.to_string());
    }
    if let Some(installation) = &tx.before_install {
        for (name, expected) in &tx.before.files {
            if (name.starts_with(&format!("versions/{}/", installation.active_version))
                || name.starts_with("bin/")
                || name.starts_with("maintenance/"))
                && hash(&root.join(relative(name)?)).as_ref().ok() != Some(expected)
            {
                failures.push(format!("prior payload requires repair: {name}"));
            }
        }
    }
    if failures.is_empty() {
        save_json(&root.join("state/ownership.json"), &tx.before)?;
        fs::remove_file(root.join("state/transaction.json"))?;
        notify();
        Ok(())
    } else {
        let mut retained = tx.after.clone();
        retained.files.extend(tx.before.files.clone());
        retained.pending = tx.files.iter().map(|c| c.path.clone()).collect();
        let helper = root.join("maintenance/unlinka-maintenance.exe");
        if !helper.exists()
            && let Some(expected) = tx.after.files.get("maintenance/unlinka-maintenance.exe")
        {
            let executable = std::env::current_exe()?;
            if hash(&executable)? == *expected {
                copy_file(&executable, &helper)?;
                retained.files.insert(
                    "maintenance/unlinka-maintenance.exe".into(),
                    expected.clone(),
                );
            }
        }

        save_json(&root.join("state/ownership.json"), &retained)?;
        registry::write(INSTALL_KEY, "InstallRoot", Some(&Value::string(root)))?;
        registry::write(INSTALL_KEY, "State", Some(&Value::string("RepairRequired")))?;
        Err(io::Error::other(format!(
            "maintenance rollback requires repair: {}",
            failures.join("; ")
        )))
    }
}

pub(super) fn rollback_package_publisher(tx: &Transaction) -> io::Result<&str> {
    let ownership = if tx.after.signed {
        &tx.after
    } else {
        &tx.before
    };
    if !ownership.signed || ownership.publisher.is_empty() {
        return Err(invalid("package rollback has no signed ownership policy"));
    }
    Ok(&ownership.publisher)
}

pub(super) fn recover(root: &Path) -> io::Result<()> {
    let path = root.join("state/transaction.json");
    let tx: Transaction = match read_json(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if tx.schema_version != 1 || !request::valid_id(&tx.id) {
        return Err(invalid("invalid transaction recovery identity"));
    }
    validate_ownership(&tx.before)?;
    validate_ownership(&tx.after)?;
    for change in &tx.files {
        validate_owned_path(&change.path)?;
    }
    for change in &tx.registry {
        validate_registry_key(&change.key)?;
    }
    if tx.removing {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "an uninstall transaction is pending; run uninstall again",
        ));
    }
    if tx.committed {
        cleanup_artifacts(root, &tx)?;
        fs::remove_file(path)?;
        return Ok(());
    }
    rollback(root, &tx)
}

pub(super) fn begin(
    root: &Path,
    before_install: Option<Installation>,
    before: Ownership,
    removing: bool,
) -> io::Result<Transaction> {
    let tx = Transaction {
        schema_version: 1,
        id: guid()?,
        removing,
        preferences: super::PreferencesDisposition::Retain,
        committed: false,
        before_install,
        after: before.clone(),
        before,
        registry: Vec::new(),
        files: Vec::new(),
        path: Vec::new(),
        package_touched: false,
        artifacts: Vec::new(),
    };
    tx.save(root)?;
    Ok(tx)
}

pub(super) fn commit(
    root: &Path,
    tx: &mut Transaction,
    version: &str,
    settings_value: &Settings,
) -> io::Result<()> {
    for (name, expected) in &tx.after.files {
        if (name.starts_with(&format!("versions/{version}/"))
            || name.starts_with("bin/")
            || name.starts_with("maintenance/"))
            && hash(&root.join(relative(name)?))? != *expected
        {
            return Err(invalid("published program verification failed"));
        }
    }
    if settings_value.menus_enabled {
        for owned in &tx.after.registry {
            if registry::read(&owned.key, &owned.name)? != Some(owned.value.clone()) {
                return Err(invalid("classic registration verification failed"));
            }
        }
        if tx.after.signed
            && package::modern_supported()?
            && !package::verify(
                &tx.after.publisher,
                version,
                &root.join("versions").join(version),
            )?
        {
            return Err(invalid("modern registration verification failed"));
        }
    }
    if settings_value.path_enabled && !path_present(root)? {
        return Err(invalid("PATH verification failed"));
    }
    settings(root, tx, settings_value)?;
    tx.write(
        root,
        INSTALL_KEY,
        "ActiveVersion",
        Some(Value::string(version)),
    )?;
    tx.write(
        root,
        INSTALL_KEY,
        "PackagePublisher",
        Some(Value::string(tx.after.publisher.clone())),
    )?;
    tx.write(
        root,
        INSTALL_KEY,
        "PackageName",
        Some(Value::string(crate::PACKAGE_NAME)),
    )?;
    tx.write(
        root,
        INSTALL_KEY,
        "Architecture",
        Some(Value::string("x64")),
    )?;
    tx.write(
        root,
        INSTALL_KEY,
        "PathOwned",
        Some(Value::number(u32::from(tx.after.path_owned.is_some()))),
    )?;
    save_json(&root.join("state/ownership.json"), &tx.after)?;
    tx.state(root, InstallationState::Ready)?;
    tx.committed = true;
    tx.save(root)?;
    // Committed recovery can finish housekeeping; registration is already complete.
    if cleanup_artifacts(root, tx).is_ok() {
        let _ignored = fs::remove_file(root.join("state/transaction.json"));
    }
    notify();
    Ok(())
}

impl Transaction {
    pub(super) fn save(&self, root: &Path) -> io::Result<()> {
        save_json(&root.join("state/transaction.json"), self)
    }

    pub(super) fn write(
        &mut self,
        root: &Path,
        key: &str,
        name: &str,
        after: Option<Value>,
    ) -> io::Result<()> {
        validate_registry_key(key)?;
        let before = registry::read(key, name)?;
        if before == after {
            return Ok(());
        }
        self.registry.push(RegistryChange {
            key: key.into(),
            name: name.into(),
            before: before.clone(),
            after,
        });
        self.save(root)?;
        if registry::read(key, name)? != before {
            return Err(invalid("registry value changed during maintenance"));
        }
        registry::write(
            key,
            name,
            self.registry
                .last()
                .ok_or_else(|| invalid("missing registry intent"))?
                .after
                .as_ref(),
        )
    }

    pub(super) fn state(&mut self, root: &Path, state: InstallationState) -> io::Result<()> {
        self.write(
            root,
            INSTALL_KEY,
            "State",
            Some(Value::string(format!("{state:?}"))),
        )
    }

    pub(super) fn copy(&mut self, root: &Path, source: &Path, target: &Path) -> io::Result<()> {
        let path = target
            .strip_prefix(root)
            .map_err(|error| invalid(format!("maintenance artifact is outside root: {error}")))?
            .to_str()
            .ok_or_else(|| invalid("artifact path is not Unicode"))?
            .replace('\\', "/");
        check_no_reparse(source)?;
        check_no_reparse(target)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
            check_no_reparse(parent)?;
        }
        let mut input = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .custom_flags(0x0020_0000)
            .open(source)?;
        check_file(&input)?;
        self.artifacts.push(Artifact {
            path,
            expected: None,
            identity: None,
        });
        self.save(root)?;
        let mut output = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .share_mode(0)
            .custom_flags(0x0020_0000)
            .open(target)
        {
            Ok(file) => file,
            Err(error) => {
                self.artifacts.pop();
                self.save(root)?;
                return Err(error);
            },
        };
        self.artifacts
            .last_mut()
            .ok_or_else(|| invalid("missing copy intent"))?
            .identity = Some(file_identity(&output)?);
        self.save(root)?;
        io::copy(&mut input, &mut output)?;
        output.sync_all()?;

        output.seek(SeekFrom::Start(0))?;
        let expected = hash_reader(&mut output)?;
        self.artifacts
            .last_mut()
            .ok_or_else(|| invalid("missing copy intent"))?
            .expected = Some(expected);
        self.save(root)
    }
}

fn rollback_files(
    root: &Path,
    tx: &Transaction,
    recovery: &mut Transaction,
    failures: &mut Vec<String>,
) {
    for change in tx.files.iter().rev() {
        let result = (|| {
            validate_owned_path(&change.path)?;
            let path = root.join(relative(&change.path)?);
            let current = match hash(&path) {
                Ok(value) => Some(value),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };
            if current == change.before {
                return Ok(());
            }
            if current.as_ref().is_some_and(|h| h != &change.after) {
                return Err(invalid("file changed externally during rollback"));
            }
            match (&change.before, &change.backup) {
                (Some(expected), Some(backup)) => {
                    let backup_path = relative(backup)?;
                    if !backup_path.starts_with(Path::new("state/backups").join(&tx.id))
                        || hash(&root.join(&backup_path))? != *expected
                    {
                        return Err(invalid(
                            "rollback binary backup failed ownership verification",
                        ));
                    }
                    let temporary = path.with_extension(format!("{}.restore", tx.id));
                    if temporary.exists() {
                        let name = temporary
                            .strip_prefix(root)
                            .map_err(|error| {
                                invalid(format!("restore artifact outside root: {error}"))
                            })?
                            .to_str()
                            .ok_or_else(|| invalid("restore artifact not Unicode"))?
                            .replace('\\', "/");
                        let artifact = recovery
                            .artifacts
                            .iter()
                            .find(|a| a.path == name)
                            .ok_or_else(|| invalid("unowned restore artifact was preserved"))?;
                        if artifact.identity.is_none() {
                            return Err(invalid(
                                "unresolved restore creation intent was preserved",
                            ));
                        }
                        remove_internal_file(
                            root,
                            &relative(&name)?,
                            artifact.expected.as_deref(),
                            artifact.identity.as_ref(),
                        )?;
                    }
                    recovery.copy(root, &root.join(backup_path), &temporary)?;
                    move_file(&temporary, &path)
                },
                (None, None) => remove_owned(root, &change.path, &change.after),
                _ => Err(invalid("incomplete rollback metadata")),
            }
        })();
        if let Err(error) = result {
            failures.push(error.to_string());
        }
    }
}
