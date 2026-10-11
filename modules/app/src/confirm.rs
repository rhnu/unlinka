use std::{
    io::{self, BufRead, IsTerminal, Write},
    path::PathBuf,
};

use unlinka_i18n::{Translator, presentation::path_text};

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
        writeln!(output, "  {}", path_text(path))?;
    }
    write!(output, "{} ", tr.text("confirm-prompt"))?;
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

#[cfg(test)]
mod tests;
