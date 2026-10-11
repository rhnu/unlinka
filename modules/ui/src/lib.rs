//! Native Windows UI shared by desktop tasks, maintenance, and Shell diagnostics.
#![cfg(windows)]

use std::io;

use winsafe::{HWND, co, prelude::*};

#[cfg(feature = "dialogs")]
mod details;
#[cfg(feature = "dialogs")]
mod dialog;
#[cfg(feature = "dialogs")]
mod messages;
#[cfg(feature = "dialogs")]
mod progress;

#[cfg(feature = "dialogs")]
pub use dialog::{CANCEL, Details, Dialog, DialogResult, Verification};
#[cfg(feature = "dialogs")]
pub use progress::{ProgressDialog, ProgressEvent, ProgressUpdate};

/// Shows a basic diagnostic without requiring Common Controls v6 in the host.
///
/// # Errors
/// Returns the Windows error if the message box cannot be displayed.
pub fn message_box(title: &str, text: &str, error: bool) -> io::Result<()> {
    HWND::NULL
        .MessageBox(
            text,
            title,
            co::MB::OK
                | if error {
                    co::MB::ICONERROR
                } else {
                    co::MB::ICONINFORMATION
                },
        )
        .map(|_| ())
        .map_err(io::Error::other)
}
