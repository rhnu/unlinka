//! Progress UI reports lifecycle events; task execution stays with the caller.
use std::{
    cell::RefCell,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
};

use winsafe::{TASKDIALOGCONFIG, Tdn, co};

use crate::{CANCEL, messages};

/// UI notifications that let the caller start, observe, or cancel its own task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgressEvent {
    Created,
    Tick,
    CancelRequested,
}

/// Display changes returned by the task controller.
pub enum ProgressUpdate {
    KeepOpen,
    Content(String),
    Finished,
}

/// An indeterminate progress dialog that waits for completion after cancellation.
pub struct ProgressDialog<'a> {
    pub title: &'a str,
    pub instruction: &'a str,
    pub content: &'a str,
    pub cancelling: &'a str,
    pub cancel_label: &'a str,
}

impl ProgressDialog<'_> {
    /// Displays progress until the controller finishes, including after cancellation.
    /// The controller starts work only on `Created`; the caller cancels and joins
    /// any worker when this method returns an error.
    ///
    /// # Errors
    /// Returns dialog creation errors and controller errors or caught callback panics.
    pub fn show<F>(&self, handler: F) -> io::Result<()>
    where
        F: FnMut(ProgressEvent) -> io::Result<ProgressUpdate> + 'static,
    {
        let session = Rc::new(RefCell::new(Session {
            handler,
            phase: Phase::Waiting,
        }));
        let error = Rc::new(RefCell::new(None));
        let callback_session = Rc::clone(&session);
        let callback_error = Rc::clone(&error);
        let cancelling = self.cancelling.to_owned();
        let callback = Box::new(move |hwnd: &winsafe::HWND, event: Tdn| {
            let result = catch_unwind(AssertUnwindSafe(|| {
                let (event, cancelling_now) = match event {
                    Tdn::Created => {
                        messages::marquee(hwnd);
                        (ProgressEvent::Created, false)
                    },
                    Tdn::Timer(_) => (ProgressEvent::Tick, false),
                    Tdn::ButtonClicked(CANCEL) => {
                        let phase = callback_session.borrow().phase;
                        if phase == Phase::Finished {
                            return Ok(co::HRESULT::S_OK);
                        }
                        (ProgressEvent::CancelRequested, phase == Phase::Running)
                    },
                    _ => return Ok(co::HRESULT::S_OK),
                };
                let update = callback_session.borrow_mut().handle(event)?;
                match update {
                    ProgressUpdate::KeepOpen => {},
                    ProgressUpdate::Content(content) => messages::content(hwnd, &content),
                    ProgressUpdate::Finished => {
                        messages::cancel_enabled(hwnd, true);
                        messages::close(hwnd);
                    },
                }
                if cancelling_now && callback_session.borrow().phase != Phase::Finished {
                    messages::instruction(hwnd, &cancelling);
                    messages::cancel_enabled(hwnd, false);
                }
                Ok(if event == ProgressEvent::CancelRequested {
                    co::HRESULT::S_FALSE
                } else {
                    co::HRESULT::S_OK
                })
            }))
            .unwrap_or_else(|_| Err(io::Error::other("native progress callback panicked")));
            match result {
                Ok(result) => result,
                Err(failure) => {
                    *callback_error.borrow_mut() = Some(failure);
                    callback_session.borrow_mut().phase = Phase::Finished;
                    messages::cancel_enabled(hwnd, true);
                    messages::close(hwnd);
                    co::HRESULT::S_FALSE
                },
            }
        });
        let result = winsafe::TaskDialogIndirect(&TASKDIALOGCONFIG {
            flags: co::TDF::ALLOW_DIALOG_CANCELLATION
                | co::TDF::CALLBACK_TIMER
                | co::TDF::SHOW_MARQUEE_PROGRESS_BAR,
            window_title: Some(self.title),
            main_instruction: Some(self.instruction),
            content: Some(self.content),
            buttons: &[(CANCEL, self.cancel_label)],
            default_button_id: CANCEL,
            callback: Some(callback),
            width: 320,
            ..Default::default()
        })
        .map_err(io::Error::other);
        if let Some(error) = error.borrow_mut().take() {
            return Err(error);
        }
        result.map(|_| ())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Waiting,
    Running,
    Cancelling,
    Finished,
}

struct Session<F> {
    handler: F,
    phase: Phase,
}

impl<F> Session<F>
where
    F: FnMut(ProgressEvent) -> io::Result<ProgressUpdate>,
{
    fn handle(&mut self, event: ProgressEvent) -> io::Result<ProgressUpdate> {
        match (self.phase, event) {
            (Phase::Finished, _) => return Ok(ProgressUpdate::Finished),
            (Phase::Waiting, ProgressEvent::Created) => self.phase = Phase::Running,
            (Phase::Running, ProgressEvent::CancelRequested) => self.phase = Phase::Cancelling,
            (Phase::Waiting, _)
            | (_, ProgressEvent::Created)
            | (Phase::Cancelling, ProgressEvent::CancelRequested) => {
                return Ok(ProgressUpdate::KeepOpen);
            },
            _ => {},
        }
        let update = (self.handler)(event)?;
        if matches!(update, ProgressUpdate::Finished) {
            self.phase = Phase::Finished;
        }
        Ok(update)
    }
}

#[cfg(test)]
mod tests;
