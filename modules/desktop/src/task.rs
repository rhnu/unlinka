use std::{
    cell::RefCell,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread::JoinHandle,
};

use unicode_segmentation::UnicodeSegmentation;
use unlinka_core::{DeleteOptions, DeleteReport, PreparedTargets, Progress};
use unlinka_i18n::{Translator, presentation};
use unlinka_ui::{CANCEL, Details, Dialog, ProgressDialog, ProgressEvent, ProgressUpdate};

/// Presents desktop wording through the shared native UI boundary.
pub(crate) fn presentation(
    tr: &Translator,
    instruction: &str,
    content: &str,
    labels: &[(u16, String)],
    default_button: u16,
    details: Option<Details<'_>>,
) -> io::Result<u16> {
    let buttons: Vec<_> = labels
        .iter()
        .map(|(id, label)| (*id, label.as_str()))
        .collect();
    Dialog {
        title: &tr.text("app-title"),
        instruction,
        content,
        buttons: &buttons,
        default_button,
        details,
        verification: None,
    }
    .show()
    .map(|result| result.button)
}

pub(crate) fn message(tr: &Translator, instruction: &str, content: &str) -> io::Result<()> {
    presentation(
        tr,
        instruction,
        content,
        &[(CANCEL, tr.text("button-close"))],
        CANCEL,
        None,
    )
    .map(|_| ())
}

/// Full paths live in the viewer; summaries never grow with path length.
fn compact(text: &str) -> String {
    let graphemes: Vec<_> = text.graphemes(true).collect();
    if graphemes.len() <= 64 {
        text.to_owned()
    } else {
        format!(
            "{}…{}",
            graphemes[..30].concat(),
            graphemes[graphemes.len() - 30..].concat()
        )
    }
}

pub(crate) fn confirm(tr: &Translator, paths: &[PathBuf]) -> io::Result<bool> {
    let instruction = if paths.len() == 1 {
        tr.format(
            "confirm-one",
            &[(
                "name",
                compact(
                    paths[0]
                        .file_name()
                        .unwrap_or(paths[0].as_os_str())
                        .to_string_lossy()
                        .as_ref(),
                )
                .into(),
            )],
        )
    } else {
        tr.count("confirm-many", paths.len() as u64)
    };
    let details = paths
        .iter()
        .map(|path| presentation::path_text(path))
        .collect::<Vec<_>>()
        .join("\n\n");
    let content = if paths.len() == 1 {
        format!(
            "{}\n{}",
            tr.text("location"),
            compact(&presentation::path_text(
                paths[0].parent().unwrap_or(&paths[0])
            ))
        )
    } else {
        String::new()
    };
    Ok(presentation(
        tr,
        &instruction,
        &content,
        &[
            (1001, tr.text("button-delete")),
            (CANCEL, tr.text("button-cancel")),
        ],
        CANCEL,
        Some(Details {
            label: &tr.text("show-paths"),
            title: &tr.text("targets-title"),
            text: &details,
            close: &tr.text("button-close"),
        }),
    )? == 1001)
}

/// Owns business execution independently of the modal window's lifetime.
struct Execution {
    tr: Translator,
    targets: Option<PreparedTargets>,
    cancel: Arc<AtomicBool>,
    latest: Arc<Mutex<Progress>>,
    receiver: Receiver<DeleteReport>,
    sender: Option<mpsc::Sender<DeleteReport>>,
    worker: Option<JoinHandle<()>>,
    report: Option<DeleteReport>,
    worker_error: bool,
}

impl Execution {
    fn event(&mut self, event: ProgressEvent) -> io::Result<ProgressUpdate> {
        match event {
            ProgressEvent::Created => {
                // Creation must succeed before any deletion worker can start.
                if let (Some(targets), Some(sender)) = (self.targets.take(), self.sender.take()) {
                    let cancel = Arc::clone(&self.cancel);
                    let latest = Arc::clone(&self.latest);
                    self.worker = Some(std::thread::Builder::new().spawn(move || {
                        let options = DeleteOptions {
                            recursive: true,
                            threads: std::thread::available_parallelism()
                                .map_or(1, std::num::NonZeroUsize::get)
                                .min(4),
                            dry_run: false,
                        };
                        let report = unlinka_core::execute_with_progress(
                            targets,
                            &options,
                            &cancel,
                            &|snapshot| {
                                if let Ok(mut progress) = latest.lock() {
                                    *progress = snapshot;
                                }
                            },
                        );
                        drop(sender.send(report));
                    })?);
                }
                Ok(ProgressUpdate::KeepOpen)
            },
            ProgressEvent::CancelRequested => {
                self.cancel.store(true, Ordering::Relaxed);
                Ok(ProgressUpdate::KeepOpen)
            },
            ProgressEvent::Tick => {
                match self.receiver.try_recv() {
                    Ok(report) => {
                        self.report = Some(report);
                        return Ok(ProgressUpdate::Finished);
                    },
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.worker_error = true;
                        return Ok(ProgressUpdate::Finished);
                    },
                    Err(mpsc::TryRecvError::Empty) => {},
                }
                let progress = *self
                    .latest
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                Ok(ProgressUpdate::Content(progress_text(&self.tr, progress)))
            },
        }
    }

    fn cancel_and_join(&mut self) -> io::Result<()> {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_panic| io::Error::other(self.tr.text("error-worker")))?;
        }
        Ok(())
    }
}

impl Drop for Execution {
    fn drop(&mut self) {
        // Unwinding or a UI failure must never detach an active deletion worker.
        drop(self.cancel_and_join());
    }
}

pub(crate) fn execute(tr: &Translator, targets: PreparedTargets) -> io::Result<DeleteReport> {
    let (sender, receiver) = mpsc::channel();
    let state = Rc::new(RefCell::new(Execution {
        tr: Translator::new(tr.locale()),
        targets: Some(targets),
        cancel: Arc::new(AtomicBool::new(false)),
        latest: Arc::new(Mutex::new(Progress::default())),
        receiver,
        sender: Some(sender),
        worker: None,
        report: None,
        worker_error: false,
    }));
    let handler_state = Rc::clone(&state);
    let dialog = ProgressDialog {
        title: &tr.text("app-title"),
        instruction: &tr.text("progress-title"),
        content: &progress_text(tr, Progress::default()),
        cancelling: &tr.text("progress-cancelling"),
        cancel_label: &tr.text("button-cancel"),
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        dialog.show(move |event| handler_state.borrow_mut().event(event))
    }))
    .unwrap_or_else(|_panic| Err(io::Error::other(tr.text("error-ui"))));
    let mut state = state.borrow_mut();
    state.cancel_and_join()?;
    result?;
    if state.worker_error {
        return Err(io::Error::other(tr.text("error-worker")));
    }
    state
        .report
        .take()
        .or_else(|| state.receiver.try_recv().ok())
        .ok_or_else(|| io::Error::other(tr.text("error-ui")))
}

fn progress_text(tr: &Translator, progress: Progress) -> String {
    tr.format(
        "progress",
        &[
            ("files", progress.files.into()),
            ("directories", progress.directories.into()),
            ("links", progress.links.into()),
            ("failed", progress.failed.into()),
            (
                "seconds",
                format!("{:.1}", progress.elapsed.as_secs_f64()).into(),
            ),
        ],
    )
}

pub(crate) fn diagnostic(tr: &Translator, error: &io::Error) -> String {
    presentation::diagnostic(tr, unlinka_core::diagnostic_code(error).as_str(), error)
}

pub(crate) fn results(tr: &Translator, report: &DeleteReport) -> io::Result<()> {
    let mut summary = presentation::counts(
        tr,
        false,
        report.files,
        report.directories,
        report.links,
        report.elapsed,
    );
    if report.failed > 0 {
        summary.push_str("\n\n");
        summary.push_str(&tr.format("report-failed", &[("failed", report.failed.into())]));
        if let Some(first) = report.failures.first() {
            summary.push('\n');
            summary.push_str(&tr.format(
                "failure",
                &[
                    (
                        "path",
                        compact(&presentation::path_text(&first.path)).into(),
                    ),
                    ("message", compact(&diagnostic(tr, &first.error)).into()),
                ],
            ));
        }
    }
    let mut failures = report
        .failures
        .iter()
        .map(|failure| {
            tr.format(
                "failure",
                &[
                    ("path", presentation::path_text(&failure.path).into()),
                    (
                        "message",
                        tr.format(
                            "failure-context",
                            &[
                                ("operation", tr.operation(failure.operation).into()),
                                ("message", diagnostic(tr, &failure.error).into()),
                            ],
                        )
                        .into(),
                    ),
                ],
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let omitted = report.failed.saturating_sub(report.failures.len() as u64);
    if omitted > 0 {
        if !failures.is_empty() {
            failures.push_str("\n\n");
        }
        failures.push_str(&tr.count("failures-omitted", omitted));
    }
    presentation(
        tr,
        &presentation::outcome(tr, false, report.cancelled, report.failed),
        &summary,
        &[(CANCEL, tr.text("button-close"))],
        CANCEL,
        (!failures.is_empty()).then_some(Details {
            label: &tr.text("show-failures"),
            title: &tr.text("failures-title"),
            text: &failures,
            close: &tr.text("button-close"),
        }),
    )
    .map(|_| ())
}

#[cfg(test)]
mod tests;
