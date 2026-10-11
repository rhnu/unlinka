//! A bounded native viewer keeps complete target and failure lists out of `TaskDialog` sizing.

use std::{
    cell::RefCell,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
};

use windows_sys::Win32::UI::HiDpi::SystemParametersInfoForDpi;
use winsafe::{self as w, co, gui, prelude::*};

const CLOSE: u16 = 2;
type ViewerFont = w::guard::DeleteObjectGuard<w::HFONT>;
type FontState = Rc<RefCell<Option<ViewerFont>>>;

/// Runs the viewer on the `TaskDialog` thread and restores its enabled state and focus.
pub(super) fn show(owner: &w::HWND, title: &str, text: &str, close: &str) -> io::Result<()> {
    guarded(|| show_inner(owner, title, text, close)).map_err(io::Error::other)
}

fn show_inner(owner: &w::HWND, title: &str, text: &str, close: &str) -> w::AnyResult<()> {
    let dpi = owner.GetDpiForWindow();
    let dpi = if dpi == 0 { 96 } else { dpi };
    let signed_dpi = i32::try_from(dpi)?;
    let scale = |value| w::MulDiv(value, signed_dpi, 96);
    // Declared before the window so the final font outlives ViewerCleanup and its controls.
    let font = Rc::new(RefCell::new(None));
    let accelerators = w::HACCEL::CreateAcceleratorTable(&[
        w::ACCEL {
            fVirt: co::ACCELF::VIRTKEY,
            key: co::VK::ESCAPE,
            cmd: CLOSE,
        },
        w::ACCEL {
            fVirt: co::ACCELF::VIRTKEY,
            key: co::VK::RETURN,
            cmd: CLOSE,
        },
    ])?;
    let wnd = gui::WindowMain::new(gui::WindowMainOpts {
        title,
        size: (scale(640), scale(384)),
        // Set the TaskDialog owner and position before the window becomes visible.
        style: co::WS::CAPTION | co::WS::SYSMENU | co::WS::CLIPCHILDREN | co::WS::BORDER,
        ex_style: co::WS_EX::DLGMODALFRAME,
        accel_table: Some(accelerators),
        ..Default::default()
    });
    let edit = gui::Edit::new(
        &wnd,
        gui::EditOpts {
            position: (scale(16), scale(16)),
            width: scale(608),
            height: scale(310),
            // Omitting AUTOHSCROLL keeps long paths wrapped; the standard edit supports Ctrl+C.
            control_style: co::ES::MULTILINE
                | co::ES::AUTOVSCROLL
                | co::ES::READONLY
                | co::ES::NOHIDESEL,
            window_style: co::WS::CHILD | co::WS::VISIBLE | co::WS::TABSTOP | co::WS::VSCROLL,
            ctrl_id: 101,
            ..Default::default()
        },
    );
    let button = gui::Button::new(
        &wnd,
        gui::ButtonOpts {
            text: close,
            position: (scale(518), scale(342)),
            width: scale(106),
            height: scale(26),
            control_style: co::BS::DEFPUSHBUTTON,
            ctrl_id: CLOSE,
            ..Default::default()
        },
    );

    events(owner, &wnd, &edit, &button, &font, text, dpi);
    let _owner = ModalOwner::new(owner);
    let _viewer = ViewerCleanup(wnd.clone());
    // WindowMain consumes the WM_QUIT posted by its own destruction, leaving TaskDialog open.
    wnd.run_main(None)?;
    Ok(())
}

/// Register native callbacks while keeping all font ownership outside the message boundary.
fn events(
    owner: &w::HWND,
    wnd: &gui::WindowMain,
    edit: &gui::Edit,
    button: &gui::Button,
    font: &FontState,
    text: &str,
    dpi: u32,
) {
    // SAFETY: The TaskDialog stays live for this synchronous nested call. The copy is borrowed
    // by the creation callback only and never destroyed by the viewer.
    let owner_copy = unsafe { owner.raw_copy() };
    let text = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n");
    let wnd_create = wnd.clone();
    let edit_create = edit.clone();
    let button_create = button.clone();
    let font_create = Rc::clone(font);
    wnd.on().wm_create(move |_| {
        guarded(|| {
            initialize(&owner_copy, &wnd_create, dpi)?;
            update_layout(&edit_create, &button_create, &font_create, dpi)?;
            edit_create.limit_text(None);
            edit_create.set_text(&text)?;
            edit_create.hwnd().SetFocus();
            Ok(0)
        })
    });
    let wnd_button = wnd.clone();
    button.on().bn_clicked(move || {
        guarded(|| {
            wnd_button.close();
            Ok(())
        })
    });
    let wnd_keys = wnd.clone();
    wnd.on().wm_command_acc_menu(CLOSE, move || {
        guarded(|| {
            wnd_keys.close();
            Ok(())
        })
    });
    let wnd_dpi = wnd.clone();
    let edit_dpi = edit.clone();
    let button_dpi = button.clone();
    let font_dpi = Rc::clone(font);
    wnd.on().wm(co::WM::DPICHANGED, move |message| {
        guarded(|| {
            dpi_changed(&wnd_dpi, &edit_dpi, &button_dpi, &font_dpi, message)?;
            Ok(0)
        })
    });
}

/// Attach and size the viewer before it becomes visible, using the owner's monitor DPI.
fn initialize(owner: &w::HWND, wnd: &gui::WindowMain, dpi: u32) -> w::AnyResult<()> {
    w::SetLastError(co::ERROR::SUCCESS);
    // SAFETY: Both handles are live windows on this thread. Changing HWNDPARENT on this
    // top-level window sets ownership without replacing its window procedure.
    let previous = unsafe {
        wnd.hwnd()
            .SetWindowLongPtr(co::GWLP::HWNDPARENT, owner.ptr() as isize)
    };
    if previous == 0 {
        let error = w::GetLastError();
        if error != co::ERROR::SUCCESS {
            return Err(error.into());
        }
    }
    let owner_rect = owner.GetWindowRect()?;
    let initial_rect = wnd.hwnd().GetWindowRect()?;
    // Moving first lets Windows assign the target monitor DPI before the final client sizing.
    // A synchronous WM_DPICHANGED during this move may itself apply suggested bounds.
    wnd.hwnd().SetWindowPos(
        w::HwndPlace::None,
        w::POINT::with(
            owner_rect.right.midpoint(owner_rect.left)
                - (initial_rect.right - initial_rect.left) / 2,
            owner_rect.bottom.midpoint(owner_rect.top)
                - (initial_rect.bottom - initial_rect.top) / 2,
        ),
        w::SIZE::default(),
        co::SWP::NOZORDER | co::SWP::NOACTIVATE | co::SWP::NOSIZE,
    )?;
    let signed_dpi = i32::try_from(dpi)?;
    let client = w::RECT {
        right: w::MulDiv(640, signed_dpi, 96),
        bottom: w::MulDiv(384, signed_dpi, 96),
        ..Default::default()
    };
    let bounds = w::AdjustWindowRectExForDpi(
        client,
        wnd.hwnd().style(),
        false,
        wnd.hwnd().style_ex(),
        dpi,
    )?;
    let width = bounds.right - bounds.left;
    let height = bounds.bottom - bounds.top;
    wnd.hwnd().SetWindowPos(
        w::HwndPlace::None,
        w::POINT::with(
            owner_rect.right.midpoint(owner_rect.left) - width / 2,
            owner_rect.bottom.midpoint(owner_rect.top) - height / 2,
        ),
        w::SIZE::with(width, height),
        co::SWP::NOZORDER | co::SWP::NOACTIVATE,
    )?;
    Ok(())
}

/// Apply Windows' suggested bounds, then rebuild the controls for the new monitor DPI.
fn dpi_changed(
    wnd: &gui::WindowMain,
    edit: &gui::Edit,
    button: &gui::Button,
    font: &FontState,
    message: w::msg::Wm,
) -> w::AnyResult<()> {
    let dpi = u32::try_from(message.wparam & 0xFFFF)?;
    let suggested = message.lparam as *const w::RECT;
    if dpi == 0 || suggested.is_null() {
        return Err(io::Error::other("Invalid details viewer DPI message").into());
    }
    // SAFETY: WM_DPICHANGED supplies an aligned, readable RECT for this synchronous callback.
    // Copy its value now; no pointer is retained after message processing.
    let bounds = unsafe { *suggested };
    wnd.hwnd().SetWindowPos(
        w::HwndPlace::None,
        w::POINT::with(bounds.left, bounds.top),
        w::SIZE::with(bounds.right - bounds.left, bounds.bottom - bounds.top),
        co::SWP::NOZORDER | co::SWP::NOACTIVATE,
    )?;
    update_layout(edit, button, font, dpi)
}

/// Swap fonts only after every control has stopped using the previous font.
fn update_layout(
    edit: &gui::Edit,
    button: &gui::Button,
    font: &FontState,
    dpi: u32,
) -> w::AnyResult<()> {
    let signed_dpi = i32::try_from(dpi)?;
    let scale = |value| w::MulDiv(value, signed_dpi, 96);
    let new_font = viewer_font(dpi)?;
    for (hwnd, position, size) in [
        (edit.hwnd(), (16, 16), (608, 310)),
        (button.hwnd(), (518, 342), (106, 26)),
    ] {
        hwnd.SetWindowPos(
            w::HwndPlace::None,
            w::POINT::with(scale(position.0), scale(position.1)),
            w::SIZE::with(scale(size.0), scale(size.1)),
            co::SWP::NOZORDER | co::SWP::NOACTIVATE,
        )?;
    }
    for hwnd in [edit.hwnd(), button.hwnd()] {
        // SAFETY: This non-owning copy is sent to a control; the guard keeps ownership.
        let hfont = unsafe { new_font.raw_copy() };
        // SAFETY: Each live native control copies this font handle synchronously. The owned
        // font remains alive until after ViewerCleanup has destroyed both controls.
        unsafe {
            hwnd.SendMessage(w::msg::WmSetFont {
                hfont,
                redraw: true,
            });
        };
    }
    // Neither borrowed state nor an old font is released during synchronous window calls.
    font.replace(Some(new_font));
    Ok(())
}

/// Request the native font metrics at this DPI instead of using a cached system DPI ratio.
fn viewer_font(dpi: u32) -> w::AnyResult<ViewerFont> {
    let mut metrics = w::NONCLIENTMETRICS::default();
    // SAFETY: WinSafe NONCLIENTMETRICS is repr(C), contains the Windows-defined fields and
    // initialized cbSize, and stays exclusively borrowed through this synchronous call.
    let success = unsafe {
        SystemParametersInfoForDpi(
            co::SPI::GETNONCLIENTMETRICS.raw(),
            u32::try_from(std::mem::size_of::<w::NONCLIENTMETRICS>())?,
            (&raw mut metrics).cast(),
            0,
            dpi,
        )
    };
    if success == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(w::HFONT::CreateFontIndirect(&metrics.lfMenuFont)?)
}

/// Convert callback panics into `WinSafe` event errors before they reach the Windows ABI.
fn guarded<T>(callback: impl FnOnce() -> w::AnyResult<T>) -> w::AnyResult<T> {
    catch_unwind(AssertUnwindSafe(callback))
        .unwrap_or_else(|_| Err(io::Error::other("The details viewer panicked").into()))
}

struct ModalOwner<'a> {
    hwnd: &'a w::HWND,
    enabled: bool,
    focus: Option<w::HWND>,
}

impl<'a> ModalOwner<'a> {
    fn new(hwnd: &'a w::HWND) -> Self {
        let focus = w::HWND::GetFocus().filter(|focus| focus == hwnd || hwnd.IsChild(focus));
        let enabled = hwnd.IsWindowEnabled();
        hwnd.EnableWindow(false);
        Self {
            hwnd,
            enabled,
            focus,
        }
    }
}

impl Drop for ModalOwner<'_> {
    fn drop(&mut self) {
        if self.hwnd.IsWindow() {
            self.hwnd.EnableWindow(self.enabled);
            if self.enabled {
                self.hwnd.SetForegroundWindow();
                if let Some(focus) = self.focus.as_ref().filter(|focus| focus.IsWindow()) {
                    focus.SetFocus();
                } else {
                    self.hwnd.SetFocus();
                }
            }
        }
    }
}

/// Event errors can end `WinSafe`'s loop before its window is destroyed.
struct ViewerCleanup(gui::WindowMain);

impl Drop for ViewerCleanup {
    fn drop(&mut self) {
        if self.0.hwnd().IsWindow() && self.0.hwnd().DestroyWindow().is_ok() {
            // Destruction posts one WM_QUIT outside run_main's loop on this failure path.
            // Consume it so the surrounding TaskDialog loop does not inherit viewer shutdown.
            w::PeekMessage(
                &mut w::MSG::default(),
                None,
                co::WM::QUIT.raw(),
                co::WM::QUIT.raw(),
                co::PM::REMOVE,
            );
        }
    }
}
