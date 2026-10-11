//! Shared, concise human-readable output for the terminal and native dialogs.
use std::{io, path::Path, time::Duration};

use crate::Translator;

/// Removes Win32's extended-path prefix without shortening the actual path.
pub fn path_text(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
    }
}

/// Translates known domain and Windows errors; unknown errors retain their detail.
pub fn diagnostic(tr: &Translator, code: &str, error: &io::Error) -> String {
    if code != "system" {
        return tr.text(&format!("diagnostic-{code}"));
    }
    let message = match error.raw_os_error() {
        Some(2 | 3) => "diagnostic-not-found",
        Some(5) => "diagnostic-access-denied",
        Some(19) => "diagnostic-write-protected",
        Some(32 | 33) => "diagnostic-in-use",
        Some(145) => "diagnostic-not-empty",
        _ => return error.to_string(),
    };
    tr.text(message)
}

/// Cancellation takes precedence, while failures must never look like success.
pub fn outcome(tr: &Translator, dry_run: bool, cancelled: bool, failed: u64) -> String {
    let message = match (dry_run, cancelled, failed > 0) {
        (true, true, _) => "scan-cancelled",
        (false, true, _) => "delete-cancelled",
        (true, false, true) => "scan-incomplete",
        (false, false, true) => "delete-incomplete",
        (true, false, false) => "scan-complete",
        (false, false, false) => "delete-complete",
    };
    tr.text(message)
}

pub fn counts(
    tr: &Translator,
    dry_run: bool,
    files: u64,
    directories: u64,
    links: u64,
    elapsed: Duration,
) -> String {
    tr.format(
        if dry_run {
            "report-scanned"
        } else {
            "report-deleted"
        },
        &[
            ("files", files.into()),
            ("directories", directories.into()),
            ("links", links.into()),
            ("seconds", format!("{:.2}", elapsed.as_secs_f64()).into()),
        ],
    )
}

#[cfg(test)]
mod tests;
