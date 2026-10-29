//! Acquire the pinned, portable Inno compiler without installing host tools.
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt::{self, Write as _},
    fs::{self, File, Metadata},
    io::{self, Cursor, Read, Write},
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
    time::Duration,
};

use sha2::{Digest, Sha256};
use zip::ZipArchive;

const VERSION: &str = "7.1.0";
const ARCHIVE_URL: &str =
    "https://api.nuget.org/v3-flatcontainer/tools.innosetup/7.1.0/tools.innosetup.7.1.0.nupkg";
// Tools.InnoSetup is an independently redistributed package, pinned after checking
// the original Pyrsys executable signatures and a real English/Chinese compile.
const ARCHIVE_SHA256: &str = "aad15c662593c6f5656457a5c001f4591babd72820cacf63f9706d4af5d93b75";
const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
struct Fingerprint {
    bytes: u64,
    sha256: [u8; 32],
}

/// Validate both the source archive and every cached tool before returning ISCC.
/// The exclusive cache lock prevents simultaneous pack commands from seeing partial writes.
pub(super) fn acquire(root: &Path) -> io::Result<PathBuf> {
    let tools = root.join("target/tools");
    acquire_in(&tools).map_err(|source| {
        io::Error::new(
            source.kind(),
            CompilerError {
                directory: tools,
                source,
            },
        )
    })
}

fn acquire_in(tools: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(tools)?;
    let lock_path = tools.join(format!("inno-{VERSION}.lock"));
    let lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    eprintln!("Inno Setup {VERSION}: waiting for compiler cache lock");
    lock.lock()?;
    // Dropping this handle releases the OS lock even when preparation fails.
    let archive_path = tools.join(format!("inno-{VERSION}.nupkg"));
    let archive_bytes = if let Some(bytes) = cached_archive(&archive_path)? {
        bytes
    } else {
        eprintln!("Inno Setup {VERSION}: downloading {ARCHIVE_URL}");
        let bytes = download()?;
        verify_archive(&bytes)?;
        replace_archive(&archive_path, &bytes)?;
        bytes
    };
    // This inventory is always derived from authenticated archive bytes. A local
    // manifest beside a modified executable would not provide the same guarantee.
    let inventory = inventory(&archive_bytes)?;
    let directory = tools.join(format!("inno-{VERSION}"));
    if cache_valid(&directory, &inventory)? {
        eprintln!(
            "Inno Setup {VERSION}: using verified compiler cache {}",
            directory.display()
        );
    } else {
        eprintln!(
            "Inno Setup {VERSION}: rebuilding missing or damaged compiler cache {}",
            directory.display()
        );
        remove_entry(&directory)?;
        fs::create_dir(&directory)?;
        extract(&archive_bytes, &directory)?;
        if !cache_valid(&directory, &inventory)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "extracted Inno compiler failed verification",
            ));
        }
    }
    Ok(directory.join("ISCC.exe"))
}

fn cached_archive(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.is_file() && !is_reparse_point(&metadata) && metadata.len() <= MAX_ARCHIVE_BYTES {
        let mut bytes = Vec::new();
        File::open(path)?
            .take(MAX_ARCHIVE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if verify_archive(&bytes).is_ok() {
            return Ok(Some(bytes));
        }
    }
    eprintln!(
        "Inno Setup {VERSION}: discarding invalid archive cache {}",
        path.display()
    );
    Ok(None)
}

fn download() -> io::Result<Vec<u8>> {
    let config = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(Duration::from_secs(180)))
        .timeout_connect(Some(Duration::from_secs(30)))
        .max_redirects(3)
        .build();
    ureq::Agent::new_with_config(config)
        .get(ARCHIVE_URL)
        .call()
        .map_err(io::Error::other)?
        .body_mut()
        .with_config()
        .limit(MAX_ARCHIVE_BYTES)
        .read_to_vec()
        .map_err(io::Error::other)
}

fn verify_archive(bytes: &[u8]) -> io::Result<()> {
    let actual = Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            write!(text, "{byte:02x}").expect("writing a hash to a String cannot fail");
            text
        });
    if actual != ARCHIVE_SHA256 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Inno archive SHA256 mismatch: expected {ARCHIVE_SHA256}, received {actual}"),
        ));
    }
    Ok(())
}

fn replace_archive(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("download");
    remove_entry(&temporary)?;
    let mut file = File::create_new(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    remove_entry(path)?;
    fs::rename(temporary, path)
}

/// Restrict ZIP names to ordinary relative tool paths, including on Windows.
fn tool_path(name: &str) -> io::Result<Option<PathBuf>> {
    let Some(relative) = name.strip_prefix("tools/") else {
        return Ok(None);
    };
    if relative.is_empty()
        || relative.split('/').any(|part| {
            part.is_empty() || part == "." || part == ".." || part.contains(['\\', ':', '\0'])
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid compiler archive path {name:?}"),
        ));
    }
    Ok(Some(PathBuf::from(relative)))
}

fn inventory(bytes: &[u8]) -> io::Result<BTreeMap<PathBuf, Fingerprint>> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut files = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name()?.into_owned();
        let Some(path) = tool_path(&name)? else {
            continue;
        };
        if entry.is_dir()
            || entry.is_symlink()
            || files.insert(path, fingerprint(&mut entry)?).is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported or duplicate compiler archive entry {name:?}"),
            ));
        }
    }
    if !files.contains_key(Path::new("ISCC.exe")) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "compiler archive contains no ISCC.exe",
        ));
    }
    Ok(files)
}

fn extract(bytes: &[u8], directory: &Path) -> io::Result<()> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(path) = tool_path(&entry.name()?)? else {
            continue;
        };
        let destination = directory.join(path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        io::copy(&mut entry, &mut File::create_new(destination)?)?;
    }
    Ok(())
}

/// Reject modified, missing and extra entries, including unexpected DLLs and links.
fn cache_valid(directory: &Path, expected: &BTreeMap<PathBuf, Fingerprint>) -> io::Result<bool> {
    let metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !metadata.is_dir() || is_reparse_point(&metadata) {
        return Ok(false);
    }
    let mut pending = vec![PathBuf::new()];
    let mut seen = BTreeSet::new();
    while let Some(parent) = pending.pop() {
        for entry in fs::read_dir(directory.join(&parent))? {
            let entry = entry?;
            let relative = parent.join(entry.file_name());
            let metadata = fs::symlink_metadata(entry.path())?;
            if is_reparse_point(&metadata) {
                return Ok(false);
            }
            if metadata.is_dir() {
                if !expected.keys().any(|path| path.starts_with(&relative)) {
                    return Ok(false);
                }
                pending.push(relative);
            } else if metadata.is_file() {
                let Some(expected) = expected.get(&relative) else {
                    return Ok(false);
                };
                if metadata.len() != expected.bytes
                    || fingerprint(File::open(entry.path())?)? != *expected
                {
                    return Ok(false);
                }
                seen.insert(relative);
            } else {
                return Ok(false);
            }
        }
    }
    Ok(seen.len() == expected.len())
}

fn fingerprint(mut reader: impl Read) -> io::Result<Fingerprint> {
    let mut sha256 = Sha256::new();
    let mut bytes = 0;
    let mut buffer = [0; 16 * 1024];
    loop {
        let length = reader.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        sha256.update(&buffer[..length]);
        bytes += length as u64;
    }
    Ok(Fingerprint {
        bytes,
        sha256: sha256.finalize().into(),
    })
}

fn is_reparse_point(metadata: &Metadata) -> bool {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// Only called with the fixed versioned paths inside the workspace tool cache.
fn remove_entry(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[derive(Debug)]
struct CompilerError {
    directory: PathBuf,
    source: io::Error,
}

impl fmt::Display for CompilerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "preparing Inno Setup {VERSION} in {}: {}",
            self.directory.display(),
            self.source
        )
    }
}

impl Error for CompilerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> { Some(&self.source) }
}

#[cfg(test)]
mod tests;
