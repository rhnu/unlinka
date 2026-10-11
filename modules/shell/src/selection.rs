//! Shared, all-or-nothing selection conversion; no filesystem traversal or link resolution.

use std::{
    ffi::OsString,
    os::windows::ffi::OsStringExt,
    path::{Component, Path, PathBuf},
};

use windows::{
    Win32::{
        Foundation::E_INVALIDARG,
        System::Com::CoTaskMemFree,
        UI::Shell::{IShellItemArray, SIGDN_FILESYSPATH},
    },
    core::{Error, PWSTR, Result},
};

const MAX_SELECTION: u32 = 65_536;
const MAX_PATH_UNITS: usize = 32_768;
const MAX_SELECTION_UNITS: usize = 16 * 1024 * 1024;

struct ShellName(PWSTR);

impl Drop for ShellName {
    fn drop(&mut self) {
        // SAFETY: GetDisplayName transfers a CoTaskMem allocation to the caller.
        unsafe { CoTaskMemFree(Some(self.0.0.cast())) };
    }
}

/// Reject volumes, shares, relative paths and lexical traversal before offering a command.
pub(crate) fn eligible_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .any(|part| matches!(part, Component::Normal(_)))
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
}

fn path_from_utf16(units: &[u16]) -> Result<PathBuf> {
    let path = PathBuf::from(OsString::from_wide(units));
    if !eligible_path(&path) {
        return Err(Error::from_hresult(E_INVALIDARG));
    }
    Ok(path)
}

pub(crate) fn copy_selection(items: &IShellItemArray) -> Result<Vec<PathBuf>> {
    // SAFETY: all interfaces belong to the current COM calling apartment.
    let count = unsafe { items.GetCount()? };
    if count == 0 || count > MAX_SELECTION {
        return Err(Error::from_hresult(E_INVALIDARG));
    }
    let mut paths = Vec::with_capacity(count as usize);
    let mut total_units = 0_usize;
    for index in 0..count {
        // SIGDN_FILESYSPATH fails for virtual entries and preserves the selected link itself.
        // SAFETY: the index is inside the reported array and the returned name is COM-owned.
        let item = unsafe { items.GetItemAt(index)? };
        // SAFETY: this live item returns a caller-owned terminated filesystem name.
        let name = ShellName(unsafe { item.GetDisplayName(SIGDN_FILESYSPATH)? });
        if name.0.is_null() {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        // Always free the name, including when conversion rejects an entry.
        let result = (|| {
            let mut length = 0;
            while length < MAX_PATH_UNITS {
                let current = name.0.0.wrapping_add(length);
                // SAFETY: Shell returns a terminated UTF-16 string; read only through its first NUL.
                if unsafe { *current } == 0 {
                    break;
                }
                length += 1;
            }
            if length == 0 || length == MAX_PATH_UNITS {
                return Err(Error::from_hresult(E_INVALIDARG));
            }
            total_units = total_units
                .checked_add(length)
                .filter(|total| *total <= MAX_SELECTION_UNITS)
                .ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
            // SAFETY: the preceding scan established the initialized string length.
            let units = unsafe { std::slice::from_raw_parts(name.0.0, length) };
            path_from_utf16(units)
        })();
        paths.push(result?);
    }
    Ok(paths)
}

#[cfg(test)]
mod tests;
