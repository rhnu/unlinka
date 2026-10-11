use std::{
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
};

use unicode_segmentation::UnicodeSegmentation;
use windows_sys::Win32::UI::{
    Controls::{
        TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0, TD_WARNING_ICON,
        TDCBF_CANCEL_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION, TDF_SIZE_TO_CONTENT,
        TaskDialogIndirect,
    },
    WindowsAndMessaging::IDCANCEL,
};

const DELETE_BUTTON: i32 = 1001;

/// Confirms permanent deletion using the system task dialog and a safe default.
pub fn native(paths: &[PathBuf]) -> io::Result<bool> {
    let title = wide("Unlinka — 删除确认");
    let instruction = if paths.len() == 1 {
        let name = paths[0]
            .file_name()
            .unwrap_or(paths[0].as_os_str())
            .to_string_lossy();
        format!("要永久删除“{name}”吗？")
    } else {
        format!("要永久删除这 {} 个项目吗？", paths.len())
    };
    let instruction = wide(&instruction);
    let content = if paths.len() == 1 {
        format!(
            "这些项目不会移入回收站。\n已经删除的内容无法通过取消操作恢复。\n\n位置：\n{}",
            dialog_path(paths[0].parent().unwrap_or(&paths[0]))
        )
    } else {
        "这些项目不会移入回收站。\n已经删除的内容无法通过取消操作恢复。".to_owned()
    };
    let content = wide(&content);
    let details = paths
        .iter()
        .map(|path| dialog_path(path))
        .collect::<Vec<_>>()
        .join("\r\n\r\n");
    let details = wide(&details);
    let expand = wide("显示完整路径");
    let collapse = wide("隐藏完整路径");
    let delete_text = wide("删除");
    let button = TASKDIALOG_BUTTON {
        nButtonID: DELETE_BUTTON,
        pszButtonText: delete_text.as_ptr(),
    };
    let config = TASKDIALOGCONFIG {
        cbSize: u32::try_from(std::mem::size_of::<TASKDIALOGCONFIG>())
            .expect("the task dialog config has a fixed Win32 structure size"),
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_SIZE_TO_CONTENT,
        dwCommonButtons: TDCBF_CANCEL_BUTTON,
        pszWindowTitle: title.as_ptr(),
        Anonymous1: TASKDIALOGCONFIG_0 {
            pszMainIcon: TD_WARNING_ICON,
        },
        pszMainInstruction: instruction.as_ptr(),
        pszContent: content.as_ptr(),
        cButtons: 1,
        pButtons: &raw const button,
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
            "无法显示 Windows 原生确认窗口（HRESULT 0x{status:08X}）；可显式使用 --confirm console"
        )));
    }
    Ok(selected == DELETE_BUTTON)
}

/// Requires an interactive terminal and accepts only an explicit affirmative.
pub fn console(paths: &[PathBuf]) -> io::Result<bool> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(io::Error::other(
            "终端确认需要交互式 stdin 和 stderr；脚本请显式使用 --yes 或 --dry-run",
        ));
    }
    let mut output = io::stderr().lock();
    writeln!(output, "永久删除以下项目（不会移入回收站）：")?;
    for path in paths {
        writeln!(output, "  {}", display_path(path))?;
    }
    write!(output, "确认删除 {} 个项目？[y/N] ", paths.len())?;
    output.flush()?;
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer)? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "确认输入提前结束",
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
