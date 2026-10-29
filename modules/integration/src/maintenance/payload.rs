use std::fmt::Write as _;

use super::{
    BTreeSet, CLASSIC_CLSID, Component, Deserialize, Digest, File, INSTALL_KEY, OpenOptions,
    OpenOptionsExt, Ownership, Path, PathBuf, Payload, Read, SETTINGS_KEY, Serialize, Sha256,
    Write, check_file, check_no_reparse, guid, invalid, io, pcw, security, wide,
};

/// # Errors
/// Returns an error when protocol/state validation or the Windows operation fails.
pub fn validate_version(version: &str) -> io::Result<[u16; 4]> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 4 {
        return Err(invalid("four-part package version required"));
    }
    let mut parsed = [0; 4];
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty()
            || !part.bytes().all(|b| b.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return Err(invalid("invalid package version component"));
        }
        parsed[i] = part.parse().map_err(|error| {
            invalid(format!("package version component exceeds 65535: {error}"))
        })?;
    }
    Ok(parsed)
}

pub(super) fn relative(path: &str) -> io::Result<PathBuf> {
    let value = PathBuf::from(path);
    if path.is_empty()
        || value.is_absolute()
        || path.contains(':')
        || path.contains('\0')
        || value
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(invalid("unsafe manifest relative path"));
    }
    for component in value.components() {
        let text = component.as_os_str().to_string_lossy();
        if text.ends_with(['.', ' '])
            || text.eq_ignore_ascii_case("CON")
            || text.eq_ignore_ascii_case("NUL")
            || text.eq_ignore_ascii_case("AUX")
            || text.eq_ignore_ascii_case("PRN")
        {
            return Err(invalid("unsafe Windows path component"));
        }
    }
    Ok(value)
}

pub(super) fn read_json<T: for<'a> Deserialize<'a>>(path: &Path) -> io::Result<T> {
    check_no_reparse(path)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x0020_0000)
        .open(path)?;
    check_file(&file)?;
    if file.metadata()?.len() > 32 * 1024 * 1024 {
        return Err(invalid("oversized maintenance metadata"));
    }
    let mut bytes = Vec::new();
    file.take(32 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|e| invalid(e.to_string()))
}

pub(super) fn save_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    check_no_reparse(path)?;
    let temporary = path.with_extension(format!("{}.tmp", guid()?));
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| invalid(e.to_string()))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    move_file(&temporary, path)?;
    Ok(())
}

pub(super) fn move_file(source: &Path, destination: &Path) -> io::Result<()> {
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    check_no_reparse(source)?;
    check_no_reparse(destination)?;
    // SAFETY: terminated paths live for the synchronous, write-through rename.
    unsafe {
        MoveFileExW(
            pcw(&wide(source)),
            pcw(&wide(destination)),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(crate::win_error)
}

pub(super) fn hash_reader(file: &mut File) -> io::Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let mut encoded = String::with_capacity(64);
    for byte in hash.finalize() {
        write!(&mut encoded, "{byte:02x}").map_err(|error| invalid(error.to_string()))?;
    }
    Ok(encoded)
}

pub(super) fn hash(path: &Path) -> io::Result<String> {
    check_no_reparse(path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x0020_0000)
        .open(path)?;
    check_file(&file)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("manifest entry must be a regular file"));
    }
    hash_reader(&mut file)
}

/// Incoming sources must match this helper; installed sources retain their own policy.
pub(super) fn payload(source: &Path, incoming: bool) -> io::Result<Payload> {
    let payload: Payload = read_json(&source.join("payload.json"))?;
    validate_payload_metadata(&payload)?;
    if incoming {
        validate_build_policy(
            &payload,
            crate::SIGNED_BUILD,
            option_env!("UNLINKA_SIGNER_SUBJECT"),
        )?;
    }
    for entry in &payload.files {
        let rel = relative(&entry.path)?;
        let path = source.join(rel);
        if hash(&path)? != entry.sha256.to_ascii_lowercase() {
            return Err(invalid(format!("payload hash mismatch: {}", entry.path)));
        }
        if payload.signed
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("exe") || e.eq_ignore_ascii_case("dll"))
            && security::signed_subject(&path)? != payload.signer_subject
        {
            return Err(invalid("payload signer does not match signer subject"));
        }
    }
    Ok(payload)
}

pub(super) fn validate_build_policy(
    payload: &Payload,
    signed: bool,
    signer_subject: Option<&str>,
) -> io::Result<()> {
    if payload.signed != signed {
        return Err(invalid(
            "payload signing policy does not match this maintenance build",
        ));
    }
    if signed && signer_subject != Some(payload.signer_subject.as_str()) {
        return Err(invalid(
            "payload signer subject does not match this maintenance build",
        ));
    }
    Ok(())
}

pub(super) fn validate_payload_metadata(payload: &Payload) -> io::Result<()> {
    if payload.schema_version != 1
        || payload.protocol_version != 1
        || payload.files.len() > 4096
        || payload.signed == payload.signer_subject.is_empty()
    {
        return Err(invalid("unsupported or incomplete payload metadata"));
    }
    validate_version(&payload.package_version)?;
    let mut names = BTreeSet::new();
    for entry in &payload.files {
        relative(&entry.path)?;
        if !names.insert(entry.path.replace('\\', "/").to_ascii_lowercase())
            || entry.sha256.len() != 64
            || !entry.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(invalid("duplicate payload file or invalid SHA-256"));
        }
    }
    for required in ["unlinka.exe", "unlinka-desktop.exe", "unlinka-shell.dll"] {
        if !names.contains(required) {
            return Err(invalid(format!("payload is missing {required}")));
        }
    }
    Ok(())
}

pub(super) fn validate_upgrade_policy(
    before: &Ownership,
    payload: &Payload,
    active_version: &str,
) -> io::Result<()> {
    if before.signed != payload.signed
        && validate_version(&payload.package_version)? <= validate_version(active_version)?
    {
        return Err(invalid(
            "changing signing policy requires a newer package version",
        ));
    }
    if before.signed && payload.signed && before.signer_subject != payload.signer_subject {
        return Err(invalid(
            "signer subject cannot change during signed upgrade",
        ));
    }
    Ok(())
}

pub(super) fn owned_payload(
    root: &Path,
    version: &str,
    ownership: &Ownership,
) -> io::Result<Payload> {
    validate_version(version)?;
    let directory = root.join("versions").join(version);
    let metadata = format!("versions/{version}/payload.json");
    if ownership.files.get(&metadata) != Some(&hash(&directory.join("payload.json"))?) {
        return Err(invalid(
            "installed payload metadata failed ownership verification",
        ));
    }
    let payload = payload(&directory, false)?;
    if payload.package_version != version
        || payload.signed != ownership.signed
        || payload.signer_subject != ownership.signer_subject
    {
        return Err(invalid(
            "installed payload policy differs from ownership metadata",
        ));
    }
    for entry in &payload.files {
        let name = format!("versions/{version}/{}", entry.path.replace('\\', "/"));
        if ownership.files.get(&name) != Some(&entry.sha256.to_ascii_lowercase()) {
            return Err(invalid(
                "installed payload file differs from ownership metadata",
            ));
        }
    }
    Ok(payload)
}

pub(super) fn load_ownership(root: &Path) -> io::Result<Ownership> {
    let path = root.join("state/ownership.json");
    let ownership: Ownership = match read_json(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ownership {
            schema_version: 1,
            ..Default::default()
        },
        Err(error) => return Err(error),
    };
    validate_ownership(&ownership)?;
    Ok(ownership)
}

pub(super) fn validate_ownership(ownership: &Ownership) -> io::Result<()> {
    if ownership.schema_version != 1 {
        return Err(invalid("unknown ownership schema"));
    }
    if (ownership.signed && ownership.signer_subject.is_empty() && !ownership.files.is_empty())
        || (!ownership.signed && !ownership.signer_subject.is_empty())
    {
        return Err(invalid("invalid ownership signing policy"));
    }
    for path in ownership.files.keys().chain(ownership.pending.iter()) {
        validate_owned_path(path)?;
    }
    for owned in &ownership.registry {
        validate_registry_key(&owned.key)?;
    }
    Ok(())
}

pub(super) fn validate_owned_path(path: &str) -> io::Result<()> {
    let path = relative(path)?;
    if path == Path::new("bin/unlinka.exe")
        || path == Path::new("maintenance/unlinka-maintenance.exe")
    {
        return Ok(());
    }
    let components: Vec<_> = path.components().collect();
    if components.len() < 3 || components[0].as_os_str() != "versions" {
        return Err(invalid("ownership path is outside product payload layout"));
    }
    validate_version(
        components[1]
            .as_os_str()
            .to_str()
            .ok_or_else(|| invalid("invalid version directory"))?,
    )?;
    Ok(())
}

pub(super) fn handler_key() -> String {
    format!(
        "Software\\Classes\\AllFileSystemObjects\\shellex\\ContextMenuHandlers\\{}",
        crate::DATA_NAME
    )
}

pub(super) fn clsid_key() -> String { format!("Software\\Classes\\CLSID\\{CLASSIC_CLSID}") }

pub(super) fn validate_registry_key(key: &str) -> io::Result<()> {
    if key == INSTALL_KEY
        || key == SETTINGS_KEY
        || key == handler_key()
        || key == clsid_key()
        || key == format!("{}\\InprocServer32", clsid_key())
    {
        Ok(())
    } else {
        Err(invalid(
            "transaction registry key is outside product namespace",
        ))
    }
}
