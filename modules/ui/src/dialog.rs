//! Modal task dialogs with owned callback data and fail-closed details handling.
use std::{
    cell::RefCell,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
};

use winsafe::{TASKDIALOGCONFIG, Tdn, co};

use crate::{details, messages};

/// Standard Windows cancellation ID, including Escape and window close.
pub const CANCEL: u16 = 2;
const VIEW_DETAILS: u16 = 1009;

/// Complete selectable text displayed separately from the bounded main dialog.
pub struct Details<'a> {
    pub label: &'a str,
    pub title: &'a str,
    pub text: &'a str,
    pub close: &'a str,
}

/// Optional verification checkbox, used by maintenance confirmation.
pub struct Verification<'a> {
    pub label: &'a str,
    pub checked: bool,
}

/// Product-sized task dialog; callers own text, actions, and business decisions.
pub struct Dialog<'a> {
    pub title: &'a str,
    pub instruction: &'a str,
    pub content: &'a str,
    pub buttons: &'a [(u16, &'a str)],
    pub default_button: u16,
    pub details: Option<Details<'a>>,
    pub verification: Option<Verification<'a>>,
}

/// Action and verification value returned when the dialog closes.
pub struct DialogResult {
    pub button: u16,
    pub checked: bool,
}

impl Dialog<'_> {
    /// Displays the dialog and returns the chosen action and verification value.
    ///
    /// # Errors
    /// Returns an error for invalid action IDs, dialog creation failure, or details failure.
    pub fn show(&self) -> io::Result<DialogResult> {
        self.validate()?;
        let mut buttons = self.buttons.to_vec();
        if let Some(details) = &self.details {
            buttons.insert(0, (VIEW_DETAILS, details.label));
        }
        let error = Rc::new(RefCell::new(None));
        let callback = self.details.as_ref().map(|details| {
            let (title, text, close) = (
                details.title.to_owned(),
                details.text.to_owned(),
                details.close.to_owned(),
            );
            let error = Rc::clone(&error);
            Box::new(move |hwnd: &winsafe::HWND, event: Tdn| {
                if !matches!(event, Tdn::ButtonClicked(VIEW_DETAILS)) {
                    return co::HRESULT::S_OK;
                }
                let result = catch_unwind(AssertUnwindSafe(|| {
                    details::show(hwnd, &title, &text, &close)
                }))
                .unwrap_or_else(|_| Err(io::Error::other("native details callback panicked")));
                if let Err(failure) = result {
                    *error.borrow_mut() = Some(failure);
                    messages::close(hwnd);
                }
                co::HRESULT::S_FALSE // Viewing details never confirms or closes the parent.
            }) as Box<dyn Fn(&winsafe::HWND, Tdn) -> co::HRESULT>
        });
        let flags = co::TDF::ALLOW_DIALOG_CANCELLATION
            | if self
                .verification
                .as_ref()
                .is_some_and(|value| value.checked)
            {
                co::TDF::VERIFICATION_FLAG_CHECKED
            } else {
                co::TDF::default()
            };
        let result = winsafe::TaskDialogIndirect(&TASKDIALOGCONFIG {
            flags,
            window_title: Some(self.title),
            main_instruction: Some(self.instruction),
            content: Some(self.content),
            buttons: &buttons,
            default_button_id: self.default_button,
            verification_text: self.verification.as_ref().map(|value| value.label),
            callback,
            width: 320,
            ..Default::default()
        })
        .map_err(io::Error::other);
        if let Some(error) = error.borrow_mut().take() {
            return Err(error);
        }
        let (button, _, checked) = result?;
        Ok(DialogResult {
            button: button.raw(),
            checked,
        })
    }

    fn validate(&self) -> io::Result<()> {
        if !self
            .buttons
            .iter()
            .any(|(id, _)| *id == self.default_button)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "default dialog action is missing",
            ));
        }
        for (index, (id, _)) in self.buttons.iter().enumerate() {
            if *id == 0
                || (self.details.is_some() && *id == VIEW_DETAILS)
                || self.buttons[..index]
                    .iter()
                    .any(|(previous, _)| previous == id)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "dialog action IDs must be unique and nonzero",
                ));
            }
        }
        Ok(())
    }
}
