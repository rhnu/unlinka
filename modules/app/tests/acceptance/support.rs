use std::{
    fs, io,
    os::windows::{ffi::OsStrExt, fs::MetadataExt},
    path::Path,
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, OPEN_EXISTING,
    },
    System::{IO::DeviceIoControl, Ioctl::FSCTL_SET_REPARSE_POINT},
};

pub fn readonly(path: &Path) {
    let mut permissions = fs::metadata(path)
        .expect("read fixture attributes")
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).expect("set fixture read-only attribute");
}

pub fn snapshot(root: &Path) -> io::Result<Vec<String>> {
    fn visit(path: &Path, result: &mut Vec<String>) -> io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        let attributes = metadata.file_attributes();
        if attributes & 0x400 != 0 {
            result.push(format!(
                "L|{}|{attributes}|{}",
                path.display(),
                fs::read_link(path)?.display()
            ));
        } else if metadata.is_dir() {
            result.push(format!("D|{}|{attributes}", path.display()));
            for entry in fs::read_dir(path)? {
                visit(&entry?.path(), result)?;
            }
        } else {
            result.push(format!(
                "F|{}|{attributes}|{}|{}|{:?}",
                path.display(),
                metadata.file_size(),
                metadata.last_write_time(),
                fs::read(path)?
            ));
        }
        Ok(())
    }
    let mut result = Vec::new();
    visit(root, &mut result)?;
    result.sort();
    Ok(result)
}

/// Create a mount-point reparse entry without shell commands or symlink privilege.
pub fn junction(link: &Path, target: &Path) -> io::Result<()> {
    fs::create_dir(link)?;
    let target = fs::canonicalize(target)?;
    let display = target.to_string_lossy();
    let display = display.strip_prefix(r"\\?\").unwrap_or(&display);
    let substitute: Vec<u16> = format!(r"\??\{display}").encode_utf16().collect();
    let print: Vec<u16> = display.encode_utf16().collect();
    let sub_bytes = u16::try_from(substitute.len() * 2).map_err(io::Error::other)?;
    let print_bytes = u16::try_from(print.len() * 2).map_err(io::Error::other)?;
    let mut buffer = Vec::new();
    buffer.extend_from_slice(&0xA000_0003_u32.to_le_bytes());
    buffer.extend_from_slice(&(8 + sub_bytes + 2 + print_bytes + 2).to_le_bytes());
    buffer.extend_from_slice(&0_u16.to_le_bytes());
    buffer.extend_from_slice(&0_u16.to_le_bytes());
    buffer.extend_from_slice(&sub_bytes.to_le_bytes());
    buffer.extend_from_slice(&(sub_bytes + 2).to_le_bytes());
    buffer.extend_from_slice(&print_bytes.to_le_bytes());
    for character in substitute.into_iter().chain([0]).chain(print).chain([0]) {
        buffer.extend_from_slice(&character.to_le_bytes());
    }
    let name: Vec<u16> = link.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: name is terminated UTF-16; null pointers request default security
    // and no template, and the returned handle is closed exactly once below.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            0x4000_0000,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let mut returned = 0;
    // SAFETY: buffer is the complete mount-point reparse layout with matching
    // byte length; the synchronous call retains neither buffer nor output.
    let success = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_SET_REPARSE_POINT,
            buffer.as_ptr().cast(),
            u32::try_from(buffer.len()).expect("reparse buffer fits a DWORD"),
            std::ptr::null_mut(),
            0,
            &raw mut returned,
            std::ptr::null_mut(),
        )
    };
    let result = if success == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    };
    // SAFETY: this is the sole close of the valid, locally owned handle.
    unsafe {
        CloseHandle(handle);
    }
    result
}
