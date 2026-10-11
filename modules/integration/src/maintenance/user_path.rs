use super::{
    OsStrExt, OsString, OsStringExt, Path, PathChange, Transaction, Value, invalid, io, pcw,
    registry,
};

pub(super) fn path_tokens(value: &Value) -> io::Result<Vec<Vec<u16>>> {
    Ok(value
        .os_string()?
        .encode_wide()
        .collect::<Vec<_>>()
        .split(|c| *c == u16::from(b';'))
        .map(<[u16]>::to_vec)
        .collect())
}

pub(super) fn path_value(tokens: &[Vec<u16>], kind: u32) -> Value {
    let chars: Vec<u16> = tokens
        .iter()
        .enumerate()
        .flat_map(|(i, token)| {
            std::iter::once(u16::from(b';'))
                .take(usize::from(i != 0))
                .chain(token.iter().copied())
        })
        .collect();
    let mut value = Value::string(OsString::from_wide(&chars));
    value.kind = kind;
    value
}

pub(super) fn normalized_entry(entry: &[u16]) -> io::Result<Vec<u16>> {
    use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
    let mut entry = entry.to_vec();
    while entry.first() == Some(&u16::from(b' ')) {
        entry.remove(0);
    }
    while entry.last() == Some(&u16::from(b' ')) {
        entry.pop();
    }
    if entry.first() == Some(&u16::from(b'"'))
        && entry.last() == Some(&u16::from(b'"'))
        && entry.len() > 1
    {
        entry = entry[1..entry.len() - 1].to_vec();
    }
    entry.push(0);
    // SAFETY: input is terminated and output capacity is queried before the expansion.
    let length = unsafe { ExpandEnvironmentStringsW(pcw(&entry), None) };
    if length == 0 || length > 32768 {
        return Err(invalid("environment path expansion failed"));
    }
    let mut result = vec![0; length as usize];
    // SAFETY: result has exactly the queried capacity; the terminated source stays live.
    if unsafe { ExpandEnvironmentStringsW(pcw(&entry), Some(&mut result)) } != length {
        return Err(invalid("environment changed during path expansion"));
    }
    result.pop();
    for unit in &mut result {
        if *unit == u16::from(b'/') {
            *unit = u16::from(b'\\');
        }
    }
    while result.last() == Some(&u16::from(b'\\')) && result.len() > 3 {
        result.pop();
    }
    Ok(result)
}

pub(super) fn equivalent(left: &[u16], right: &[u16]) -> io::Result<bool> {
    use windows::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
    let left = normalized_entry(left)?;
    let right = normalized_entry(right)?;
    // SAFETY: API receives bounded UTF-16 slices and does not mutate them.
    Ok(unsafe { CompareStringOrdinal(&left, &right, true) } == CSTR_EQUAL)
}

pub(super) fn path_present(root: &Path) -> io::Result<bool> {
    let Some(value) = registry::read("Environment", "Path")? else {
        return Ok(false);
    };
    let bin = root
        .join("bin")
        .as_os_str()
        .encode_wide()
        .collect::<Vec<_>>();
    for entry in path_tokens(&value)? {
        if equivalent(&entry, &bin)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn edit_path(root: &Path, tx: &mut Transaction, enabled: bool) -> io::Result<()> {
    let before = registry::read("Environment", "Path")?;
    let value = before.clone().unwrap_or_else(|| Value::string(""));
    let bin = root
        .join("bin")
        .as_os_str()
        .encode_wide()
        .collect::<Vec<_>>();
    let (tokens, ownership, change) = plan_path(
        path_tokens(&value)?,
        &bin,
        tx.after.path_owned.as_deref(),
        enabled,
        equivalent,
    )?;
    let changed = change.is_some();
    if let Some((entry, added)) = change {
        tx.path.push(PathChange {
            entry,
            added,
            original_missing: before.is_none(),
        });
    }
    tx.after.path_owned = ownership;
    tx.save(root)?;
    if !changed {
        return Ok(());
    }
    if registry::read("Environment", "Path")? != before {
        return Err(invalid("user PATH changed during maintenance; retry"));
    }
    registry::write(
        "Environment",
        "Path",
        Some(&path_value(&tokens, value.kind)),
    )?;
    tx.save(root)
}

type PathPlan = (Vec<Vec<u16>>, Option<Vec<u16>>, Option<(Vec<u16>, bool)>);

pub(super) fn plan_path<F>(
    mut tokens: Vec<Vec<u16>>,
    bin: &[u16],
    owned: Option<&[u16]>,
    enabled: bool,
    equivalent: F,
) -> io::Result<PathPlan>
where
    F: Fn(&[u16], &[u16]) -> io::Result<bool>,
{
    if enabled {
        for token in &tokens {
            if equivalent(token, bin)? {
                return Ok((tokens, owned.map(<[u16]>::to_vec), None));
            }
        }
        if tokens.len() == 1 && tokens[0].is_empty() {
            tokens.clear();
        }
        tokens.push(bin.to_vec());
        Ok((tokens, Some(bin.to_vec()), Some((bin.to_vec(), true))))
    } else if let Some(owned) = owned {
        if let Some(index) = tokens.iter().position(|p| p == owned) {
            tokens.remove(index);
            Ok((tokens, None, Some((owned.to_vec(), false))))
        } else {
            for token in &tokens {
                if equivalent(token, owned)? {
                    return Err(invalid(
                        "owned PATH entry was changed externally; it was preserved",
                    ));
                }
            }
            Ok((tokens, None, None))
        }
    } else {
        Ok((tokens, None, None))
    }
}

pub(super) fn rollback_path(change: &PathChange) -> io::Result<()> {
    let value = registry::read("Environment", "Path")?.unwrap_or_else(|| Value::string(""));
    let mut tokens = path_tokens(&value)?;
    let before_tokens = tokens.clone();
    reverse_path_tokens(&mut tokens, &change.entry, change.added);
    if tokens == before_tokens {
        return Ok(());
    }
    let after = if change.added && change.original_missing && tokens.is_empty() {
        None
    } else {
        Some(path_value(&tokens, value.kind))
    };
    registry::write("Environment", "Path", after.as_ref())
}

pub(super) fn reverse_path_tokens(tokens: &mut Vec<Vec<u16>>, entry: &[u16], added: bool) {
    if added {
        if let Some(index) = tokens.iter().position(|p| p == entry) {
            tokens.remove(index);
        }
    } else if !tokens.iter().any(|p| p == entry) {
        if tokens.len() == 1 && tokens[0].is_empty() {
            tokens.clear();
        }
        tokens.push(entry.to_vec());
    }
}

pub(super) fn compensation(
    current: Option<&Value>,
    before: Option<&Value>,
    after: Option<&Value>,
) -> io::Result<Option<Value>> {
    if current == before || current == after {
        Ok(before.cloned())
    } else {
        Err(invalid("registry changed externally during rollback"))
    }
}
