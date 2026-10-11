mod cli;
mod confirm;

use std::{
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use clap::Parser;
use unlinka_core::{DeleteOptions, DeleteReport, execute, prepare};

use crate::cli::{Cli, Confirmation, ExitStatus};

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = u8::try_from(error.exit_code())
                .expect("clap uses only exit code 0 for help or 2 for invalid input");
            if error.print().is_err() {
                return ExitCode::from(ExitStatus::ExecutionFailed as u8);
            }
            return ExitCode::from(code);
        },
    };
    ExitCode::from(run(&cli) as u8)
}

/// Prepare all targets before confirmation; no mutation occurs until execution.
fn run(cli: &Cli) -> ExitStatus {
    let cancelled = Arc::new(AtomicBool::new(false));
    let interrupt_flag = Arc::clone(&cancelled);
    if let Err(error) = ctrlc::set_handler(move || {
        interrupt_flag.store(true, Ordering::Relaxed);
    }) {
        eprintln!("error: cannot install Ctrl+C cancellation handler: {error}");
        return ExitStatus::ExecutionFailed;
    }

    if cli.verbose > 0 {
        eprintln!("Preparing {} supplied path(s).", cli.paths.len());
    }
    let targets = match prepare(&cli.paths, cli.recursive) {
        Ok(targets) => targets,
        Err(error) => {
            if cancelled.load(Ordering::Relaxed) {
                return cancelled_before_execution();
            }
            eprintln!("error: target preparation failed: {error}");
            eprintln!("Supplied paths:");
            for path in &cli.paths {
                eprintln!("  {}", path.display());
            }
            return ExitStatus::InvalidInput;
        },
    };

    if cancelled.load(Ordering::Relaxed) {
        return cancelled_before_execution();
    }

    if cli.verbose > 0 {
        eprintln!(
            "Prepared {} top-level target(s) after normalization and overlap removal.",
            targets.paths().len()
        );
    }

    if cli.dry_run {
        eprintln!("Dry run: no items will be deleted. Top-level targets:");
        for path in targets.paths() {
            eprintln!("  {}", path.display());
        }
    } else {
        eprintln!("Permanent deletion: items do not enter the Recycle Bin.");
        if !cli.yes {
            let confirmed = match cli.confirmation() {
                Confirmation::Native => confirm::native(targets.paths()),
                Confirmation::Console => confirm::console(targets.paths()),
            };
            match confirmed {
                Ok(true) => {},
                Ok(false) => return cancelled_before_execution(),
                Err(error) => {
                    if cancelled.load(Ordering::Relaxed) {
                        return cancelled_before_execution();
                    }
                    eprintln!("error: confirmation failed: {error}");
                    return ExitStatus::ConfirmationFailed;
                },
            }
        }
    }

    // A signal may arrive while the native or console confirmation is open.
    if cancelled.load(Ordering::Relaxed) {
        return cancelled_before_execution();
    }

    eprintln!(
        "Starting {} for {} top-level target(s) with a total thread budget of {} (including scanning).",
        if cli.dry_run { "scan" } else { "deletion" },
        targets.paths().len(),
        cli.threads
    );
    let report = execute(
        targets,
        &DeleteOptions {
            recursive: cli.recursive,
            threads: cli.threads,
            dry_run: cli.dry_run,
        },
        &cancelled,
    );
    print_report(&report, cli.dry_run);
    ExitStatus::for_outcome(
        report.cancelled || cancelled.load(Ordering::Relaxed),
        report.failed,
    )
}

fn cancelled_before_execution() -> ExitStatus {
    eprintln!("Cancelled. No targets were deleted.");
    ExitStatus::Cancelled
}

fn print_report(report: &DeleteReport, dry_run: bool) {
    if dry_run {
        eprintln!(
            "Dry-run scan {} in {:.3}s: {} file(s), {} directory/directories, {} link(s), {} failure(s). Nothing was deleted.",
            if report.cancelled {
                "cancelled"
            } else {
                "finished"
            },
            report.elapsed.as_secs_f64(),
            report.files,
            report.directories,
            report.links,
            report.failed
        );
        eprintln!(
            "Counts cover the scanned targets and, with --recursive, their contents. Links are counted separately; link targets are not scanned."
        );
    } else {
        eprintln!(
            "Deletion {} in {:.3}s: {} file(s), {} directory/directories, {} link(s) deleted; {} failure(s).",
            if report.cancelled {
                "cancelled"
            } else {
                "finished"
            },
            report.elapsed.as_secs_f64(),
            report.files,
            report.directories,
            report.links,
            report.failed
        );
    }

    for failure in report.failures.iter().take(100) {
        eprintln!(
            "error: {} '{}': {}",
            failure.operation,
            failure.path.display(),
            failure.error
        );
    }
    let omitted = report.failed.saturating_sub(
        u64::try_from(report.failures.len().min(100)).expect("at most 100 failures are displayed"),
    );
    if omitted > 0 {
        eprintln!("{omitted} additional failure(s) omitted.");
    }
}
