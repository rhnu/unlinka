use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use unlinka_core::{DeleteReport, Progress};
use unlinka_i18n::{Locale, Translator};
use unlinka_ui::{ProgressEvent, ProgressUpdate};

use super::Execution;

fn execution() -> Execution {
    let (sender, receiver) = mpsc::channel();
    Execution {
        tr: Translator::new(Locale::EnUs),
        targets: None,
        cancel: Arc::new(AtomicBool::new(false)),
        latest: Arc::new(Mutex::new(Progress::default())),
        receiver,
        sender: Some(sender),
        worker: None,
        report: None,
        worker_error: false,
    }
}

#[test]
fn cancellation_waits_for_the_worker_report() {
    let mut state = execution();
    let sender = state.sender.take().unwrap();
    let (release, released) = mpsc::channel();
    state.worker = Some(std::thread::spawn(move || {
        released.recv().unwrap();
        sender
            .send(DeleteReport {
                cancelled: true,
                ..DeleteReport::default()
            })
            .unwrap();
    }));

    assert!(matches!(
        state.event(ProgressEvent::CancelRequested).unwrap(),
        ProgressUpdate::KeepOpen
    ));
    assert!(state.cancel.load(Ordering::Relaxed));
    assert!(matches!(
        state.event(ProgressEvent::Tick).unwrap(),
        ProgressUpdate::Content(_)
    ));
    assert!(state.report.is_none());

    release.send(()).unwrap();
    state.cancel_and_join().unwrap();
    assert!(matches!(
        state.event(ProgressEvent::Tick).unwrap(),
        ProgressUpdate::Finished
    ));
    assert!(state.report.as_ref().unwrap().cancelled);
}

#[test]
fn dropping_execution_cancels_and_joins_the_worker() {
    let mut state = execution();
    let cancel = Arc::clone(&state.cancel);
    let (finished, completion) = mpsc::channel();
    state.worker = Some(std::thread::spawn(move || {
        while !cancel.load(Ordering::Relaxed) {
            std::thread::yield_now();
        }
        finished.send(()).unwrap();
    }));

    drop(state);
    assert!(completion.try_recv().is_ok());
}

#[test]
fn disconnected_worker_finishes_with_an_error() {
    let mut state = execution();
    drop(state.sender.take());
    assert!(matches!(
        state.event(ProgressEvent::Tick).unwrap(),
        ProgressUpdate::Finished
    ));
    assert!(state.worker_error);
    assert!(state.report.is_none());
}
