use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    mem::size_of,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::OpenOptionsExt,
        io::AsRawHandle,
    },
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use windows::Win32::{
    Foundation::HANDLE,
    Storage::FileSystem::{FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle},
};

use crate::{
    AppLease, Installation, check_file, guid, invalid, security, user_data, validate_language,
};

pub const MAX_SELECTIONS: usize = 65_536;
pub const MAX_REQUEST_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellRequest {
    schema_version: u32,
    action: String,
    #[serde(deserialize_with = "deserialize_paths")]
    pub paths_utf16: Vec<Vec<u16>>,
    pub locale: String,
}

fn deserialize_paths<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<Vec<u16>>, D::Error> {
    struct Paths;
    impl<'de> serde::de::Visitor<'de> for Paths {
        type Value = Vec<Vec<u16>>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a bounded array of filesystem UTF-16 paths")
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> Result<Self::Value, A::Error> {
            let mut paths = Vec::new();
            while let Some(path) = sequence.next_element::<Vec<u16>>()? {
                if paths.len() == MAX_SELECTIONS || path.len() > 32767 {
                    return Err(serde::de::Error::custom(
                        "selection or path length exceeds protocol limits",
                    ));
                }
                paths.push(path);
            }
            Ok(paths)
        }
    }
    deserializer.deserialize_seq(Paths)
}

struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(invalid("shell selection exceeds 32 MiB protocol limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

fn serialize(request: &ShellRequest, limit: usize) -> io::Result<Vec<u8>> {
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut writer, request).map_err(|e| invalid(e.to_string()))?;
    Ok(writer.bytes)
}

impl ShellRequest {
    /// # Errors
    /// Fails if the request contains empty, relative, rooted-volume or NUL paths,
    /// an unsupported locale, or exceeds the protocol bounds.
    pub fn paths(&self) -> io::Result<Vec<PathBuf>> {
        validate(self)?;
        Ok(self
            .paths_utf16
            .iter()
            .map(|p| PathBuf::from(OsString::from_wide(p)))
            .collect())
    }
}

fn validate(request: &ShellRequest) -> io::Result<()> {
    if request.schema_version != 1 || request.action != "permanent_delete" {
        return Err(invalid("unsupported shell request protocol or action"));
    }
    validate_language(&request.locale)?;
    if request.paths_utf16.is_empty() || request.paths_utf16.len() > MAX_SELECTIONS {
        return Err(invalid("shell selection count is outside protocol limits"));
    }
    for path in &request.paths_utf16 {
        if path.is_empty() || path.len() > 32767 || path.contains(&0) {
            return Err(invalid("invalid filesystem path in shell request"));
        }
        let path = PathBuf::from(OsString::from_wide(path));
        if !path.is_absolute() || path.parent().is_none() {
            return Err(invalid(
                "shell requests require absolute non-root filesystem paths",
            ));
        }
    }
    Ok(())
}

fn directory() -> io::Result<(PathBuf, File, File)> {
    let base = user_data()?.join(crate::DATA_NAME);
    let base_guard = security::private_dir(&base)?;
    let directory = base.join("requests");
    let guard = security::private_dir(&directory)?;
    Ok((directory, base_guard, guard))
}

pub fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

fn open_request(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .access_mode(0x8000_0000 | 0x0001_0000)
        .share_mode(1)
        .custom_flags(0x0020_0000)
        .open(path)?;
    check_file(&file)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_REQUEST_BYTES {
        return Err(invalid("invalid or oversized shell request file"));
    }
    Ok(file)
}

fn delete_open(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: the request is opened with DELETE rights, without write or delete sharing, and the structure is correctly sized.
    unsafe {
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileDispositionInfo,
            (&raw const info).cast(),
            u32::try_from(size_of::<FILE_DISPOSITION_INFO>())
                .expect("Windows structure size fits u32"),
        )
    }
    .map_err(crate::win_error)
}

/// # Errors
/// Fails for an invalid GUID, inaccessible or replaced request file, oversized
/// content, unsupported schema or action, or invalid selection paths.
pub fn consume_shell_request(id: &str) -> io::Result<ShellRequest> {
    if !valid_id(id) {
        return Err(invalid("invalid shell request GUID"));
    }
    let (directory, _base, _guard) = directory()?;
    let mut file = open_request(&directory.join(format!("{id}.json")))?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_REQUEST_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let result = if bytes.len() as u64 > MAX_REQUEST_BYTES {
        Err(invalid("oversized shell request"))
    } else {
        serde_json::from_slice::<ShellRequest>(&bytes)
            .map_err(|e| invalid(e.to_string()))
            .and_then(|request| {
                validate(&request)?;
                Ok(request)
            })
    };
    delete_open(&file)?;
    result
}

/// # Errors
/// Fails if installation menus are unavailable, any selection is invalid, the
/// private request cannot be written, or the installed desktop cannot start.
pub fn submit_shell_request(paths: &[PathBuf], locale: &str) -> io::Result<()> {
    let _lease = AppLease::acquire_if_installed()?
        .ok_or_else(|| invalid("shell task requires a ready installation"))?;
    if !crate::read_settings()?.menus_enabled {
        return Err(invalid("Explorer menus are disabled"));
    }
    let install = Installation::current()?.ok_or_else(|| invalid("installation is missing"))?;
    let request = ShellRequest {
        schema_version: 1,
        action: "permanent_delete".into(),
        paths_utf16: paths
            .iter()
            .map(|p| p.as_os_str().encode_wide().collect())
            .collect(),
        locale: locale.into(),
    };
    validate(&request)?;
    let bytes = serialize(
        &request,
        usize::try_from(MAX_REQUEST_BYTES).map_err(|error| invalid(error.to_string()))?,
    )?;
    let (directory, _base, _guard) = directory()?;
    cleanup_in(&directory)?;
    let id = guid()?;
    let path = directory.join(format!("{id}.json"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .custom_flags(0x0020_0000)
        .open(&path)?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ignored = fs::remove_file(path);
        return Err(error);
    }
    drop(file);
    let launch = install
        .executable("unlinka-desktop.exe")
        .and_then(|executable| {
            Command::new(executable)
                .arg("--shell-request")
                .arg(&id)
                .current_dir(&install.root)
                .spawn()
        });
    match launch {
        Ok(_) => Ok(()),
        Err(error) => {
            if let Ok(file) = open_request(&path) {
                let _ignored = delete_open(&file);
            }
            Err(error)
        },
    }
}

pub(crate) fn cleanup_stale() -> io::Result<()> {
    let (path, _base, _guard) = directory()?;
    cleanup_in(&path)
}

fn cleanup_in(directory: &Path) -> io::Result<()> { cleanup_matching(directory, true).map(|_| ()) }

/// Called while uninstall owns the exclusive lease and entry points are disabled.
pub(crate) fn cleanup_owned() -> io::Result<bool> {
    let (path, _base, _guard) = directory()?;
    cleanup_matching(&path, false)
}

fn cleanup_matching(directory: &Path, stale_only: bool) -> io::Result<bool> {
    let mut retained = false;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let owned_name = name
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .is_some_and(valid_id);
        if !owned_name {
            retained = true;
            continue;
        }
        let Ok(file) = open_request(&entry.path()) else {
            retained = true;
            continue;
        };
        if stale_only
            && file
                .metadata()?
                .modified()?
                .elapsed()
                .unwrap_or(Duration::ZERO)
                <= Duration::from_hours(24)
        {
            continue;
        }
        let mut bytes = Vec::new();
        (&file)
            .take(MAX_REQUEST_BYTES + 1)
            .read_to_end(&mut bytes)?;
        // A GUID alone never grants ownership of malformed or foreign content.
        if serde_json::from_slice::<ShellRequest>(&bytes).is_ok_and(|r| validate(&r).is_ok()) {
            if delete_open(&file).is_err() {
                retained = true;
            }
        } else {
            retained = true;
        }
    }
    Ok(retained)
}

#[cfg(test)]
mod tests;
