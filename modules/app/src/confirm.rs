use std::{
    io::{self, BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
};

use unlinka_i18n::Translator;

/// Requires an interactive terminal and accepts only an explicit affirmative.
pub fn console(paths: &[PathBuf], tr: &Translator) -> io::Result<bool> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(io::Error::other(tr.text("confirm-terminal")));
    }
    prompt(paths, tr, &mut io::stdin().lock(), &mut io::stderr().lock())
}

/// Displays the targets before reading the terminal's answer.
fn prompt(
    paths: &[PathBuf],
    tr: &Translator,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<bool> {
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
    if input.read_line(&mut answer)? == 0 {
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

#[cfg(test)]
mod tests;
