mod cli;
mod confirm;
mod result;
mod status;

use std::{
    io::Write,
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use clap::{
    FromArgMatches,
    error::{ContextKind, ErrorKind},
};
use unlinka_core::{DeleteOptions, DeleteReport, PreparedTargets, execute, prepare};
use unlinka_i18n::{FluentValue, Translator, select_locale, windows_languages};
use unlinka_integration::{AppLease, read_settings};

use crate::{
    cli::{Cli, Confirmation, ExitStatus},
    result::ResultDocument,
};

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().collect();
    let (explicit, json) = cli::bootstrap(&arguments);
    let environment = std::env::var("UNLINKA_LANG").ok();
    let settings = read_settings().ok();
    let translator = Translator::new(select_locale(
        explicit.as_deref(),
        environment.as_deref(),
        settings.as_ref().map(|settings| settings.language.as_str()),
        &windows_languages(),
    ));
    let mut command = cli::command(&translator);
    if arguments
        .iter()
        .skip(1)
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--help" || arg == "-h")
    {
        let status = if command.print_long_help().is_ok() {
            println!();
            ExitStatus::Success
        } else {
            ExitStatus::ExecutionFailed
        };
        return ExitCode::from(status as u8);
    }
    if arguments
        .iter()
        .skip(1)
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--version" || arg == "-V")
    {
        println!("unlinka {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let cli = match command
        .try_get_matches_from(arguments)
        .and_then(|matches| Cli::from_arg_matches(&matches))
    {
        Ok(cli) => cli,
        Err(error) => {
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                return ExitCode::from(u8::from(error.print().is_err()));
            }
            let argument = error
                .get(ContextKind::InvalidArg)
                .map(ToString::to_string)
                .unwrap_or_default();
            let value = error
                .get(ContextKind::InvalidValue)
                .map(ToString::to_string)
                .unwrap_or_default();
            let message = match error.kind() {
                ErrorKind::UnknownArgument => "error-unknown",
                ErrorKind::MissingRequiredArgument => "error-missing",
                ErrorKind::InvalidValue
                | ErrorKind::ValueValidation
                | ErrorKind::TooFewValues
                | ErrorKind::TooManyValues => "error-value",
                ErrorKind::ArgumentConflict => "error-conflict",
                _ => "error-arguments",
            };
            eprintln!(
                "[invalid_arguments] {}\n{}",
                translator.format(
                    message,
                    &[
                        ("argument", argument.as_str().into()),
                        ("value", value.as_str().into())
                    ]
                ),
                translator.text("error-hint")
            );
            let mut document = ResultDocument::new(ExitStatus::InvalidInput, false, &[]);
            document.diagnostic("invalid_arguments", "parse_arguments", None, None);
            return ExitCode::from(finish(&document, json) as u8);
        },
    };
    ExitCode::from(complete(&cli, &translator) as u8)
}

/// Retains the installed activity lease until final stdout output has returned.
fn complete(cli: &Cli, tr: &Translator) -> ExitStatus {
    if cli.integration_status {
        return finish(&run(cli, tr), cli.json);
    }
    let _lease = match AppLease::acquire_if_installed() {
        Ok(lease) => lease,
        Err(error) => {
            return finish(
                &failed(
                    cli,
                    tr,
                    ExitStatus::ExecutionFailed,
                    "installation_unavailable",
                    "error-installation",
                    "acquire_lease",
                    &error,
                ),
                cli.json,
            );
        },
    };
    finish(&run(cli, tr), cli.json)
}

fn finish(document: &ResultDocument, json: bool) -> ExitStatus {
    if json {
        let stdout = std::io::stdout();
        let mut writer = stdout.lock();
        if serde_json::to_writer(&mut writer, document).is_err() || writeln!(writer).is_err() {
            return ExitStatus::ExecutionFailed;
        }
    }
    document.status()
}

fn run(cli: &Cli, tr: &Translator) -> ResultDocument {
    if cli.integration_status {
        return match unlinka_integration::maintenance::integration_status() {
            Ok(value) => {
                if !cli.json {
                    println!("{}", status::format(tr, &value));
                }
                let mut document = ResultDocument::new(ExitStatus::Success, false, &[]);
                document.integration = Some(value);
                document
            },
            Err(error) => failed(
                cli,
                tr,
                ExitStatus::ExecutionFailed,
                "integration_status_failed",
                "error-installation",
                "integration_status",
                &error,
            ),
        };
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&cancelled);
    if let Err(error) = ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed)) {
        eprintln!(
            "[cancellation_handler_failed] {}: {error}",
            tr.text("error-interrupt")
        );
        let mut document =
            ResultDocument::new(ExitStatus::ExecutionFailed, cli.dry_run, &cli.paths);
        document.diagnostic(
            "cancellation_handler_failed",
            "install_signal_handler",
            None,
            None,
        );
        return document;
    }
    if cli.verbose > 0 {
        eprintln!("{}", tr.count("preparing", cli.paths.len() as u64));
    }
    let targets = match prepare(&cli.paths, cli.recursive) {
        Ok(targets) => targets,
        Err(error) => {
            if cancelled.load(Ordering::Relaxed) {
                return cancelled_before_execution(cli, tr);
            }
            eprintln!("{}", tr.text("supplied-paths"));
            for path in &cli.paths {
                eprintln!("  {}", path.display());
            }
            return failed(
                cli,
                tr,
                ExitStatus::InvalidInput,
                unlinka_core::diagnostic_code(&error).as_str(),
                "error-prepare",
                "prepare",
                &error,
            );
        },
    };
    if cancelled.load(Ordering::Relaxed) {
        return cancelled_before_execution(cli, tr);
    }
    execute_prepared(cli, tr, targets, &cancelled)
}

fn execute_prepared(
    cli: &Cli,
    tr: &Translator,
    targets: PreparedTargets,
    cancelled: &AtomicBool,
) -> ResultDocument {
    if cli.verbose > 0 {
        eprintln!("{}", tr.count("prepared", targets.paths().len() as u64));
    }
    if cli.dry_run {
        eprintln!("{}", tr.text("dry-run"));
        for path in targets.paths() {
            eprintln!("  {}", path.display());
        }
    } else {
        eprintln!("{}", tr.text("permanent-warning"));
        if !cli.yes {
            let confirmation = match cli.confirmation() {
                Confirmation::Native => confirm::native(targets.paths(), tr),
                Confirmation::Console => confirm::console(targets.paths(), tr),
            };
            match confirmation {
                Ok(true) => {},
                Ok(false) => return cancelled_before_execution(cli, tr),
                Err(error) => {
                    if cancelled.load(Ordering::Relaxed) {
                        return cancelled_before_execution(cli, tr);
                    }
                    return failed(
                        cli,
                        tr,
                        ExitStatus::ConfirmationFailed,
                        "confirmation_failed",
                        "error-confirm",
                        "confirm",
                        &error,
                    );
                },
            }
        }
    }
    if cancelled.load(Ordering::Relaxed) {
        return cancelled_before_execution(cli, tr);
    }
    eprintln!(
        "{}",
        tr.format(
            "starting",
            &[
                (
                    "mode",
                    tr.text(if cli.dry_run {
                        "mode-scan"
                    } else {
                        "mode-delete"
                    })
                    .into()
                ),
                ("count", FluentValue::from(targets.paths().len())),
                ("threads", FluentValue::from(cli.threads))
            ]
        )
    );
    let report = execute(
        targets,
        &DeleteOptions {
            recursive: cli.recursive,
            threads: cli.threads,
            dry_run: cli.dry_run,
        },
        cancelled,
    );
    print_report(&report, cli.dry_run, tr);
    ResultDocument::from_report(
        &report,
        cli.dry_run,
        &cli.paths,
        cancelled.load(Ordering::Relaxed),
    )
}

fn failed(
    cli: &Cli,
    tr: &Translator,
    status: ExitStatus,
    code: &str,
    message: &str,
    operation: &str,
    error: &std::io::Error,
) -> ResultDocument {
    let detail = core_error(tr, error);
    eprintln!("[{code}] {}: {detail}", tr.text(message));
    let mut document = ResultDocument::new(status, cli.dry_run, &cli.paths);
    document.diagnostic(code, operation, None, error.raw_os_error());
    document
}

fn core_error(tr: &Translator, error: &std::io::Error) -> String {
    let code = unlinka_core::diagnostic_code(error).as_str();
    if code == "system" {
        error.to_string()
    } else {
        tr.text(&format!("diagnostic-{code}"))
    }
}

fn cancelled_before_execution(cli: &Cli, tr: &Translator) -> ResultDocument {
    eprintln!("{}", tr.text("cancelled-before"));
    let mut document = ResultDocument::new(ExitStatus::Cancelled, cli.dry_run, &cli.paths);
    document.diagnostic("cancelled", "confirm", None, None);
    document
}

fn print_report(report: &DeleteReport, dry_run: bool, tr: &Translator) {
    eprintln!(
        "{}",
        tr.format(
            "report",
            &[
                (
                    "mode",
                    tr.text(if dry_run { "mode-scan" } else { "mode-delete" })
                        .into()
                ),
                (
                    "state",
                    tr.text(if report.cancelled {
                        "state-cancelled"
                    } else {
                        "state-finished"
                    })
                    .into()
                ),
                (
                    "seconds",
                    format!("{:.3}", report.elapsed.as_secs_f64()).into()
                ),
                ("files", report.files.into()),
                ("directories", report.directories.into()),
                ("links", report.links.into()),
                ("failed", report.failed.into())
            ]
        )
    );
    if dry_run {
        eprintln!("{}", tr.text("dry-run-note"));
    }
    for failure in &report.failures {
        eprintln!(
            "[{}] {}",
            unlinka_core::diagnostic_code(&failure.error).as_str(),
            tr.format(
                "failure",
                &[
                    ("operation", tr.operation(failure.operation).into()),
                    ("path", failure.path.to_string_lossy().as_ref().into()),
                    ("message", core_error(tr, &failure.error).into())
                ]
            )
        );
    }
    let omitted = report.failed.saturating_sub(report.failures.len() as u64);
    if omitted > 0 {
        eprintln!("{}", tr.count("failures-omitted", omitted));
    }
}
