use super::{
    AsRawHandle, File, FileIdentity, OpenOptions, OpenOptionsExt, Path, Transaction, check_file,
    check_no_reparse, fs, hash_reader, invalid, io, log_diagnostic, relative, request, size_of,
};

pub(super) fn validate_artifact(tx: &Transaction, path: &Path) -> io::Result<()> {
    if !request::valid_id(&tx.id) {
        return Err(invalid("invalid artifact transaction id"));
    }
    if path.starts_with(Path::new("staging").join(&tx.id))
        || path.starts_with(Path::new("state/backups").join(&tx.id))
    {
        return Ok(());
    }
    let name = path
        .to_str()
        .ok_or_else(|| invalid("non-Unicode artifact"))?;
    let Some(original) = name
        .strip_suffix(&format!(".{}.new", tx.id))
        .or_else(|| name.strip_suffix(&format!(".{}.restore", tx.id)))
    else {
        return Err(invalid("artifact is outside its transaction namespace"));
    };
    // with_extension replaces the original extension; only known executable locations can use these siblings.
    let original = format!("{original}.exe");
    if original == "bin/unlinka.exe"
        || original == "maintenance/unlinka-maintenance.exe"
        || original.starts_with("versions/")
    {
        Ok(())
    } else {
        Err(invalid("invalid replacement artifact"))
    }
}

pub(super) fn remove_internal_file(
    root: &Path,
    relative_path: &Path,
    expected: Option<&str>,
    expected_identity: Option<&FileIdentity>,
) -> io::Result<()> {
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_DISPOSITION_INFO, FileDispositionInfo,
            GetFileInformationByHandle, SetFileInformationByHandle,
        },
    };
    let path = root.join(relative_path);
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
    if expected_identity
        .is_some_and(|expected| file_identity(&file).as_ref().ok() != Some(expected))
    {
        return Err(invalid("maintenance artifact file identity changed"));
    }
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the output structure and protected file handle live for the synchronous metadata query.
    unsafe {
        GetFileInformationByHandle(
            windows::Win32::Foundation::HANDLE(file.as_raw_handle()),
            &raw mut info,
        )
    }
    .map_err(crate::win_error)?;
    if info.nNumberOfLinks != 1 || !file.metadata()?.is_file() {
        return Err(invalid(
            "internal cleanup entry is not an owned regular file",
        ));
    }
    if expected.is_some_and(|expected| {
        hash_reader(&mut file).as_ref().ok().map(String::as_str) != Some(expected)
    }) {
        return Err(invalid("maintenance artifact was changed externally"));
    }
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: the owned handle grants DELETE access, and the disposition structure is correctly sized.
    unsafe {
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileDispositionInfo,
            (&raw const disposition).cast(),
            u32::try_from(size_of::<FILE_DISPOSITION_INFO>())
                .expect("Windows structure size fits u32"),
        )
    }
    .map_err(crate::win_error)
}

pub(super) fn file_identity(file: &File) -> io::Result<FileIdentity> {
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the metadata structure and owned file handle remain live for this synchronous query.
    unsafe {
        GetFileInformationByHandle(
            windows::Win32::Foundation::HANDLE(file.as_raw_handle()),
            &raw mut info,
        )
    }
    .map_err(crate::win_error)?;
    Ok(FileIdentity {
        volume: info.dwVolumeSerialNumber,
        high: info.nFileIndexHigh,
        low: info.nFileIndexLow,
    })
}

pub(super) fn cleanup_artifacts(root: &Path, tx: &Transaction) -> io::Result<()> {
    for artifact in &tx.artifacts {
        let path = relative(&artifact.path)?;
        validate_artifact(tx, &path)?;
        if artifact.identity.is_none() && artifact.expected.is_none() {
            // A write-ahead create intent is not proof that creation succeeded.
            if root.join(&path).exists() {
                let _ignored = log_diagnostic("Preserved an unresolved maintenance create intent");
            }
            continue;
        }
        remove_internal_file(
            root,
            &path,
            artifact.expected.as_deref(),
            artifact.identity.as_ref(),
        )?;
        let mut parent = path.parent();
        while let Some(directory) = parent {
            if directory == Path::new("state")
                || directory == Path::new("staging")
                || directory == Path::new("versions")
                || directory == Path::new("bin")
                || directory == Path::new("maintenance")
            {
                break;
            }
            let full = root.join(directory);
            check_no_reparse(&full)?;
            if full.exists() && fs::remove_dir(&full).is_err() {
                break;
            }
            parent = directory.parent();
        }
    }
    Ok(())
}
