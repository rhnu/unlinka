//! Windows deletion with confirmation-time identity checks and pinned directories.
//!
//! The thread budget includes the scanning thread. Reparse entries are removed as
//! entries and never scanned. Directory pins reject conflicting write/delete
//! opens; this deliberately favors a failed operation over traversing a replaced
//! directory. Attribute-only reparse mutations are rechecked before child opens,
//! but path-based opens still have a check/open race against hostile filesystem
//! mutation; this is not a sandbox boundary. Files created after enumeration can
//! leave a directory nonempty; leaves replaced before opening may be removed at
//! the same authorized name. Kernel/filter drivers and remote servers can also
//! impose semantics beyond the local sharing guarantees.
#![cfg(windows)]

mod diagnostic;
mod engine;
mod paths;
mod win;

use std::{io, path::PathBuf, sync::atomic::AtomicBool, time::Duration};

pub use diagnostic::{DiagnosticCode, diagnostic_code};

/// Options shared by CLI and GUI callers.
#[derive(Debug, Clone)]
pub struct DeleteOptions {
    pub recursive: bool,
    pub threads: usize,
    pub dry_run: bool,
}

/// Validated targets, retaining identity handles until execution is ready.
pub struct PreparedTargets {
    paths: Vec<PathBuf>,
    entries: Vec<paths::PreparedEntry>,
}

impl PreparedTargets {
    /// Physical parent paths with the final entry preserved, for confirmation.
    pub fn paths(&self) -> &[PathBuf] { &self.paths }
}

/// One failed operation; the aggregate retains at most the first 100 failures.
#[derive(Debug)]
pub struct Failure {
    pub path: PathBuf,
    pub operation: &'static str,
    pub error: io::Error,
}

/// Successful removals (or candidates in dry-run) and accurate failure totals.
#[derive(Debug, Default)]
pub struct DeleteReport {
    pub files: u64,
    pub directories: u64,
    pub links: u64,
    pub failed: u64,
    pub failures: Vec<Failure>,
    pub cancelled: bool,
    pub elapsed: Duration,
}

/// A completed-operation snapshot; total work is intentionally unknown while scanning.
#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    pub files: u64,
    pub directories: u64,
    pub links: u64,
    pub failed: u64,
    pub elapsed: Duration,
}

impl DeleteReport {
    fn fail(&mut self, path: PathBuf, operation: &'static str, error: io::Error) {
        self.failed += 1;
        if self.failures.len() < 100 {
            self.failures.push(Failure {
                path,
                operation,
                error,
            });
        }
    }

    fn count(&mut self, kind: win::Kind) {
        match kind {
            win::Kind::File => self.files += 1,
            win::Kind::Directory => self.directories += 1,
            win::Kind::Link => self.links += 1,
        }
    }
}

/// Validate every input before returning the set that the caller will confirm.
///
/// # Errors
/// Returns an error for invalid, protected, missing, or inaccessible paths,
/// ordinary directories without recursive mode, and failed identity queries.
pub fn prepare(paths: &[PathBuf], recursive: bool) -> io::Result<PreparedTargets> {
    paths::prepare(paths, recursive)
}

/// Remove the confirmed targets, stopping unstarted work when cancelled.
pub fn execute(
    targets: PreparedTargets,
    options: &DeleteOptions,
    cancelled: &AtomicBool,
) -> DeleteReport {
    engine::execute(targets, options, cancelled, None)
}

/// Execute with coalesced progress callbacks, invoked without holding engine locks.
/// The callback runs on a monitor thread at most ten times per second and must
/// return promptly. The final report remains authoritative, including cancellation.
pub fn execute_with_progress(
    targets: PreparedTargets,
    options: &DeleteOptions,
    cancelled: &AtomicBool,
    observer: &(dyn Fn(Progress) + Sync),
) -> DeleteReport {
    engine::execute(targets, options, cancelled, Some(observer))
}

#[cfg(test)]
mod tests;
