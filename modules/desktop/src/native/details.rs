//! A bounded native text viewer keeps target and failure lists out of `TaskDialog` sizing.
use std::io;

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::{
            Controls::EM_SETLIMITTEXT,
            WindowsAndMessaging::{
                BS_DEFPUSHBUTTON, DLGTEMPLATE, DS_CENTER, DS_MODALFRAME, DS_SETFONT,
                DialogBoxIndirectParamW, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, EndDialog,
                GetDlgItem, IDCANCEL, SendMessageW, SetDlgItemTextW, WM_CLOSE, WM_COMMAND,
                WM_INITDIALOG, WS_BORDER, WS_CAPTION, WS_CHILD, WS_POPUP, WS_SYSMENU, WS_TABSTOP,
                WS_VISIBLE, WS_VSCROLL,
            },
        },
    },
    core::PCWSTR,
};

use super::wide;

const TEXT: u16 = 101;

pub(crate) struct Details<'a> {
    pub label: &'a str,
    pub title: &'a str,
    pub text: &'a str,
}

struct Viewer {
    text: Vec<u16>,
    error: Option<windows::core::Error>,
}

/// Modal ownership returns keyboard focus to the main dialog without changing its size.
pub(super) fn show(owner: HWND, details: &Details<'_>, close: &str) -> io::Result<()> {
    let template = template(details.title, close);
    let mut state = Viewer {
        text: wide(&details.text.replace('\n', "\r\n")),
        error: None,
    };
    // SAFETY: The DWORD-aligned template and text buffer live through the modal call.
    let result = unsafe {
        DialogBoxIndirectParamW(
            None,
            template.as_ptr().cast::<DLGTEMPLATE>(),
            Some(owner),
            Some(callback),
            LPARAM((&raw mut state) as isize),
        )
    };
    if let Some(error) = state.error {
        Err(io::Error::other(error))
    } else if result == IDCANCEL.0 as isize {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

unsafe extern "system" fn callback(
    hwnd: HWND,
    message: u32,
    button: WPARAM,
    data: LPARAM,
) -> isize {
    // A panic must not unwind through the Windows dialog procedure.
    std::panic::catch_unwind(|| {
        if message == WM_INITDIALOG {
            // SAFETY: data points to show's live state, used only while initializing the viewer.
            let state = unsafe { &mut *(data.0 as *mut Viewer) };
            // SAFETY: hwnd is the live dialog created from the template containing TEXT.
            let initialized = unsafe { GetDlgItem(Some(hwnd), i32::from(TEXT)) }.and_then(|edit| {
                // SAFETY: The edit control is live; this message contains no pointers.
                unsafe { SendMessageW(edit, EM_SETLIMITTEXT, Some(WPARAM(0)), None) };
                // SAFETY: The UTF-16 text remains live and is copied synchronously.
                unsafe { SetDlgItemTextW(hwnd, i32::from(TEXT), PCWSTR(state.text.as_ptr())) }
            });
            if let Err(error) = initialized {
                state.error = Some(error);
                // SAFETY: Initialization failed in this live modal viewer.
                unsafe { drop(EndDialog(hwnd, -1)) };
            }
            1 // Let the dialog manager focus the read-only text for scrolling and selection.
        } else if message == WM_CLOSE
            || (message == WM_COMMAND && button.0 & 0xFFFF == IDCANCEL.0 as usize)
        {
            // SAFETY: hwnd is the live modal viewer; Escape and Enter only close this viewer.
            unsafe { drop(EndDialog(hwnd, IDCANCEL.0 as isize)) };
            1
        } else {
            0
        }
    })
    .unwrap_or_else(|_| {
        // SAFETY: Fail closed if initialization or event handling panics.
        unsafe { drop(EndDialog(hwnd, -1)) };
        1
    })
}

/// Standard dialog units scale with the chosen font and the active display DPI.
fn template(title: &str, close: &str) -> Vec<u32> {
    let mut words = Vec::new();
    dword(
        &mut words,
        WS_POPUP.0 | WS_CAPTION.0 | WS_SYSMENU.0 | (DS_MODALFRAME | DS_SETFONT | DS_CENTER) as u32,
    );
    dword(&mut words, 0);
    words.extend([2, 0, 0, 400, 240, 0, 0]); // Controls, bounds, no menu, default class.
    words.extend(wide(title));
    words.push(9);
    words.extend(wide("Segoe UI"));
    item(
        &mut words,
        WS_CHILD.0
            | WS_VISIBLE.0
            | WS_TABSTOP.0
            | WS_BORDER.0
            | WS_VSCROLL.0
            | (ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY) as u32,
        [10, 10, 380, 190],
        TEXT,
        0x0081, // Standard EDIT class: wrapping, scrolling, selection and Ctrl+C.
        "",
    );
    item(
        &mut words,
        WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        [324, 212, 66, 18],
        u16::try_from(IDCANCEL.0).expect("Cancel's standard control ID fits u16"),
        0x0080, // Standard BUTTON class.
        close,
    );
    if words.len() % 2 != 0 {
        words.push(0);
    }
    // Vec<u32> guarantees the DWORD alignment required by DLGTEMPLATE and its items.
    words
        .chunks_exact(2)
        .map(|pair| u32::from(pair[0]) | (u32::from(pair[1]) << 16))
        .collect()
}

fn dword(words: &mut Vec<u16>, value: u32) {
    let bytes = value.to_le_bytes();
    words.extend([
        u16::from_le_bytes([bytes[0], bytes[1]]),
        u16::from_le_bytes([bytes[2], bytes[3]]),
    ]);
}

fn item(words: &mut Vec<u16>, style: u32, bounds: [u16; 4], id: u16, class: u16, text: &str) {
    if words.len() % 2 != 0 {
        words.push(0);
    }
    dword(words, style);
    dword(words, 0);
    words.extend(bounds);
    words.extend([id, 0xFFFF, class]);
    words.extend(wide(text));
    words.push(0); // No creation data.
}
