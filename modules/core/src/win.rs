use std::{
    ffi::OsString,
    io,
    mem::size_of,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};

use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER,
        ERROR_NO_MORE_FILES, ERROR_NOT_SUPPORTED, HANDLE, INVALID_HANDLE_VALUE,
    },
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, DELETE, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_FLAG_DELETE,
        FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FileDispositionInfo, FileDispositionInfoEx, FileIdInfo, FindClose, FindExInfoBasic,
        FindExSearchNameMatch, FindFirstFileExW, FindNextFileW, GetFileInformationByHandle,
        GetFileInformationByHandleEx, GetFinalPathNameByHandleW, OPEN_EXISTING,
        SetFileInformationByHandle, WIN32_FIND_DATAW,
    },
    System::SystemInformation::GetWindowsDirectoryW,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    File,
    Directory,
    Link,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Identity {
    volume: u64,
    id: [u8; 16],
    pub kind: Kind,
}

/// Owns an independent kernel file handle. It is never used for seek/read/write.
pub(crate) struct Handle(HANDLE);

// SAFETY: A live kernel file handle can be moved between threads. Ownership
// remains unique and Rust borrows prevent CloseHandle while an operation runs.
unsafe impl Send for Handle {}

// SAFETY: The exposed operations do not use seek state or user buffers shared
// across calls. Kernel metadata queries and disposition calls are thread-safe.
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: Construction accepts only a valid, independently owned file
        // handle. Drop is the sole closer and runs after all borrows have ended.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

impl Handle {
    pub fn open(path: &Path, delete: bool, pin: bool) -> io::Result<Self> {
        let name = wide(path)?;
        let access = FILE_READ_ATTRIBUTES | if delete { DELETE } else { 0 };
        let sharing = if pin {
            FILE_SHARE_READ
        } else {
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
        };
        // SAFETY: name is NUL-terminated UTF-16 and stays live for the call;
        // security/template pointers are null, and no output pointers are used.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                access,
                sharing,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }

    pub fn identity(&self) -> io::Result<Identity> {
        let basic = self.information()?;
        let kind = kind_from_attributes(basic.dwFileAttributes);
        let mut id = FILE_ID_INFO::default();
        // SAFETY: FileIdInfo expects FILE_ID_INFO. id is exclusively accessible
        // and its real byte size is supplied; the handle stays live for the call.
        if unsafe {
            GetFileInformationByHandleEx(
                self.0,
                FileIdInfo,
                (&raw mut id).cast(),
                structure_size::<FILE_ID_INFO>(),
            )
        } != 0
        {
            return Ok(Identity {
                volume: id.VolumeSerialNumber,
                id: id.FileId.Identifier,
                kind,
            });
        }
        let error = io::Error::last_os_error();
        if !unsupported(&error) {
            return Err(error);
        }
        let mut identifier = [0; 16];
        identifier[..4].copy_from_slice(&basic.nFileIndexLow.to_le_bytes());
        identifier[4..8].copy_from_slice(&basic.nFileIndexHigh.to_le_bytes());
        Ok(Identity {
            volume: u64::from(basic.dwVolumeSerialNumber),
            id: identifier,
            kind,
        })
    }

    /// Internal entries need a fresh type check, but no identity comparison.
    pub fn kind(&self) -> io::Result<Kind> {
        Ok(kind_from_attributes(self.information()?.dwFileAttributes))
    }

    fn information(&self) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
        let mut basic = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: basic is correctly sized, aligned output storage; the owned
        // handle and exclusive output borrow both remain valid during the call.
        if unsafe { GetFileInformationByHandle(self.0, &raw mut basic) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(basic)
    }

    pub fn require_directory(&self) -> io::Result<()> {
        if self.kind()? != Kind::Directory {
            return Err(io::Error::other(
                "pinned directory is no longer an ordinary directory",
            ));
        }
        Ok(())
    }

    pub fn final_path(&self) -> io::Result<PathBuf> {
        let mut buffer = vec![0; 512];
        loop {
            let capacity =
                u32::try_from(buffer.len()).expect("Win32 path buffers fit a DWORD length");
            // SAFETY: buffer is writable UTF-16 storage of capacity elements.
            // The owned handle is live and the API does not retain the pointer.
            let len =
                unsafe { GetFinalPathNameByHandleW(self.0, buffer.as_mut_ptr(), capacity, 0) };
            if len == 0 {
                return Err(io::Error::last_os_error());
            }
            if (len as usize) < buffer.len() {
                buffer.truncate(len as usize);
                return Ok(PathBuf::from(OsString::from_wide(&buffer)));
            }
            buffer.resize(len as usize + 1, 0);
        }
    }

    pub fn remove(&self) -> io::Result<()> {
        let disposition = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
        };
        // SAFETY: FileDispositionInfoEx expects FILE_DISPOSITION_INFO_EX;
        // disposition and its byte size match the ABI and live until return.
        // This API treats the information pointer as input-only.
        if unsafe {
            SetFileInformationByHandle(
                self.0,
                FileDispositionInfoEx,
                (&raw const disposition).cast(),
                structure_size::<FILE_DISPOSITION_INFO_EX>(),
            )
        } != 0
        {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if !unsupported(&error) {
            return Err(error);
        }
        let legacy = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: FileDispositionInfo expects FILE_DISPOSITION_INFO; the input
        // structure and its real byte size remain valid for the synchronous call.
        if unsafe {
            SetFileInformationByHandle(
                self.0,
                FileDispositionInfo,
                (&raw const legacy).cast(),
                structure_size::<FILE_DISPOSITION_INFO>(),
            )
        } == 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

fn kind_from_attributes(attributes: u32) -> Kind {
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        Kind::Link
    } else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        Kind::Directory
    } else {
        Kind::File
    }
}

pub(crate) fn unsupported(error: &io::Error) -> bool {
    matches!(
        error_code(error),
        Some(ERROR_INVALID_FUNCTION | ERROR_NOT_SUPPORTED | ERROR_INVALID_PARAMETER)
    )
}

fn error_code(error: &io::Error) -> Option<u32> {
    error
        .raw_os_error()
        .and_then(|code| u32::try_from(code).ok())
}

fn structure_size<T>() -> u32 {
    u32::try_from(size_of::<T>()).expect("Win32 information structures fit a DWORD size")
}

pub(crate) fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut name: Vec<u16> = path.as_os_str().encode_wide().collect();
    if name.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains NUL",
        ));
    }
    if name.len() >= 32767 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path exceeds Win32 extended path limit",
        ));
    }
    name.push(0);
    Ok(name)
}

pub(crate) fn windows_directory() -> io::Result<PathBuf> {
    let mut buffer = vec![0; 32768];
    let capacity = u32::try_from(buffer.len()).expect("Win32 path buffers fit a DWORD length");
    // SAFETY: buffer provides capacity writable UTF-16 elements and stays
    // exclusively accessible until the synchronous API returns.
    let len = unsafe { GetWindowsDirectoryW(buffer.as_mut_ptr(), capacity) } as usize;
    if len == 0 {
        return Err(io::Error::last_os_error());
    }
    if len >= buffer.len() {
        return Err(io::Error::other("Windows directory path is too long"));
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..len])))
}

/// Find handles have a distinct close function from file handles.
pub(crate) struct Finder {
    handle: HANDLE,
    first: Option<WIN32_FIND_DATAW>,
}

impl Finder {
    pub fn open(directory: &Path) -> io::Result<Option<Self>> {
        let pattern = wide(&directory.join("*"))?;
        let mut data = WIN32_FIND_DATAW::default();
        // SAFETY: pattern is live NUL-terminated UTF-16; data is aligned output
        // storage for FindExInfoBasic, and the unused search-filter is null.
        let handle = unsafe {
            FindFirstFileExW(
                pattern.as_ptr(),
                FindExInfoBasic,
                (&raw mut data).cast(),
                FindExSearchNameMatch,
                null(),
                0,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let error = io::Error::last_os_error();
            if error_code(&error) == Some(ERROR_FILE_NOT_FOUND) {
                return Ok(None);
            }
            return Err(error);
        }
        Ok(Some(Self {
            handle,
            first: Some(data),
        }))
    }

    pub fn next(&mut self) -> io::Result<Option<(OsString, bool)>> {
        loop {
            let data = if let Some(data) = self.first.take() {
                data
            } else {
                let mut data = WIN32_FIND_DATAW::default();
                // SAFETY: self owns a live find handle; data is exclusively
                // accessible output storage of the exact required structure.
                if unsafe { FindNextFileW(self.handle, &raw mut data) } == 0 {
                    let error = io::Error::last_os_error();
                    return if error_code(&error) == Some(ERROR_NO_MORE_FILES) {
                        Ok(None)
                    } else {
                        Err(error)
                    };
                }
                data
            };
            let end = data
                .cFileName
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(data.cFileName.len());
            let name = &data.cFileName[..end];
            if name == [46] || name == [46, 46] {
                continue;
            }
            let directory = data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0
                && data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0;
            return Ok(Some((OsString::from_wide(name), directory)));
        }
    }
}

impl Drop for Finder {
    fn drop(&mut self) {
        // SAFETY: self uniquely owns a valid find handle and this is its only
        // close operation; no method borrow remains when Drop executes.
        unsafe {
            FindClose(self.handle);
        }
    }
}
