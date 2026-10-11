use std::{
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use serde::Serialize;
use unlinka_core::DeleteReport;
use unlinka_i18n::{Translator, presentation};

use crate::cli::ExitStatus;

#[derive(Debug, Serialize)]
pub(crate) struct Diagnostic {
    code: String,
    operation: String,
    path_utf16: Option<Vec<u16>>,
    os_error: Option<i32>,
}

/// Versioned, language-independent output. Raw paths preserve unpaired UTF-16.
#[derive(Debug, Serialize)]
pub(crate) struct ResultDocument {
    schema_version: u32,
    outcome: &'static str,
    exit_code: u8,
    dry_run: bool,
    files: u64,
    directories: u64,
    links: u64,
    failed: u64,
    cancelled: bool,
    elapsed_seconds: f64,
    paths_utf16: Vec<Vec<u16>>,
    diagnostics: Vec<Diagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) integration: Option<serde_json::Value>,
}

impl ResultDocument {
    pub(crate) fn new(status: ExitStatus, dry_run: bool, paths: &[PathBuf]) -> Self {
        Self {
            schema_version: 1,
            outcome: match status {
                ExitStatus::Success => "success",
                ExitStatus::InvalidInput => "invalid_input",
                ExitStatus::ExecutionFailed => "execution_failed",
                ExitStatus::Cancelled => "cancelled",
                ExitStatus::ConfirmationFailed => "confirmation_failed",
            },
            exit_code: status as u8,
            dry_run,
            files: 0,
            directories: 0,
            links: 0,
            failed: 0,
            cancelled: status == ExitStatus::Cancelled,
            elapsed_seconds: 0.0,
            paths_utf16: paths
                .iter()
                .map(|path| path.as_os_str().encode_wide().collect())
                .collect(),
            diagnostics: Vec::new(),
            integration: None,
        }
    }

    pub(crate) fn diagnostic(
        &mut self,
        code: &str,
        operation: &str,
        path: Option<&Path>,
        os_error: Option<i32>,
    ) {
        self.diagnostics.push(Diagnostic {
            code: code.into(),
            operation: operation.into(),
            path_utf16: path.map(|path| path.as_os_str().encode_wide().collect()),
            os_error,
        });
    }

    pub(crate) fn from_report(
        report: &DeleteReport,
        dry_run: bool,
        paths: &[PathBuf],
        interrupted: bool,
    ) -> Self {
        let mut document = Self::new(
            ExitStatus::for_outcome(report.cancelled || interrupted, report.failed),
            dry_run,
            paths,
        );
        document.files = report.files;
        document.directories = report.directories;
        document.links = report.links;
        document.failed = report.failed;
        document.elapsed_seconds = report.elapsed.as_secs_f64();
        for failure in &report.failures {
            document.diagnostic(
                unlinka_core::diagnostic_code(&failure.error).as_str(),
                failure.operation,
                Some(&failure.path),
                failure.error.raw_os_error(),
            );
        }
        document
    }

    pub(crate) fn status(&self) -> ExitStatus {
        match self.exit_code {
            0 => ExitStatus::Success,
            2 => ExitStatus::InvalidInput,
            3 => ExitStatus::Cancelled,
            4 => ExitStatus::ConfirmationFailed,
            _ => ExitStatus::ExecutionFailed,
        }
    }
}

/// Keeps ordinary terminal output short; verbose mode exposes retained diagnostics.
pub(crate) fn report_text(
    report: &DeleteReport,
    dry_run: bool,
    verbose: bool,
    tr: &Translator,
) -> String {
    let mut lines = vec![
        presentation::outcome(tr, dry_run, report.cancelled, report.failed),
        presentation::counts(
            tr,
            dry_run,
            report.files,
            report.directories,
            report.links,
            report.elapsed,
        ),
    ];
    if dry_run {
        lines.push(tr.text("dry-run-note"));
    }
    if report.failed > 0 {
        lines.push(tr.format("report-failed", &[("failed", report.failed.into())]));
    }
    let limit = if verbose { report.failures.len() } else { 5 };
    let shown = report.failures.len().min(limit);
    for failure in report.failures.iter().take(shown) {
        let code = unlinka_core::diagnostic_code(&failure.error).as_str();
        let detail = tr.format(
            "failure",
            &[
                ("path", presentation::path_text(&failure.path).into()),
                (
                    "message",
                    presentation::diagnostic(tr, code, &failure.error).into(),
                ),
            ],
        );
        lines.push(format!("  {}", detail.replace('\n', "\n    ")));
        if verbose {
            let mut technical = tr.format(
                "failure-technical",
                &[
                    ("operation", tr.operation(failure.operation).into()),
                    ("code", code.into()),
                ],
            );
            if let Some(code) = failure.error.raw_os_error() {
                technical.push_str(" · ");
                technical.push_str(&tr.format("failure-os-code", &[("code", code.into())]));
            }
            lines.push(format!("    {technical}"));
        }
    }
    let available = report.failed.min(report.failures.len() as u64);
    let hidden = available.saturating_sub(shown as u64);
    if hidden > 0 {
        lines.push(tr.count("failures-more", hidden));
    }
    let omitted = report.failed.saturating_sub(report.failures.len() as u64);
    if omitted > 0 {
        lines.push(tr.count("failures-omitted", omitted));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests;
