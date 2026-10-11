use super::{
    AsRawHandle, FileChange, OpenOptions, OpenOptionsExt, Path, Payload, Transaction, check_file,
    check_no_reparse, fs, hash, hash_reader, invalid, io, move_file, relative, size_of,
    validate_owned_path,
};

pub(super) fn copy_file(source: &Path, target: &Path) -> io::Result<()> {
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
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .custom_flags(0x0020_0000)
        .open(target)?;
    io::copy(&mut input, &mut output)?;
    output.sync_all()
}

pub(super) fn replace_owned(
    root: &Path,
    tx: &mut Transaction,
    source: &Path,
    name: &str,
) -> io::Result<()> {
    validate_owned_path(name)?;
    let target = root.join(relative(name)?);
    let expected = hash(source)?;
    let current = match hash(&target) {
        Ok(value) => Some(value),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if current.as_ref() == Some(&expected) {
        if !tx.after.files.contains_key(name) {
            return Err(invalid(
                "existing stable file is not owned by this installation",
            ));
        }
        return Ok(());
    }
    if current.is_some() && !tx.after.files.contains_key(name) {
        return Err(invalid("stable binary file ownership conflict"));
    }
    let backup = if current.is_some() {
        let backup = format!("state/backups/{}/{}", tx.id, name.replace('/', "_"));
        tx.copy(root, &target, &root.join(&backup))?;
        Some(backup)
    } else {
        None
    };
    tx.files.push(FileChange {
        path: name.into(),
        before: current,
        after: expected.clone(),
        backup,
    });
    tx.after.files.insert(name.into(), expected);
    tx.save(root)?;
    let temporary = target.with_extension(format!("{}.new", tx.id));
    tx.copy(root, source, &temporary)?;
    move_file(&temporary, &target)
}

pub(super) fn publish(
    root: &Path,
    tx: &mut Transaction,
    source: &Path,
    payload: &Payload,
) -> io::Result<()> {
    let final_dir = root.join("versions").join(&payload.package_version);
    check_no_reparse(&final_dir)?;
    if final_dir.exists() {
        for entry in &payload.files {
            let name = format!(
                "versions/{}/{}",
                payload.package_version,
                entry.path.replace('\\', "/")
            );
            if tx.before.files.get(&name) != Some(&entry.sha256.to_ascii_lowercase()) {
                return Err(invalid(
                    "existing version directory does not match owned payload",
                ));
            }
        }
        // Same-version repair restores only missing/corrupt owned files, never overwrites a loaded DLL.
        for entry in &payload.files {
            let name = format!(
                "versions/{}/{}",
                payload.package_version,
                entry.path.replace('\\', "/")
            );
            replace_owned(root, tx, &source.join(relative(&entry.path)?), &name)?;
        }
        let metadata = format!("versions/{}/payload.json", payload.package_version);
        replace_owned(root, tx, &source.join("payload.json"), &metadata)?;
        return Ok(());
    }
    let staged = root.join("staging").join(&tx.id);
    fs::create_dir_all(&staged)?;
    check_no_reparse(&staged)?;
    for entry in &payload.files {
        tx.copy(
            root,
            &source.join(relative(&entry.path)?),
            &staged.join(relative(&entry.path)?),
        )?;
    }
    tx.copy(
        root,
        &source.join("payload.json"),
        &staged.join("payload.json"),
    )?;
    let staged_payload = super::payload(&staged, true)?;
    if staged_payload.package_version != payload.package_version {
        return Err(invalid("staging version mismatch"));
    }
    for entry in &payload.files {
        let name = format!(
            "versions/{}/{}",
            payload.package_version,
            entry.path.replace('\\', "/")
        );
        tx.after
            .files
            .insert(name.clone(), entry.sha256.to_ascii_lowercase());
        tx.files.push(FileChange {
            path: name,
            before: None,
            after: entry.sha256.to_ascii_lowercase(),
            backup: None,
        });
    }
    let metadata = format!("versions/{}/payload.json", payload.package_version);
    let metadata_hash = hash(&staged.join("payload.json"))?;
    tx.after
        .files
        .insert(metadata.clone(), metadata_hash.clone());
    tx.files.push(FileChange {
        path: metadata,
        before: None,
        after: metadata_hash,
        backup: None,
    });
    tx.save(root)?;
    fs::create_dir_all(root.join("versions"))?;
    move_file(&staged, &final_dir)
}

/// Delete an owned file by a non-shared, no-reparse handle after hashing that same handle.
pub(super) fn remove_owned(root: &Path, path: &str, expected: &str) -> io::Result<()> {
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
        },
    };
    validate_owned_path(path)?;
    let path = root.join(relative(path)?);
    check_no_reparse(&path)?;
    let mut file = match OpenOptions::new()
        .access_mode(0x8000_0000 | 0x0001_0000)
        .share_mode(0)
        .custom_flags(0x0020_0000)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    check_file(&file)?;
    if hash_reader(&mut file)? != expected {
        return Err(invalid(format!(
            "owned file changed externally and was preserved: {}",
            path.display()
        )));
    }
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: the file was opened with DELETE rights; hashing and disposition use the same protected handle.
    unsafe {
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileDispositionInfo,
            (&raw const info).cast(),
            u32::try_from(size_of::<FILE_DISPOSITION_INFO>())
                .expect("Windows structure size fits u32"),
        )
    }
    .map_err(crate::win_error)?;
    drop(file);
    let mut parent = path.parent();
    while let Some(directory) = parent {
        if directory == root || directory == root.join("versions") {
            break;
        }
        check_no_reparse(directory)?;
        if fs::remove_dir(directory).is_err() {
            break;
        }
        parent = directory.parent();
    }
    Ok(())
}
