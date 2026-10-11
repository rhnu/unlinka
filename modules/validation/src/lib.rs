//! Disposable fixtures and bounded process execution for destructive validation.

use std::{
    ffi::OsString,
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

/// A fresh, owned directory whose cleanup never follows reparse entries.
pub struct Fixture {
    root: PathBuf,
}

impl Fixture {
    /// Create a unique disposable directory under the workspace scratch folder.
    ///
    /// # Errors
    /// Returns an error for an invalid label, unsafe scratch link, clock error,
    /// or failure to create the owned directory.
    pub fn new(label: &str) -> io::Result<Self> {
        if label.is_empty()
            || !label
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid fixture label",
            ));
        }
        let scratch = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()?
            .join(".scratch");
        fs::create_dir_all(&scratch)?;
        if is_link(&fs::symlink_metadata(&scratch)?) {
            return Err(io::Error::other("scratch directory must not be a link"));
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = scratch.join(format!("{label}-{}-{stamp}-{sequence}", std::process::id()));
        fs::create_dir(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path { &self.root }

    /// Reject absolute/parent paths and existing links in parent components.
    ///
    /// # Errors
    /// Returns an error if the path escapes the fixture, traverses a link, or
    /// required ancestor metadata cannot be inspected.
    pub fn path(&self, relative: impl AsRef<Path>) -> io::Result<PathBuf> {
        let relative = relative.as_ref();
        let mut result = self.root.clone();
        let components: Vec<_> = relative.components().collect();
        for (index, component) in components.iter().enumerate() {
            match component {
                Component::Normal(name) => result.push(name),
                Component::CurDir => {},
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "path escapes fixture",
                    ));
                },
            }
            if index + 1 < components.len() {
                match fs::symlink_metadata(&result) {
                    Ok(metadata) if is_link(&metadata) => {
                        return Err(io::Error::other("fixture path traverses a link"));
                    },
                    Ok(_) => {},
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {},
                    Err(error) => return Err(error),
                }
            }
        }
        if is_link(&fs::symlink_metadata(&self.root)?) {
            return Err(io::Error::other("fixture root was replaced by a link"));
        }
        Ok(result)
    }

    /// Create an ordinary directory within this fixture.
    ///
    /// # Errors
    /// Returns an error for an unsafe path, a link at the destination, or a
    /// filesystem directory-creation failure.
    pub fn folder(&self, relative: impl AsRef<Path>) -> io::Result<PathBuf> {
        let path = self.path(relative)?;
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if is_link(&metadata) {
                return Err(io::Error::other("fixture folder is a link"));
            }
        }
        fs::create_dir_all(&path)?;
        Ok(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = cleanup(&self.root) {
            eprintln!(
                "warning: fixture cleanup failed at {}: {error}",
                self.root.display()
            );
        }
    }
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn cleanup(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if is_link(&metadata) {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes()
                & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY
                != 0
            {
                return fs::remove_dir(path);
            }
        }
        return fs::remove_file(path);
    }
    #[cfg(windows)]
    if metadata.permissions().readonly() {
        use std::os::windows::{ffi::OsStrExt, fs::MetadataExt};

        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_READONLY, SetFileAttributesW,
        };
        let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: the owned fixture path is terminated UTF-16 and stays live;
        // only the read-only bit is cleared, preserving all other attributes.
        if unsafe {
            SetFileAttributesW(
                name.as_ptr(),
                metadata.file_attributes() & !FILE_ATTRIBUTE_READONLY,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            cleanup(&entry?.path())?;
        }
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    }
}

/// Run without a visible window, draining both pipes, and kill/reap on timeout.
///
/// # Errors
/// Returns process spawn, capture, or wait failures, and a timeout error after
/// 120 seconds. A nonzero process exit is returned normally in `Output`.
///
/// # Panics
/// Panics if the standard process API fails to provide requested piped handles.
pub fn run(binary: &Path, args: &[OsString], cwd: &Path) -> io::Result<Output> {
    let mut command = Command::new(binary);
    command.args(args).current_dir(cwd);
    run_command(&mut command)
}

/// Like [`run`], retaining caller-supplied arguments, environment, and cwd.
///
/// # Errors
/// Returns process spawn, capture, or wait failures, and a timeout error after
/// 120 seconds. A nonzero process exit is returned normally in `Output`.
///
/// # Panics
/// Panics if the standard process API fails to provide requested piped handles.
pub fn run_command(command: &mut Command) -> io::Result<Output> {
    command
        .env("UNLINKA_LANG", "en-US")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let read = |mut pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).map(|_| bytes)
        })
    };
    let stdout = read(Box::new(stdout));
    let stderr = read(Box::new(stderr));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < Duration::from_secs(120) => {
                thread::sleep(Duration::from_millis(10));
            },
            Ok(None) => {
                stop(&mut child);
                break Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "validation child exceeded 120 seconds",
                ));
            },
            Err(error) => {
                stop(&mut child);
                break Err(error);
            },
        }
    };
    let stdout = stdout
        .join()
        .map_err(|_panic| io::Error::other("stdout reader panicked"))??;
    let stderr = stderr
        .join()
        .map_err(|_panic| io::Error::other("stderr reader panicked"))??;
    Ok(Output {
        status: status?,
        stdout,
        stderr,
    })
}

fn stop(child: &mut std::process::Child) {
    if let Err(error) = child.kill() {
        eprintln!("warning: could not kill validation child: {error}");
    }
    if let Err(error) = child.wait() {
        eprintln!("warning: could not reap validation child: {error}");
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Counts {
    pub files: u64,
    pub directories: u64,
    pub links: u64,
    pub failed: u64,
}

/// Reads the versioned JSON result without depending on human-facing wording.
///
/// # Errors
/// Returns an error for a missing or unsupported JSON result or invalid count fields.
pub fn counts(text: &str) -> io::Result<Counts> {
    let line = text
        .lines()
        .find(|line| line.starts_with('{'))
        .ok_or_else(|| io::Error::other("CLI JSON result missing"))?;
    let document: serde_json::Value = serde_json::from_str(line).map_err(io::Error::other)?;
    if document["schema_version"].as_u64() != Some(1) {
        return Err(io::Error::other("unsupported CLI result schema"));
    }
    let count = |name| {
        document[name]
            .as_u64()
            .ok_or_else(|| io::Error::other(format!("invalid CLI count: {name}")))
    };
    Ok(Counts {
        files: count("files")?,
        directories: count("directories")?,
        links: count("links")?,
        failed: count("failed")?,
    })
}

#[cfg(test)]
mod tests;
