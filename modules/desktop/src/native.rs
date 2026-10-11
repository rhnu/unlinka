use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread::JoinHandle,
};

use unicode_segmentation::UnicodeSegmentation;
use unlinka_core::{DeleteOptions, DeleteReport, PreparedTargets, Progress};
use unlinka_i18n::Translator;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::{
            Controls::{
                TASKDIALOG_BUTTON, TASKDIALOG_NOTIFICATIONS, TASKDIALOGCONFIG, TDE_CONTENT,
                TDF_ALLOW_DIALOG_CANCELLATION, TDF_CALLBACK_TIMER, TDF_SHOW_MARQUEE_PROGRESS_BAR,
                TDF_SIZE_TO_CONTENT, TDM_CLICK_BUTTON, TDM_SET_ELEMENT_TEXT,
                TDM_SET_PROGRESS_BAR_MARQUEE, TDN_BUTTON_CLICKED, TDN_CREATED, TDN_TIMER,
                TaskDialogIndirect,
            },
            WindowsAndMessaging::{IDCANCEL, PostMessageW, SendMessageW},
        },
    },
    core::{HRESULT, PCWSTR},
};

pub(crate) fn wide(value: &str) -> Vec<u16> { value.encode_utf16().chain(Some(0)).collect() }

fn error(value: windows::core::Error) -> io::Error { io::Error::other(value) }

pub(crate) fn dialog(
    tr: &Translator,
    instruction: &str,
    content: &str,
    labels: &[(i32, String)],
    default: i32,
    details: Option<&str>,
) -> io::Result<i32> {
    let title = wide(&tr.text("app-title"));
    let instruction = wide(instruction);
    let content = wide(content);
    let strings: Vec<_> = labels.iter().map(|(_, label)| wide(label)).collect();
    let buttons: Vec<_> = labels
        .iter()
        .zip(&strings)
        .map(|((id, _), text)| TASKDIALOG_BUTTON {
            nButtonID: *id,
            pszButtonText: PCWSTR(text.as_ptr()),
        })
        .collect();
    let details = details.map(wide);
    let show = wide(&tr.text("show-paths"));
    let hide = wide(&tr.text("hide-paths"));
    let config = TASKDIALOGCONFIG {
        cbSize: u32::try_from(size_of::<TASKDIALOGCONFIG>())
            .expect("native dialog structure size fits u32"),
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_SIZE_TO_CONTENT,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        pszMainInstruction: PCWSTR(instruction.as_ptr()),
        pszContent: PCWSTR(content.as_ptr()),
        cButtons: u32::try_from(buttons.len()).map_err(io::Error::other)?,
        pButtons: buttons.as_ptr(),
        nDefaultButton: default,
        pszExpandedInformation: details
            .as_ref()
            .map_or(PCWSTR::null(), |text| PCWSTR(text.as_ptr())),
        pszCollapsedControlText: PCWSTR(show.as_ptr()),
        pszExpandedControlText: PCWSTR(hide.as_ptr()),
        ..Default::default()
    };
    let mut selected = 0;
    // SAFETY: The config and all referenced strings remain alive through the modal dialog.
    unsafe { TaskDialogIndirect(&raw const config, Some(&raw mut selected), None, None) }
        .map_err(error)?;
    Ok(selected)
}

pub(crate) fn message(tr: &Translator, instruction: &str, content: &str) -> io::Result<()> {
    dialog(
        tr,
        instruction,
        content,
        &[(IDCANCEL.0, tr.text("button-close"))],
        IDCANCEL.0,
        None,
    )
    .map(|_| ())
}

fn path_text(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(&text).into()
    };
    let mut result = String::new();
    let mut columns = 0;
    for grapheme in text.graphemes(true) {
        let width = grapheme
            .chars()
            .map(|ch| if ch.is_ascii() { 1 } else { 2 })
            .sum::<usize>();
        if columns > 0 && columns + width > 60 {
            result.push('\n');
            columns = 0;
        }
        result.push_str(grapheme);
        columns += width;
    }
    result
}

pub(crate) fn confirm(tr: &Translator, paths: &[PathBuf]) -> io::Result<bool> {
    let instruction = if paths.len() == 1 {
        tr.format(
            "confirm-one",
            &[(
                "name",
                paths[0]
                    .file_name()
                    .unwrap_or(paths[0].as_os_str())
                    .to_string_lossy()
                    .as_ref()
                    .into(),
            )],
        )
    } else {
        tr.count("confirm-many", paths.len() as u64)
    };
    let details = paths
        .iter()
        .map(|path| path_text(path))
        .collect::<Vec<_>>()
        .join("\n\n");
    let content = if paths.len() == 1 {
        format!(
            "{}\n\n{}\n{}",
            tr.text("confirm-warning"),
            tr.text("location"),
            path_text(paths[0].parent().unwrap_or(&paths[0]))
        )
    } else {
        tr.text("confirm-warning")
    };
    Ok(dialog(
        tr,
        &instruction,
        &content,
        &[
            (1001, tr.text("button-delete")),
            (IDCANCEL.0, tr.text("button-cancel")),
        ],
        IDCANCEL.0,
        Some(&details),
    )? == 1001)
}

struct Execution<'a> {
    tr: &'a Translator,
    targets: Option<PreparedTargets>,
    cancel: Arc<AtomicBool>,
    latest: Arc<Mutex<Progress>>,
    receiver: Receiver<DeleteReport>,
    sender: Option<mpsc::Sender<DeleteReport>>,
    worker: Option<JoinHandle<()>>,
    report: Option<DeleteReport>,
    worker_error: bool,
}

pub(crate) fn execute(tr: &Translator, targets: PreparedTargets) -> io::Result<DeleteReport> {
    let (sender, receiver) = mpsc::channel();
    let mut state = Execution {
        tr,
        targets: Some(targets),
        cancel: Arc::new(AtomicBool::new(false)),
        latest: Arc::new(Mutex::new(Progress::default())),
        receiver,
        sender: Some(sender),
        worker: None,
        report: None,
        worker_error: false,
    };
    let title = wide(&tr.text("app-title"));
    let instruction = wide(&tr.text("progress-title"));
    let content = wide(&progress_text(tr, Progress::default()));
    let cancel = wide(&tr.text("button-cancel"));
    let button = TASKDIALOG_BUTTON {
        nButtonID: IDCANCEL.0,
        pszButtonText: PCWSTR(cancel.as_ptr()),
    };
    let config = TASKDIALOGCONFIG {
        cbSize: u32::try_from(size_of::<TASKDIALOGCONFIG>())
            .expect("native dialog structure size fits u32"),
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION
            | TDF_CALLBACK_TIMER
            | TDF_SHOW_MARQUEE_PROGRESS_BAR
            | TDF_SIZE_TO_CONTENT,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        pszMainInstruction: PCWSTR(instruction.as_ptr()),
        pszContent: PCWSTR(content.as_ptr()),
        cButtons: 1,
        pButtons: &raw const button,
        nDefaultButton: IDCANCEL.0,
        pfCallback: Some(callback),
        lpCallbackData: (&raw mut state) as isize,
        ..Default::default()
    };
    // SAFETY: The callback state and UI strings live until TaskDialogIndirect returns.
    let result = unsafe { TaskDialogIndirect(&raw const config, None, None, None) }.map_err(error);
    state.cancel.store(true, Ordering::Relaxed);
    if let Some(worker) = state.worker.take() {
        worker
            .join()
            .map_err(|_panic| io::Error::other(tr.text("error-worker")))?;
    }
    result?;
    state
        .report
        .or_else(|| state.receiver.try_recv().ok())
        .ok_or_else(|| io::Error::other(tr.text("error-ui")))
}

unsafe extern "system" fn callback(
    hwnd: HWND,
    notification: TASKDIALOG_NOTIFICATIONS,
    button: WPARAM,
    _: LPARAM,
    data: isize,
) -> HRESULT {
    // No panic may unwind across the native callback boundary.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: data references the caller's modal-dialog state, used only on its UI thread.
        let state = unsafe { &mut *(data as *mut Execution<'_>) };
        if notification == TDN_CREATED {
            if let (Some(targets), Some(sender)) = (state.targets.take(), state.sender.take()) {
                let cancel = Arc::clone(&state.cancel);
                let latest = Arc::clone(&state.latest);
                state.worker = Some(std::thread::spawn(move || {
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
                }));
            }
            // SAFETY: hwnd belongs to the live task dialog, and these messages carry no pointers.
            unsafe {
                SendMessageW(
                    hwnd,
                    TDM_SET_PROGRESS_BAR_MARQUEE.0 as u32,
                    Some(WPARAM(1)),
                    Some(LPARAM(0)),
                );
            }
        } else if notification == TDN_TIMER {
            let completed = match state.receiver.try_recv() {
                Ok(report) => {
                    state.report = Some(report);
                    true
                },
                Err(mpsc::TryRecvError::Disconnected) => {
                    state.worker_error = true;
                    true
                },
                Err(mpsc::TryRecvError::Empty) => false,
            };
            if completed {
                // SAFETY: The completion report is retained; clicking Cancel now closes the dialog.
                unsafe {
                    drop(PostMessageW(
                        Some(hwnd),
                        TDM_CLICK_BUTTON.0 as u32,
                        WPARAM(IDCANCEL.0 as usize),
                        LPARAM(0),
                    ));
                }
            } else {
                let content = if state.cancel.load(Ordering::Relaxed) {
                    state.tr.text("progress-cancelling")
                } else {
                    progress_text(
                        state.tr,
                        *state
                            .latest
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner),
                    )
                };
                let content = wide(&content);
                // SAFETY: TaskDialog copies the text synchronously while the buffer is alive.
                unsafe {
                    SendMessageW(
                        hwnd,
                        TDM_SET_ELEMENT_TEXT.0 as u32,
                        Some(WPARAM(TDE_CONTENT.0 as usize)),
                        Some(LPARAM(content.as_ptr() as isize)),
                    );
                }
            }
        } else if notification == TDN_BUTTON_CLICKED
            && button.0 == IDCANCEL.0 as usize
            && state.report.is_none()
            && !state.worker_error
        {
            state.cancel.store(true, Ordering::Relaxed);
            return HRESULT(1); // Keep the dialog open until all started work returns.
        }
        HRESULT(0)
    }))
    .unwrap_or(HRESULT(1))
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
    let code = unlinka_core::diagnostic_code(error).as_str();
    if code == "system" {
        format!("[{code}] {error}")
    } else {
        format!("[{code}] {}", tr.text(&format!("diagnostic-{code}")))
    }
}

pub(crate) fn results(tr: &Translator, report: &DeleteReport) -> io::Result<()> {
    let summary = tr.format(
        "report",
        &[
            ("mode", tr.text("mode-delete").into()),
            (
                "state",
                tr.text(if report.cancelled {
                    "state-cancelled"
                } else {
                    "state-finished"
                })
                .into(),
            ),
            (
                "seconds",
                format!("{:.3}", report.elapsed.as_secs_f64()).into(),
            ),
            ("files", report.files.into()),
            ("directories", report.directories.into()),
            ("links", report.links.into()),
            ("failed", report.failed.into()),
        ],
    );
    let mut failures = report
        .failures
        .iter()
        .map(|failure| {
            format!(
                "{}\n{}\n{}",
                tr.operation(failure.operation),
                path_text(&failure.path),
                diagnostic(tr, &failure.error)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let omitted = report.failed.saturating_sub(report.failures.len() as u64);
    if omitted > 0 {
        failures.push('\n');
        failures.push_str(&tr.count("failures-omitted", omitted));
    }
    dialog(
        tr,
        &tr.text("results-title"),
        &summary,
        &[(IDCANCEL.0, tr.text("button-close"))],
        IDCANCEL.0,
        (!failures.is_empty()).then_some(failures.as_str()),
    )
    .map(|_| ())
}
