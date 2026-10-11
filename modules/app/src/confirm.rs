use std::{
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
};

use unicode_segmentation::UnicodeSegmentation;
use unlinka_i18n::{FluentValue, Translator};
use windows_sys::Win32::UI::{
    Controls::{
        TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0, TD_WARNING_ICON,
        TDF_ALLOW_DIALOG_CANCELLATION, TDF_SIZE_TO_CONTENT, TaskDialogIndirect,
    },
    WindowsAndMessaging::IDCANCEL,
};

const DELETE_BUTTON: i32 = 1001;

/// Confirms permanent deletion using the system task dialog and a safe default.
pub fn native(paths: &[PathBuf], tr: &Translator) -> io::Result<bool> {
    let title = wide(&tr.text("confirm-title"));
    let instruction = if paths.len() == 1 {
        tr.format(
            "confirm-one",
            &[(
                "name",
                FluentValue::from(
                    paths[0]
                        .file_name()
                        .unwrap_or(paths[0].as_os_str())
                        .to_string_lossy()
                        .as_ref(),
                ),
            )],
        )
    } else {
        tr.count("confirm-many", paths.len() as u64)
    };
    let instruction = wide(&instruction);
    let mut content = tr.text("confirm-warning");
    if paths.len() == 1 {
        content.push_str("\n\n");
        content.push_str(&tr.text("location"));
        content.push('\n');
        content.push_str(&dialog_path(paths[0].parent().unwrap_or(&paths[0])));
    }
    let content = wide(&content);
    let details = wide(
        &paths
            .iter()
            .map(|path| dialog_path(path))
            .collect::<Vec<_>>()
            .join("\r\n\r\n"),
    );
    let expand = wide(&tr.text("show-paths"));
    let collapse = wide(&tr.text("hide-paths"));
    let delete_text = wide(&tr.text("button-delete"));
    let cancel_text = wide(&tr.text("button-cancel"));
    let buttons = [
        TASKDIALOG_BUTTON {
            nButtonID: DELETE_BUTTON,
            pszButtonText: delete_text.as_ptr(),
        },
        TASKDIALOG_BUTTON {
            nButtonID: IDCANCEL,
            pszButtonText: cancel_text.as_ptr(),
        },
    ];
    let config = TASKDIALOGCONFIG {
        cbSize: u32::try_from(std::mem::size_of::<TASKDIALOGCONFIG>())
            .expect("the task dialog config has a fixed Win32 structure size"),
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_SIZE_TO_CONTENT,

        pszWindowTitle: title.as_ptr(),
        Anonymous1: TASKDIALOGCONFIG_0 {
            pszMainIcon: TD_WARNING_ICON,
        },
        pszMainInstruction: instruction.as_ptr(),
        pszContent: content.as_ptr(),
        cButtons: 2,
        pButtons: buttons.as_ptr(),
        nDefaultButton: IDCANCEL,
        pszExpandedInformation: details.as_ptr(),
        pszExpandedControlText: collapse.as_ptr(),
        pszCollapsedControlText: expand.as_ptr(),
        ..Default::default()
    };
    let mut selected = 0;
    // SAFETY: Config, button, and null-terminated strings live through this synchronous
    // call; output points to a writable i32 and unused optional outputs are null.
    let status = unsafe {
        TaskDialogIndirect(
            &raw const config,
            &raw mut selected,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if status < 0 {
        return Err(io::Error::other(format!(
            "{} (HRESULT 0x{status:08X}); --confirm console",
            tr.text("error-ui")
        )));
    }
    Ok(selected == DELETE_BUTTON)
}

/// Requires an interactive terminal and accepts only an explicit affirmative.
pub fn console(paths: &[PathBuf], tr: &Translator) -> io::Result<bool> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(io::Error::other(tr.text("confirm-terminal")));
    }
    let mut output = io::stderr().lock();
    writeln!(output, "{}", tr.text("confirm-console"))?;
    for path in paths {
        writeln!(output, "  {}", display_path(path))?;
    }
    write!(
        output,
        "{} ",
        tr.count("confirm-prompt", paths.len() as u64)
    )?;
    output.flush()?;
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer)? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            tr.text("confirm-eof"),
        ));
    }
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// Keeps extended Win32 paths readable without altering the actual target.
fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
    }
}

/// `TaskDialog` ellipsizes long path-shaped text even with `SIZE_TO_CONTENT`.
/// Keep every character visible by breaking long lines at grapheme boundaries;
/// non-ASCII characters conservatively consume two display columns.
fn dialog_path(path: &Path) -> String {
    let mut output = String::new();
    let mut columns = 0;
    for grapheme in display_path(path).graphemes(true) {
        let width: usize = grapheme
            .chars()
            .map(|character| if character.is_ascii() { 1 } else { 2 })
            .sum();
        if columns > 0 && columns + width > 40 {
            output.push('\n');
            columns = 0;
        }
        output.push_str(grapheme);
        columns += width;
    }
    output
}

/// Converts UI text into a null-terminated Win32 string.
fn wide(text: &str) -> Vec<u16> { text.encode_utf16().chain(std::iter::once(0)).collect() }

#[cfg(test)]
mod tests;
