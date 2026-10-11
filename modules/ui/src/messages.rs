//! `WinSafe` has no typed TDM messages; keep their pointer-sensitive sends here.
use windows_sys::Win32::UI::Controls::{
    TDE_CONTENT, TDE_MAIN_INSTRUCTION, TDM_CLICK_BUTTON, TDM_ENABLE_BUTTON, TDM_SET_ELEMENT_TEXT,
    TDM_SET_PROGRESS_BAR_MARQUEE,
};
use winsafe::{HWND, WString, co, msg};

use crate::CANCEL;

/// Only live `TaskDialog` callback handles reach these private operations.
fn message(id: i32, wparam: usize, lparam: isize) -> msg::Wm {
    // SAFETY: These IDs come from Microsoft's task-dialog constants.
    msg::Wm::new(
        unsafe {
            co::WM::from_raw(u32::try_from(id).expect("task-dialog message IDs are positive"))
        },
        wparam,
        lparam,
    )
}

pub(super) fn close(hwnd: &HWND) {
    let close = message(TDM_CLICK_BUTTON, usize::from(CANCEL), 0);
    // SAFETY: The live dialog is notified asynchronously with a button ID, no pointers.
    if unsafe { hwnd.PostMessage(close) }.is_err() {
        // SAFETY: If posting fails, synchronous delivery uses the same live handle and ID.
        unsafe { hwnd.SendMessage(close) };
    }
}

pub(super) fn cancel_enabled(hwnd: &HWND, enabled: bool) {
    // SAFETY: This live dialog message contains only a button ID and boolean.
    unsafe {
        hwnd.SendMessage(message(
            TDM_ENABLE_BUTTON,
            usize::from(CANCEL),
            isize::from(enabled),
        ))
    };
}

pub(super) fn marquee(hwnd: &HWND) {
    // SAFETY: This live dialog message contains only an enable flag and default speed.
    unsafe { hwnd.SendMessage(message(TDM_SET_PROGRESS_BAR_MARQUEE, 1, 0)) };
}

pub(super) fn content(hwnd: &HWND, text: &str) { set_text(hwnd, TDE_CONTENT, text); }

pub(super) fn instruction(hwnd: &HWND, text: &str) { set_text(hwnd, TDE_MAIN_INSTRUCTION, text); }

fn set_text(hwnd: &HWND, element: i32, text: &str) {
    let text = WString::from_str(text);
    // SAFETY: The live dialog copies this NUL-terminated buffer synchronously before it drops.
    unsafe {
        hwnd.SendMessage(message(
            TDM_SET_ELEMENT_TEXT,
            usize::try_from(element).expect("task-dialog element IDs are positive"),
            text.as_ptr() as isize,
        ))
    };
}
