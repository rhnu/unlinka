#![windows_subsystem = "windows"]
mod settings;
mod task;

use std::{io, process::ExitCode};

use unlinka_i18n::{Translator, select_locale, windows_languages};
use unlinka_integration::{AppLease, Installation, read_settings, request::consume_shell_request};

fn main() -> ExitCode {
    let saved = read_settings().ok();
    let fallback = Translator::new(select_locale(
        None,
        None,
        saved.as_ref().map(|settings| settings.language.as_str()),
        &windows_languages(),
    ));
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let settings_requested = arguments.len() == 1 && arguments[0] == "--settings";
    let result = if settings_requested {
        settings::show(&fallback)
    } else if arguments.len() == 2 && arguments[0] == "--shell-request" {
        match arguments[1].to_str() {
            Some(id) => shell_task(id, &fallback),
            None => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                fallback.text("error-request"),
            )),
        }
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            fallback.text("error-arguments"),
        ))
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            drop(task::message(
                &fallback,
                &fallback.text(if settings_requested {
                    "error-maintenance"
                } else {
                    "error-request"
                }),
                &error.to_string(),
            ));
            ExitCode::from(1)
        },
    }
}

fn shell_task(id: &str, fallback: &Translator) -> io::Result<u8> {
    let _lease = AppLease::acquire_if_installed()?
        .ok_or_else(|| io::Error::other(fallback.text("error-installation")))?;
    let installation = Installation::current()?
        .ok_or_else(|| io::Error::other(fallback.text("error-installation")))?;
    std::env::set_current_dir(&installation.root)?;
    let request = consume_shell_request(id)?;
    let paths = request.paths()?;
    let tr = Translator::new(select_locale(
        Some(&request.locale),
        None,
        None,
        &windows_languages(),
    ));
    let targets = match unlinka_core::prepare(&paths, true) {
        Ok(targets) => targets,
        Err(error) => {
            task::message(
                &tr,
                &tr.text("error-prepare"),
                &task::diagnostic(&tr, &error),
            )?;
            return Ok(2);
        },
    };
    match task::confirm(&tr, targets.paths()) {
        Ok(true) => {},
        Ok(false) => return Ok(3),
        Err(error) => {
            task::message(&tr, &tr.text("error-confirm"), &error.to_string())?;
            return Ok(4);
        },
    }
    let report = match task::execute(&tr, targets) {
        Ok(report) => report,
        Err(error) => {
            task::message(&tr, &tr.text("error-ui"), &error.to_string())?;
            return Ok(1);
        },
    };
    task::results(&tr, &report)?;
    Ok(if report.cancelled {
        3
    } else {
        u8::from(report.failed > 0)
    })
}
