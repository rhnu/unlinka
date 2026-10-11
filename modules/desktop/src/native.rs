mod details;

use std::{
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread::JoinHandle,
};

use details::Details;
use unicode_segmentation::UnicodeSegmentation;
use unlinka_core::{DeleteOptions, DeleteReport, PreparedTargets, Progress};
use unlinka_i18n::{Translator, presentation};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::{
            Controls::{
                TASKDIALOG_BUTTON, TASKDIALOG_ELEMENTS, TASKDIALOG_NOTIFICATIONS, TASKDIALOGCONFIG,
                TDE_CONTENT, TDE_MAIN_INSTRUCTION, TDF_ALLOW_DIALOG_CANCELLATION,
                TDF_CALLBACK_TIMER, TDF_SHOW_MARQUEE_PROGRESS_BAR, TDM_CLICK_BUTTON,
                TDM_ENABLE_BUTTON, TDM_SET_ELEMENT_TEXT, TDM_SET_PROGRESS_BAR_MARQUEE,
                TDN_BUTTON_CLICKED, TDN_CREATED, TDN_TIMER, TaskDialogIndirect,
            },
            WindowsAndMessaging::{IDCANCEL, PostMessageW, SendMessageW},
        },
    },
    core::{HRESULT, PCWSTR},
};

const VIEW_DETAILS: i32 = 1009;

pub(crate) fn wide(value: &str) -> Vec<u16> { value.encode_utf16().chain(Some(0)).collect() }

fn error(value: windows::core::Error) -> io::Error { io::Error::other(value) }

pub(crate) fn dialog(
    tr: &Translator,
    instruction: &str,
    content: &str,
    labels: &[(i32, String)],
    default: i32,
    details: Option<Details<'_>>,
) -> io::Result<i32> {
    let title = wide(&tr.text("app-title"));
    let instruction = wide(instruction);
    let content = wide(content);
    let mut labels = labels.to_vec();
    if let Some(details) = &details {
        labels.insert(0, (VIEW_DETAILS, details.label.to_owned()));
    }
    let strings: Vec<_> = labels.iter().map(|(_, label)| wide(label)).collect();
    let buttons: Vec<_> = labels
        .iter()
        .zip(&strings)
        .map(|((id, _), text)| TASKDIALOG_BUTTON {
            nButtonID: *id,
            pszButtonText: PCWSTR(text.as_ptr()),
        })
        .collect();
    let has_details = details.is_some();
    let mut state = DetailState {
        details,
        close: tr.text("button-close"),
        error: None,
    };
    let config = TASKDIALOGCONFIG {
        cbSize: u32::try_from(size_of::<TASKDIALOGCONFIG>())
            .expect("native dialog structure size fits u32"),
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        pszMainInstruction: PCWSTR(instruction.as_ptr()),
        pszContent: PCWSTR(content.as_ptr()),
        cButtons: u32::try_from(buttons.len()).map_err(io::Error::other)?,
        pButtons: buttons.as_ptr(),
        nDefaultButton: default,
        pfCallback: has_details.then_some(details_callback),
        lpCallbackData: (&raw mut state) as isize,
        cxWidth: 320,
        ..Default::default()
    };
    let mut selected = 0;
    // SAFETY: The config and all referenced strings remain alive through the modal dialog.
    unsafe { TaskDialogIndirect(&raw const config, Some(&raw mut selected), None, None) }
        .map_err(error)?;
    if let Some(error) = state.error {
        return Err(error);
    }
    Ok(selected)
}

struct DetailState<'a> {
    details: Option<Details<'a>>,
    close: String,
    error: Option<io::Error>,
}

unsafe extern "system" fn details_callback(
    hwnd: HWND,
    notification: TASKDIALOG_NOTIFICATIONS,
    button: WPARAM,
    _: LPARAM,
    data: isize,
) -> HRESULT {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if notification == TDN_BUTTON_CLICKED && button.0 == VIEW_DETAILS as usize {
            // SAFETY: data points to dialog's live UI-thread state throughout its modal call.
            let state = unsafe { &mut *(data as *mut DetailState<'_>) };
            if let Some(details) = &state.details {
                if let Err(error) = details::show(hwnd, details, &state.close) {
                    state.error = Some(error);
                    // SAFETY: A details failure cancels confirmation rather than hiding targets.
                    unsafe {
                        drop(PostMessageW(
                            Some(hwnd),
                            TDM_CLICK_BUTTON.0 as u32,
                            WPARAM(IDCANCEL.0 as usize),
                            LPARAM(0),
                        ));
                    }
                }
            }
            return HRESULT(1); // Viewing details never accepts or closes the main dialog.
        }
        HRESULT(0)
    }))
    .unwrap_or(HRESULT(1))
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
    Ok(dialog(
        tr,
        &instruction,
        &content,
        &[
            (1001, tr.text("button-delete")),
            (IDCANCEL.0, tr.text("button-cancel")),
        ],
        IDCANCEL.0,
        Some(Details {
            label: &tr.text("show-paths"),
            title: &tr.text("targets-title"),
            text: &details,
        }),
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
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_CALLBACK_TIMER | TDF_SHOW_MARQUEE_PROGRESS_BAR,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        pszMainInstruction: PCWSTR(instruction.as_ptr()),
        pszContent: PCWSTR(content.as_ptr()),
        cButtons: 1,
        pButtons: &raw const button,
        nDefaultButton: IDCANCEL.0,
        pfCallback: Some(callback),
        lpCallbackData: (&raw mut state) as isize,
        cxWidth: 320,
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
                cancel_enabled(hwnd, true);
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
                let content = progress_text(
                    state.tr,
                    *state
                        .latest
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                );
                update_text(hwnd, TDE_CONTENT, &content);
            }
        } else if notification == TDN_BUTTON_CLICKED
            && button.0 == IDCANCEL.0 as usize
            && state.report.is_none()
            && !state.worker_error
        {
            state.cancel.store(true, Ordering::Relaxed);
            update_text(
                hwnd,
                TDE_MAIN_INSTRUCTION,
                &state.tr.text("progress-cancelling"),
            );
            cancel_enabled(hwnd, false);
            return HRESULT(1); // Keep the dialog open until all started work returns.
        }
        HRESULT(0)
    }))
    .unwrap_or(HRESULT(1))
}

fn update_text(hwnd: HWND, element: TASKDIALOG_ELEMENTS, text: &str) {
    let text = wide(text);
    // SAFETY: Callers use the live UI-thread task dialog; it copies text synchronously.
    unsafe {
        SendMessageW(
            hwnd,
            TDM_SET_ELEMENT_TEXT.0 as u32,
            Some(WPARAM(
                usize::try_from(element.0).expect("task dialog element IDs are nonnegative"),
            )),
            Some(LPARAM(text.as_ptr() as isize)),
        );
    }
}

fn cancel_enabled(hwnd: HWND, enabled: bool) {
    // SAFETY: Callers use the live task dialog and this message carries no pointers.
    unsafe {
        SendMessageW(
            hwnd,
            TDM_ENABLE_BUTTON.0 as u32,
            Some(WPARAM(IDCANCEL.0 as usize)),
            Some(LPARAM(isize::from(enabled))),
        );
    }
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
    dialog(
        tr,
        &presentation::outcome(tr, false, report.cancelled, report.failed),
        &summary,
        &[(IDCANCEL.0, tr.text("button-close"))],
        IDCANCEL.0,
        (!failures.is_empty()).then_some(Details {
            label: &tr.text("show-failures"),
            title: &tr.text("failures-title"),
            text: &failures,
        }),
    )
    .map(|_| ())
}
