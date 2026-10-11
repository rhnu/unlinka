use std::{ffi::OsString, io, os::windows::ffi::OsStringExt};

use serde::{Deserialize, Serialize};
use windows::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND},
    System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WOW64_64KEY, KEY_WRITE, REG_DWORD,
        REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW,
        RegDeleteKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryInfoKeyW, RegQueryValueExW,
        RegSetValueExW,
    },
};

use crate::{invalid, pcw, wide};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Value {
    pub kind: u32,
    pub data: Vec<u8>,
}

impl Value {
    pub fn string(value: impl AsRef<std::ffi::OsStr>) -> Self {
        Self {
            kind: REG_SZ.0,
            data: wide(value).iter().flat_map(|v| v.to_le_bytes()).collect(),
        }
    }

    pub fn number(value: u32) -> Self {
        Self {
            kind: REG_DWORD.0,
            data: value.to_le_bytes().to_vec(),
        }
    }

    pub fn os_string(&self) -> io::Result<OsString> {
        if !matches!(self.kind, 1 | 2) || self.data.len() % 2 != 0 {
            return Err(invalid("registry string has invalid type or encoding"));
        }
        let mut chars: Vec<u16> = self
            .data
            .chunks_exact(2)
            .map(|s| u16::from_le_bytes([s[0], s[1]]))
            .collect();
        if chars.pop() != Some(0) || chars.contains(&0) {
            return Err(invalid("registry string is not exactly terminated"));
        }
        Ok(OsString::from_wide(&chars))
    }

    pub fn dword(&self) -> io::Result<u32> {
        if self.kind != REG_DWORD.0 || self.data.len() != 4 {
            return Err(invalid("invalid registry DWORD"));
        }
        Ok(u32::from_le_bytes(
            self.data
                .as_slice()
                .try_into()
                .map_err(|error| invalid(format!("invalid DWORD: {error}")))?,
        ))
    }
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: this key is owned by the guard and closed exactly once.
        let _close_status = unsafe { RegCloseKey(self.0) };
    }
}

fn open(path: &str, create: bool) -> io::Result<Option<Key>> {
    let path = wide(path);
    let mut key = HKEY::default();
    let error = if create {
        // SAFETY: terminated path and output pointer remain live; HKCU 64-bit view is explicit.
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                pcw(&path),
                None,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE | KEY_WOW64_64KEY,
                None,
                &raw mut key,
                None,
            )
        }
    } else {
        // SAFETY: terminated path and output pointer remain live for opening the HKCU key.
        unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                pcw(&path),
                None,
                KEY_READ | KEY_WOW64_64KEY,
                &raw mut key,
            )
        }
    };
    if error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND {
        return Ok(None);
    }
    error.ok().map_err(crate::win_error)?;
    Ok(Some(Key(key)))
}

pub(crate) fn read(path: &str, name: &str) -> io::Result<Option<Value>> {
    let Some(key) = open(path, false)? else {
        return Ok(None);
    };
    let name = wide(name);
    let mut size = 0;
    let mut kind = REG_VALUE_TYPE::default();
    // SAFETY: terminated name and initialized output pointers remain live while sizing.
    let error = unsafe {
        RegQueryValueExW(
            key.0,
            pcw(&name),
            None,
            Some(&raw mut kind),
            None,
            Some(&raw mut size),
        )
    };
    if error == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    error.ok().map_err(crate::win_error)?;
    if size > 1024 * 1024 {
        return Err(invalid("oversized registry value"));
    }
    let mut data = vec![0; size as usize];
    // SAFETY: the data buffer has the queried capacity, supplied in size to bound the write.
    unsafe {
        RegQueryValueExW(
            key.0,
            pcw(&name),
            None,
            Some(&raw mut kind),
            Some(data.as_mut_ptr()),
            Some(&raw mut size),
        )
        .ok()
        .map_err(crate::win_error)?;
    }
    data.truncate(size as usize);
    Ok(Some(Value { kind: kind.0, data }))
}

pub(crate) fn read_string(path: &str, name: &str) -> io::Result<Option<String>> {
    read(path, name)?
        .map(|v| {
            v.os_string()?
                .into_string()
                .map_err(|value| invalid(format!("registry identity is not Unicode: {value:?}")))
        })
        .transpose()
}

pub(crate) fn write(path: &str, name: &str, value: Option<&Value>) -> io::Result<()> {
    if value.is_none() && open(path, false)?.is_none() {
        return Ok(());
    }
    let key = open(path, true)?.ok_or_else(|| invalid("registry key creation failed"))?;
    let name = wide(name);
    let error = match value {
        Some(v) => {
            // SAFETY: owned key, terminated name and byte slice remain live for the write.
            unsafe {
                RegSetValueExW(
                    key.0,
                    pcw(&name),
                    None,
                    REG_VALUE_TYPE(v.kind),
                    Some(&v.data),
                )
            }
        },
        None => {
            // SAFETY: the owned key and terminated value name remain live for deletion.
            unsafe { RegDeleteValueW(key.0, pcw(&name)) }
        },
    };
    if error == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    error.ok().map_err(crate::win_error)
}

/// Deletes only an empty key. Unknown values/subkeys prevent removal.
pub(crate) fn remove_empty(path: &str) {
    let Ok(Some(key)) = open(path, false) else {
        return;
    };
    let mut subkeys = 0;
    let mut values = 0;
    // SAFETY: the key is live and both initialized output counters remain writable.
    let error = unsafe {
        RegQueryInfoKeyW(
            key.0,
            None,
            None,
            None,
            Some(&raw mut subkeys),
            None,
            None,
            Some(&raw mut values),
            None,
            None,
            None,
            None,
        )
    };
    if error.is_err() || subkeys != 0 || values != 0 {
        return;
    }
    drop(key);
    let path = wide(path);
    // SAFETY: the terminated path identifies the already verified empty HKCU 64-bit key.
    let _delete_status =
        unsafe { RegDeleteKeyExW(HKEY_CURRENT_USER, pcw(&path), KEY_WOW64_64KEY.0, None) };
}
