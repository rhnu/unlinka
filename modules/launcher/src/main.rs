#![cfg(windows)]

use std::{
    ffi::OsString,
    io,
    os::windows::{ffi::OsStringExt, process::CommandExt},
    process::{Command, Stdio},
};

use unlinka_i18n::{Translator, select_locale, windows_languages};
use unlinka_integration::{AppLease, Installation};
use windows::{
    Win32::System::{
        Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler},
        Environment::GetCommandLineW,
    },
    core::BOOL,
};

// The launcher stays alive for result output. The child alone handles cancellation.
unsafe extern "system" fn control(signal: u32) -> BOOL {
    (signal == CTRL_C_EVENT || signal == CTRL_BREAK_EVENT).into()
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!(
                "{}",
                translator().format("maintenance-error", &[("reason", error.to_string().into())])
            );
            std::process::exit(2);
        },
    }
}

fn run() -> io::Result<i32> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args
        .iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "--integration-status")
    {
        if let Some(installation) =
            Installation::current()?.filter(|installation| !installation.active_version.is_empty())
        {
            let executable = installation.executable("unlinka.exe")?;
            if executable.is_file() {
                return child(executable);
            }
        }
        return Ok(unlinka_integration::status::query(&args));
    }
    let lease = match AppLease::acquire_if_installed() {
        Ok(lease) => lease,
        Err(error) if args.len() == 1 && (args[0] == "--version" || args[0] == "-V") => {
            println!(
                "{}",
                translator().format(
                    "maintenance-launcher-version",
                    &[
                        ("version", env!("CARGO_PKG_VERSION").into()),
                        ("reason", error.to_string().into())
                    ]
                )
            );
            return Ok(0);
        },
        Err(error) if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") => {
            println!(
                "{}",
                translator().format(
                    "maintenance-launcher-help",
                    &[("reason", error.to_string().into())]
                )
            );
            return Ok(0);
        },
        Err(error) => return Err(error),
    };
    let _lease = lease.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            translator().text("maintenance-installation-missing"),
        )
    })?;
    let installation =
        Installation::current()?.ok_or_else(|| io::Error::other("installation is missing"))?;
    let executable = installation.executable("unlinka.exe")?;
    child(executable)
}

fn child(executable: std::path::PathBuf) -> io::Result<i32> {
    // SAFETY: callback has the documented ABI and remains loaded until the launcher exits.
    unsafe { SetConsoleCtrlHandler(Some(control), true) }
        .map_err(|e| io::Error::other(e.to_string()))?;
    // Preserve the exact UTF-16 argument tail, including quotes, trailing slashes and --.
    // SAFETY: Windows owns a stable terminated command-line buffer for the process lifetime.
    let pointer = unsafe { GetCommandLineW() };
    // SAFETY: pointer references the live process command-line buffer returned above.
    let command_line = unsafe { pointer.as_wide() };
    let tail = raw_argument_tail(command_line);
    let status = Command::new(executable)
        .raw_arg(OsString::from_wide(tail))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    Ok(status.code().unwrap_or(2))
}

fn translator() -> Translator {
    let saved = unlinka_integration::read_settings()
        .ok()
        .map(|s| s.language);
    Translator::new(select_locale(
        None,
        None,
        saved.as_deref(),
        &windows_languages(),
    ))
}

fn raw_argument_tail(command: &[u16]) -> &[u16] {
    let mut index = 0;
    while index < command.len() && matches!(command[index], 9 | 32) {
        index += 1;
    }
    let mut quoted = false;
    while index < command.len() {
        match command[index] {
            34 => quoted = !quoted,
            9 | 32 if !quoted => break,
            _ => {},
        }
        index += 1;
    }
    while index < command.len() && matches!(command[index], 9 | 32) {
        index += 1;
    }
    &command[index..]
}

#[cfg(test)]
mod tests;
