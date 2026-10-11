#![cfg(windows)]

use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use windows::{
    Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            LOCK_FILE_FLAGS, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
        },
        System::IO::OVERLAPPED,
    },
    core::PCWSTR,
};

pub mod maintenance;
mod package;
mod registry;
pub mod request;
mod security;
pub mod status;

include!(concat!(env!("OUT_DIR"), "/identity.rs"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallationState {
    Installing,
    Ready,
    Updating,
    RepairRequired,
    Removing,
    RemovalPending,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Installation {
    pub root: PathBuf,
    pub active_version: String,
    pub state: InstallationState,
}

impl Installation {
    /// Read the current-user installation record.
    ///
    /// # Errors
    /// Fails for invalid registry values, an unexpected root, reparse components or
    /// a Ready installation without a valid active version.
    pub fn current() -> io::Result<Option<Self>> {
        let Some(root) = registry::read(INSTALL_KEY, "InstallRoot")? else {
            return Ok(None);
        };
        let root = PathBuf::from(root.os_string()?);
        if root != install_root()? {
            return Err(invalid(
                "installation root is not the fixed current-user location",
            ));
        }
        check_no_reparse(&root)?;
        let version = registry::read_string(INSTALL_KEY, "ActiveVersion")?.unwrap_or_default();
        if !version.is_empty() {
            maintenance::validate_version(&version)?;
        }
        let state = registry::read_string(INSTALL_KEY, "State")?
            .ok_or_else(|| invalid("installation state is missing"))?;
        let state = match state.as_str() {
            "Installing" => InstallationState::Installing,
            "Ready" => InstallationState::Ready,
            "Updating" => InstallationState::Updating,
            "RepairRequired" => InstallationState::RepairRequired,
            "Removing" => InstallationState::Removing,
            "RemovalPending" => InstallationState::RemovalPending,
            _ => return Err(invalid("unknown installation state")),
        };
        if state == InstallationState::Ready && version.is_empty() {
            return Err(invalid("ready installation has no active version"));
        }
        Ok(Some(Self {
            root,
            active_version: version,
            state,
        }))
    }

    /// Return the version directory recorded by this installation.
    pub fn active_dir(&self) -> PathBuf { self.root.join("versions").join(&self.active_version) }

    /// Resolve an installed entry point after rejecting reparse components.
    ///
    /// # Errors
    /// Returns an error for unknown names, invalid versions or reparse components.
    pub fn executable(&self, name: &str) -> io::Result<PathBuf> {
        if !matches!(name, "unlinka.exe" | "unlinka-desktop.exe") {
            return Err(invalid("unknown installation entry point"));
        }
        maintenance::validate_version(&self.active_version)?;
        let path = self.active_dir().join(name);
        check_no_reparse(&path)?;
        Ok(path)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub language: String,
    pub menus_enabled: bool,
    pub path_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            menus_enabled: true,
            path_enabled: true,
        }
    }
}

/// # Errors
/// Fails if saved registry values have invalid types or an unsupported language.
pub fn read_settings() -> io::Result<Settings> {
    let settings = Settings {
        language: registry::read_string(SETTINGS_KEY, "Language")?.unwrap_or_else(|| "auto".into()),
        menus_enabled: registry::read(SETTINGS_KEY, "MenusEnabled")?
            .map(|v| v.dword())
            .transpose()?
            .unwrap_or(1)
            != 0,
        path_enabled: registry::read(SETTINGS_KEY, "PathEnabled")?
            .map(|v| v.dword())
            .transpose()?
            .unwrap_or(1)
            != 0,
    };
    validate_language(&settings.language)?;
    Ok(settings)
}

pub(crate) fn validate_language(language: &str) -> io::Result<()> {
    if matches!(language, "auto" | "zh-CN" | "en-US") {
        Ok(())
    } else {
        Err(invalid("unsupported language"))
    }
}

/// A shared kernel lease lives through confirmation, execution and result output.
pub struct AppLease {
    _file: File,
}

impl AppLease {
    /// # Errors
    /// Fails if maintenance owns the lease, the installation is not Ready, or its
    /// registry record or lock file cannot be validated.
    pub fn acquire_if_installed() -> io::Result<Option<Self>> {
        let Some(installation) = Installation::current()? else {
            return Ok(None);
        };
        let file = lock(&installation.root, false)?;
        let current =
            Installation::current()?.ok_or_else(|| invalid("installation was removed"))?;
        if current.state != InstallationState::Ready {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("installation is {:?}", current.state),
            ));
        }
        Ok(Some(Self { _file: file }))
    }
}

pub(crate) fn lock(root: &Path, exclusive: bool) -> io::Result<File> {
    check_no_reparse(root)?;
    fs::create_dir_all(root.join("state"))?;
    check_no_reparse(&root.join("state"))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(3)
        .custom_flags(0x0020_0000)
        .open(root.join("state/activity.lock"))?;
    check_file(&file)?;
    let mut overlapped = OVERLAPPED::default();
    let flags = LOCKFILE_FAIL_IMMEDIATELY
        | if exclusive {
            LOCKFILE_EXCLUSIVE_LOCK
        } else {
            LOCK_FILE_FLAGS::default()
        };
    // SAFETY: the owned file handle and zero-offset OVERLAPPED remain live for the call.
    unsafe {
        LockFileEx(
            HANDLE(file.as_raw_handle()),
            flags,
            None,
            1,
            0,
            &raw mut overlapped,
        )
    }
    .map_err(|error| io::Error::new(io::ErrorKind::WouldBlock, error.to_string()))?;
    Ok(file)
}

/// # Errors
/// Fails if Windows cannot resolve the current user `LocalAppData` known folder.
pub fn install_root() -> io::Result<PathBuf> {
    Ok(user_data()?.join("Programs").join(INSTALL_NAME))
}

pub(crate) fn user_data() -> io::Result<PathBuf> {
    // Known folder resolution avoids trusting an inherited LOCALAPPDATA value.
    use windows::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
    };
    // SAFETY: the known-folder API returns a COM-task allocation through its typed result.
    let path = unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None) }
        .map_err(win_error)?;
    // SAFETY: path is a live terminated UTF-16 allocation returned by the preceding API.
    let result = PathBuf::from(std::ffi::OsString::from_wide(unsafe { path.as_wide() }));
    // SAFETY: this is the documented allocator for SHGetKnownFolderPath output.
    unsafe {
        CoTaskMemFree(Some(path.0.cast()));
    }
    Ok(result)
}

pub(crate) fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

pub(crate) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[derive(Debug)]
struct WindowsFailure(windows::core::Error);

impl std::fmt::Display for WindowsFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} (HRESULT {:#010x})", self.0, self.0.code().0)
    }
}

impl std::error::Error for WindowsFailure {}

pub(crate) fn win_error(error: windows::core::Error) -> io::Error {
    io::Error::other(WindowsFailure(error))
}

pub(crate) fn check_file(file: &File) -> io::Result<()> {
    if file.metadata()?.file_attributes() & 0x400 != 0 {
        Err(invalid("reparse entries are not allowed"))
    } else {
        Ok(())
    }
}

pub(crate) fn check_no_reparse(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(invalid("absolute path required"));
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_attributes() & 0x400 != 0 => {
                return Err(invalid("reparse path components are not allowed"));
            },
            Ok(_) => {},
            Err(error) if error.kind() == io::ErrorKind::NotFound => {},
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(crate) fn guid() -> io::Result<String> {
    // SAFETY: GUID generation has no pointer arguments and requires no COM apartment.
    let value = unsafe { windows::Win32::System::Com::CoCreateGuid() }.map_err(win_error)?;
    Ok(format!("{value:?}")
        .trim_matches(['{', '}'])
        .to_ascii_lowercase())
}

pub(crate) fn notify() {
    use windows::Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::{
            Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify},
            WindowsAndMessaging::{
                HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
            },
        },
    };
    let environment = wide("Environment");
    // SAFETY: the broadcast string lives for the bounded synchronous call; Shell uses no item pointers.
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
    // SAFETY: the environment string remains live during the bounded synchronous broadcast.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(environment.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            2000,
            None,
        );
    }
}

pub(crate) fn pcw(value: &[u16]) -> PCWSTR { PCWSTR(value.as_ptr()) }
