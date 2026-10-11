use std::{
    collections::HashSet,
    env, fs, io,
    os::windows::ffi::OsStrExt,
    path::{Component, Path, PathBuf, Prefix},
};

use crate::{
    PreparedTargets,
    win::{Handle, Identity, Kind},
};

pub(crate) struct PreparedEntry {
    pub path: PathBuf,
    pub identity: Identity,
    // Keeping the object alive prevents file ID reuse across confirmation.
    pub snapshot: Handle,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

/// Reject special namespaces and stream syntax before any filesystem lookup.
pub(crate) fn validate_syntax(path: &Path) -> io::Result<()> {
    let units: Vec<_> = path.as_os_str().encode_wide().collect();
    if units.is_empty() || units.contains(&0) {
        return Err(invalid("empty path or NUL in path"));
    }
    let text = path.to_string_lossy().replace('/', "\\");
    let lower = text.to_lowercase();
    if lower.starts_with("\\\\.\\") || lower.starts_with("\\??\\") {
        return Err(invalid("device namespaces are not allowed"));
    }
    if lower.starts_with("\\\\?\\") {
        let suffix = &text[4..];
        let disk = suffix.as_bytes().get(1) == Some(&b':')
            && suffix
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
            && suffix.as_bytes().get(2) == Some(&b'\\');
        if !disk && !suffix.to_lowercase().starts_with("unc\\") {
            return Err(invalid("only drive and UNC extended paths are allowed"));
        }
    }
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Disk(_) | Prefix::VerbatimDisk(_) => {},
                Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share)
                    if !server
                        .encode_wide()
                        .chain(share.encode_wide())
                        .any(|unit| unit == u16::from(b':')) => {},
                _ => return Err(invalid("device namespaces are not allowed")),
            },
            Component::Normal(name) if name.encode_wide().any(|unit| unit == u16::from(b':')) => {
                return Err(invalid("alternate data streams are not allowed"));
            },
            _ => {},
        }
    }
    if let Some(Component::Prefix(prefix)) = path.components().next() {
        if matches!(prefix.kind(), Prefix::Disk(_)) && !path.has_root() {
            return Err(invalid("drive-relative paths are not allowed"));
        }
    }
    Ok(())
}

/// Keys are used conservatively for protection only, never to omit targets.
pub(crate) fn path_key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\").to_lowercase();
    let text = if let Some(unc) = text.strip_prefix("\\\\?\\unc\\") {
        format!("\\\\{unc}")
    } else {
        text.strip_prefix("\\\\?\\").unwrap_or(&text).to_owned()
    };
    text.trim_end_matches('\\').to_owned()
}

pub(crate) fn contains_path(parent: &Path, child: &Path) -> bool {
    let parent = path_key(parent);
    let child = path_key(child);
    child == parent
        || child
            .strip_prefix(&parent)
            .is_some_and(|suffix| suffix.starts_with('\\'))
}

pub(crate) fn redundant(root: &Path, kind: Kind, candidate: &Path) -> bool {
    // Preserve exact OS strings for case-sensitive NTFS and unpaired UTF-16.
    root == candidate || (kind == Kind::Directory && candidate.starts_with(root))
}

pub(crate) struct Protection {
    windows_root: PathBuf,
    paths: Vec<PathBuf>,
    identities: Vec<Identity>,
}

impl Protection {
    pub fn collect() -> io::Result<Self> {
        let windows_root = fs::canonicalize(crate::win::windows_directory()?)?;
        let mut protected = vec![env::current_dir()?, windows_root.clone()];
        let profile = env::var_os("USERPROFILE")
            .ok_or_else(|| invalid("USERPROFILE is unavailable; cannot protect the user root"))?;
        protected.push(PathBuf::from(profile));
        let mut paths = Vec::new();
        let mut identities = Vec::new();
        let mut seen = HashSet::new();
        for path in protected {
            let physical = fs::canonicalize(path)?;
            for ancestor in physical.ancestors() {
                if !seen.insert(path_key(ancestor)) {
                    continue;
                }
                let handle = Handle::open(ancestor, false, false)?;
                identities.push(handle.identity()?);
                paths.push(handle.final_path()?);
            }
        }
        Ok(Self {
            windows_root,
            paths,
            identities,
        })
    }

    pub fn check(&self, path: &Path, identity: Identity) -> io::Result<()> {
        if contains_path(&self.windows_root, path)
            || self
                .paths
                .iter()
                .any(|protected| path_key(protected) == path_key(path))
            || self.identities.contains(&identity)
        {
            return Err(invalid(format!(
                "protected directory or ancestor: {}",
                path.display()
            )));
        }
        Ok(())
    }
}

/// Lock each physical ancestor from the volume/share root downward. Reparse
/// ancestors are refused rather than followed during the path-based enumeration.
pub(crate) fn pin_ancestors(path: &Path) -> io::Result<Vec<Handle>> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("cannot select a filesystem root"))?;
    let mut ancestors: Vec<_> = parent.ancestors().collect();
    ancestors.reverse();
    let mut pins = Vec::with_capacity(ancestors.len());
    for ancestor in ancestors {
        let pin = Handle::open(ancestor, false, true)?;
        if pin.identity()?.kind != Kind::Directory {
            return Err(invalid(format!(
                "ancestor is not an ordinary directory: {}",
                ancestor.display()
            )));
        }
        pins.push(pin);
    }
    Ok(pins)
}

pub(crate) fn prepare(paths: &[PathBuf], recursive: bool) -> io::Result<PreparedTargets> {
    if paths.is_empty() {
        return Err(invalid("at least one target is required"));
    }
    let protection = Protection::collect()?;
    let mut entries = Vec::with_capacity(paths.len());
    for input in paths {
        validate_syntax(input)?;
        let absolute = std::path::absolute(input)?;
        let name = absolute
            .file_name()
            .ok_or_else(|| invalid("cannot select a filesystem root"))?;
        let parent = absolute
            .parent()
            .ok_or_else(|| invalid("target has no parent"))?;
        // Resolve aliases in the parent only: resolving the final entry would
        // turn a symlink deletion into a deletion of its destination.
        let physical = fs::canonicalize(parent)?.join(name);
        let _pins = pin_ancestors(&physical)?;
        let snapshot = Handle::open(&physical, false, false)?;
        let identity = snapshot.identity()?;
        let path = snapshot.final_path()?;
        validate_syntax(&path)?;
        protection.check(&path, identity)?;
        if identity.kind == Kind::Directory && !recursive {
            return Err(invalid(format!(
                "ordinary directory requires recursive mode: {}",
                path.display()
            )));
        }
        entries.push(PreparedEntry {
            path,
            identity,
            snapshot,
        });
    }
    // Validation above covers even inputs that are redundant with another input.
    // Parent directories come first so every descendant is suppressed once.
    entries.sort_by_key(|entry| entry.path.components().count());
    let mut selected: Vec<PreparedEntry> = Vec::new();
    for entry in entries {
        if selected
            .iter()
            .any(|root| redundant(&root.path, root.identity.kind, &entry.path))
        {
            continue;
        }
        selected.push(entry);
    }
    Ok(PreparedTargets {
        paths: selected.iter().map(|entry| entry.path.clone()).collect(),
        entries: selected,
    })
}
