//! In-process COM entry points. The worker releases its loader reference atomically on exit.

use std::{
    ffi::c_void,
    io,
    mem::size_of,
    os::windows::ffi::OsStrExt,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use unlinka_i18n::{Locale, Translator, select_locale, windows_languages};
use unlinka_integration::{Installation, InstallationState, read_settings};
use unlinka_ui::message_box;
use windows::{
    Win32::{
        Foundation::{
            CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, CloseHandle, E_FAIL, E_INVALIDARG,
            E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, FreeLibrary, HMODULE, S_FALSE, S_OK,
        },
        System::{
            Com::{CoTaskMemAlloc, IBindCtx, IClassFactory, IClassFactory_Impl, IDataObject},
            LibraryLoader::{
                FreeLibraryAndExitThread, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
                GetModuleHandleExW,
            },
            Registry::HKEY,
            Threading::{CreateThread, THREAD_CREATION_FLAGS},
        },
        UI::{
            Shell::{
                CMF_DEFAULTONLY, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, Common::ITEMIDLIST,
                ECF_DEFAULT, ECS_ENABLED, ECS_HIDDEN, GCS_HELPTEXTA, GCS_HELPTEXTW, GCS_VALIDATEA,
                GCS_VALIDATEW, GCS_VERBA, GCS_VERBW, IContextMenu, IContextMenu_Impl,
                IEnumExplorerCommand, IExplorerCommand, IExplorerCommand_Impl, IShellExtInit,
                IShellExtInit_Impl, IShellItemArray, SHCreateShellItemArrayFromDataObject,
                SHGetInstanceExplorer,
            },
            WindowsAndMessaging::{HMENU, InsertMenuW, MF_BYPOSITION, MF_STRING},
        },
    },
    core::{
        BOOL, Error, GUID, HRESULT, IUnknown, Interface, PCWSTR, PSTR, PWSTR, Ref, Result,
        implement,
    },
};

use crate::selection::copy_selection;

const VERB: &str = "Unlinka.PermanentDelete";
const CMIC_MASK_UNICODE: u32 = 0x4000_0000;
static OBJECTS: AtomicUsize = AtomicUsize::new(0);
static SERVER_LOCKS: AtomicUsize = AtomicUsize::new(0);
static WORKERS: AtomicUsize = AtomicUsize::new(0);

/// Counts COM objects until their final `IUnknown` release (including class factories).
struct ObjectLease;

impl ObjectLease {
    fn new() -> Self {
        OBJECTS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for ObjectLease {
    fn drop(&mut self) { OBJECTS.fetch_sub(1, Ordering::SeqCst); }
}

fn boundary<T>(action: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(action)).unwrap_or_else(|_| Err(Error::from_hresult(E_FAIL)))
}

fn io_error(error: io::Error) -> Error {
    let code = error
        .raw_os_error()
        .and_then(|code| u32::try_from(code).ok())
        .map_or(E_FAIL, HRESULT::from_win32);
    let message = error.to_string();
    drop(error);
    Error::new(code, message)
}

fn identity_guid(value: &str) -> Result<GUID> { GUID::try_from(value.trim_matches(['{', '}'])) }

fn locale() -> Result<Locale> {
    let settings = read_settings().map_err(io_error)?;
    Ok(select_locale(
        None,
        None,
        Some(&settings.language),
        &windows_languages(),
    ))
}

fn ready_locale() -> Result<Locale> {
    let installation = Installation::current().map_err(io_error)?;
    let settings = read_settings().map_err(io_error)?;
    if !settings.menus_enabled
        || !installation.is_some_and(|installation| installation.state == InstallationState::Ready)
    {
        return Err(Error::from_hresult(E_FAIL));
    }
    Ok(select_locale(
        None,
        None,
        Some(&settings.language),
        &windows_languages(),
    ))
}

fn title() -> Result<String> { Ok(Translator::new(locale()?).text("menu-delete")) }

fn wide(text: &str) -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() }

fn allocated_string(text: &str) -> Result<PWSTR> {
    let text = wide(text);
    allocated_wide(&text)
}

fn allocated_wide(text: &[u16]) -> Result<PWSTR> {
    // SAFETY: COM expects a caller-owned CoTaskMem string; the allocation size includes NUL.
    let memory = unsafe { CoTaskMemAlloc(std::mem::size_of_val(text)).cast::<u16>() };
    if memory.is_null() {
        return Err(Error::from_hresult(E_OUTOFMEMORY));
    }
    // SAFETY: allocation and source have the same checked length and do not overlap.
    unsafe { std::ptr::copy_nonoverlapping(text.as_ptr(), memory, text.len()) };
    Ok(PWSTR(memory))
}

#[expect(
    clippy::as_pointer_underscore,
    clippy::ref_as_ptr,
    clippy::inline_always,
    reason = "windows implement emits these interface conversions; this scope contains only generated COM bindings"
)]
mod classic_object {
    use super::{IContextMenu, IShellExtInit, Mutex, ObjectLease, PathBuf, implement};

    #[implement(IShellExtInit, IContextMenu)]
    pub(super) struct ClassicCommand {
        pub(super) paths: Mutex<Vec<PathBuf>>,
        pub(super) _lease: ObjectLease,
    }
}
use classic_object::{ClassicCommand, ClassicCommand_Impl};

impl ClassicCommand {
    fn new() -> Self {
        Self {
            _lease: ObjectLease::new(),
            paths: Mutex::new(Vec::new()),
        }
    }
}

impl IShellExtInit_Impl for ClassicCommand_Impl {
    fn Initialize(
        &self,
        _folder: *const ITEMIDLIST,
        data: Ref<IDataObject>,
        _key: HKEY,
    ) -> Result<()> {
        boundary(|| {
            let mut paths = self
                .paths
                .lock()
                .map_err(|error| Error::new(E_FAIL, error.to_string()))?;
            paths.clear();
            let Some(data) = data.as_ref() else {
                // A directory background supplies no data object and must remain hidden.
                return Ok(());
            };
            // SAFETY: data belongs to this COM call; conversion retains the complete selection.
            let items: IShellItemArray = unsafe { SHCreateShellItemArrayFromDataObject(data)? };
            *paths = copy_selection(&items)?;
            Ok(())
        })
    }
}

impl IContextMenu_Impl for ClassicCommand_Impl {
    fn QueryContextMenu(
        &self,
        menu: HMENU,
        index: u32,
        first: u32,
        last: u32,
        flags: u32,
    ) -> HRESULT {
        boundary(|| {
            if flags & CMF_DEFAULTONLY != 0 || first > last || ready_locale().is_err() {
                return Ok(S_OK);
            }
            if self
                .paths
                .lock()
                .map_err(|error| Error::new(E_FAIL, error.to_string()))?
                .is_empty()
            {
                return Ok(S_OK);
            }
            let text = wide(&title()?);
            // SAFETY: Explorer supplies the menu handle and InsertMenuW copies the text.
            unsafe {
                InsertMenuW(
                    menu,
                    index,
                    MF_BYPOSITION | MF_STRING,
                    first as usize,
                    PCWSTR(text.as_ptr()),
                )?;
            };
            // The low word is the consumed command count, not the assigned absolute menu ID.
            Ok(HRESULT(1))
        })
        .unwrap_or_else(|error| error.code())
    }

    fn InvokeCommand(&self, info: *const CMINVOKECOMMANDINFO) -> Result<()> {
        boundary(|| {
            // SAFETY: the COM contract provides a readable structure; size gates EX-only fields.
            unsafe { validate_invocation(info)? };
            let paths = self
                .paths
                .lock()
                .map_err(|error| Error::new(E_FAIL, error.to_string()))?
                .clone();
            dispatch(paths)
        })
    }

    fn GetCommandString(
        &self,
        id: usize,
        kind: u32,
        _reserved: *const u32,
        output: PSTR,
        capacity: u32,
    ) -> Result<()> {
        boundary(|| {
            if id != 0 {
                return Err(Error::from_hresult(E_INVALIDARG));
            }
            match kind {
                GCS_VALIDATEA | GCS_VALIDATEW => Ok(()),
                GCS_VERBA | GCS_VERBW | GCS_HELPTEXTA | GCS_HELPTEXTW => {
                    let help = matches!(kind, GCS_HELPTEXTA | GCS_HELPTEXTW);
                    let text = if help { title()? } else { VERB.to_owned() };
                    // SAFETY: the COM caller supplies a buffer of cchMax characters.
                    unsafe { write_command_string(output, capacity, kind & 4 != 0, &text) }
                },
                _ => Err(Error::from_hresult(E_INVALIDARG)),
            }
        })
    }
}

/// Reads only the supported canonical verb, with a bounded comparison instead of unbounded strlen.
unsafe fn validate_invocation(info: *const CMINVOKECOMMANDINFO) -> Result<()> {
    if info.is_null() {
        return Err(Error::from_hresult(E_POINTER));
    }
    // SAFETY: validated pointer, caller owns the structure throughout this invocation.
    let info_ref = unsafe { &*info };
    if (info_ref.cbSize as usize) < size_of::<CMINVOKECOMMANDINFO>() {
        return Err(Error::from_hresult(E_INVALIDARG));
    }
    let unicode = info_ref.cbSize as usize >= size_of::<CMINVOKECOMMANDINFOEX>()
        && info_ref.fMask & CMIC_MASK_UNICODE != 0;
    // Numeric offsets always live in lpVerb, including an EX structure with UNICODE set.
    if u16::try_from(info_ref.lpVerb.0 as usize).is_ok() {
        return if info_ref.lpVerb.is_null() {
            Ok(())
        } else {
            Err(Error::from_hresult(E_FAIL))
        };
    }
    let valid = if unicode {
        // SAFETY: cbSize confirms the extended structure layout is present.
        let verb = unsafe { (*(info.cast::<CMINVOKECOMMANDINFOEX>())).lpVerbW.0 };
        if u16::try_from(verb as usize).is_ok() {
            false
        } else {
            VERB.bytes()
                .chain(Some(0))
                .enumerate()
                .all(|(index, byte)| {
                    // SAFETY: a string verb has at least its terminating NUL; stop at the first mismatch.
                    let current = verb.wrapping_add(index);
                    // SAFETY: comparison stops at the first mismatch, including a premature NUL.
                    let actual = unsafe { *current };
                    u8::try_from(actual).is_ok_and(|actual| actual.eq_ignore_ascii_case(&byte))
                })
        }
    } else {
        let verb = info_ref.lpVerb.0;
        if u16::try_from(verb as usize).is_ok() {
            verb.is_null()
        } else {
            VERB.bytes()
                .chain(Some(0))
                .enumerate()
                .all(|(index, byte)| {
                    // SAFETY: a string verb has at least its terminating NUL; comparison short-circuits.
                    let current = verb.wrapping_add(index);
                    // SAFETY: comparison short-circuits at the first mismatch or premature NUL.
                    let actual = unsafe { *current };
                    actual.eq_ignore_ascii_case(&byte)
                })
        }
    };
    if valid {
        Ok(())
    } else {
        Err(Error::from_hresult(E_FAIL))
    }
}

unsafe fn write_command_string(
    output: PSTR,
    capacity: u32,
    unicode: bool,
    text: &str,
) -> Result<()> {
    if output.is_null() || capacity == 0 {
        return Err(Error::from_hresult(E_POINTER));
    }
    if unicode {
        let units = wide(text);
        let length = units.len().saturating_sub(1).min(capacity as usize - 1);
        let destination = output.0.cast::<c_void>().cast::<u16>();
        // SAFETY: GCS_*W supplies aligned UTF-16 storage with capacity measured in units.
        unsafe { std::ptr::copy_nonoverlapping(units.as_ptr(), destination, length) };
        let terminator = destination.wrapping_add(length);
        // SAFETY: length reserves one writable unit inside the caller's capacity for NUL.
        unsafe { *terminator = 0 };
    } else {
        // The canonical verb is ASCII. ANSI help uses an ASCII-safe explanation.
        let bytes = if text.is_ascii() {
            text.as_bytes()
        } else {
            b"Permanently delete with Unlinka..."
        };
        let length = bytes.len().min(capacity as usize - 1);
        // SAFETY: capacity counts bytes for the GCS_*A variants, reserving one byte for NUL.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), output.0, length) };
        let terminator = output.0.wrapping_add(length);
        // SAFETY: length reserves one writable byte within the caller's capacity for NUL.
        unsafe { *terminator = 0 };
    }
    Ok(())
}

#[expect(
    clippy::as_pointer_underscore,
    clippy::ref_as_ptr,
    clippy::inline_always,
    reason = "windows implement emits these interface conversions; this scope contains only generated COM bindings"
)]
mod modern_object {
    use super::{IExplorerCommand, ObjectLease, implement};

    #[implement(IExplorerCommand)]
    pub(super) struct ModernCommand {
        pub(super) _lease: ObjectLease,
    }
}
use modern_object::{ModernCommand, ModernCommand_Impl};

impl IExplorerCommand_Impl for ModernCommand_Impl {
    fn GetTitle(&self, _items: Ref<IShellItemArray>) -> Result<PWSTR> {
        boundary(|| allocated_string(&title()?))
    }

    fn GetIcon(&self, _items: Ref<IShellItemArray>) -> Result<PWSTR> {
        boundary(|| {
            let installation = Installation::current()
                .map_err(io_error)?
                .ok_or_else(|| Error::from_hresult(E_FAIL))?;
            let path = installation.active_dir().join("Assets").join("Unlinka.ico");
            let location: Vec<u16> = std::iter::once(u16::from(b'"'))
                .chain(path.as_os_str().encode_wide())
                .chain([u16::from(b'"'), u16::from(b','), u16::from(b'0'), 0])
                .collect();
            allocated_wide(&location)
        })
    }

    fn GetToolTip(&self, _items: Ref<IShellItemArray>) -> Result<PWSTR> {
        boundary(|| allocated_string(&title()?))
    }

    fn GetCanonicalName(&self) -> Result<GUID> {
        boundary(|| identity_guid(unlinka_integration::ACTION_GUID))
    }

    fn GetState(&self, items: Ref<IShellItemArray>, _slow: BOOL) -> Result<u32> {
        boundary(|| {
            let visible = ready_locale().is_ok()
                && items
                    .as_ref()
                    .is_some_and(|items| copy_selection(items).is_ok());
            Ok(if visible {
                ECS_ENABLED.0 as u32
            } else {
                ECS_HIDDEN.0 as u32
            })
        })
    }

    fn Invoke(&self, items: Ref<IShellItemArray>, _context: Ref<IBindCtx>) -> Result<()> {
        boundary(|| {
            let items = items
                .as_ref()
                .ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
            dispatch(copy_selection(items)?)
        })
    }

    fn GetFlags(&self) -> Result<u32> { boundary(|| Ok(ECF_DEFAULT.0 as u32)) }

    fn EnumSubCommands(&self) -> Result<IEnumExplorerCommand> {
        boundary(|| Err(Error::from_hresult(E_NOTIMPL)))
    }
}

/// Worker data is completely owned; no apartment-bound COM interface crosses threads.
struct RequestWork {
    module: HMODULE,
    paths: Vec<PathBuf>,
    locale: Locale,
    // SHGetInstanceExplorer returns a free-threaded, Release-only host reference.
    // The worker never queries it or uses any apartment-bound interface.
    _host: Option<IUnknown>,
}

/// Releases a loader reference if dispatch fails or unwinds before the worker owns it.
struct ModulePin(HMODULE);

impl Drop for ModulePin {
    fn drop(&mut self) {
        // SAFETY: this reference belongs to dispatch, whose COM call still has the DLL loaded.
        drop(unsafe { FreeLibrary(self.0) });
    }
}

fn dispatch(paths: Vec<PathBuf>) -> Result<()> {
    if paths.is_empty() {
        return Err(Error::from_hresult(E_INVALIDARG));
    }
    let locale = ready_locale()?;
    let mut module = HMODULE::default();
    // SAFETY: FROM_ADDRESS treats the address as code, without reading it as a string.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
            PCWSTR(request_worker as *const u16),
            &raw mut module,
        )?;
    };
    let pin = ModulePin(module);
    let work = Box::new(RequestWork {
        module,
        paths,
        locale,
        // SAFETY: Shell documents this reference as free-threaded; unsupported hosts return none.
        _host: unsafe { SHGetInstanceExplorer().ok() },
    });
    let pointer = Box::into_raw(work);
    WORKERS.fetch_add(1, Ordering::SeqCst);
    // SAFETY: the worker uniquely owns the box after successful creation and has a loader reference.
    let thread = unsafe {
        CreateThread(
            None,
            0,
            Some(request_worker),
            Some(pointer.cast()),
            THREAD_CREATION_FLAGS(0),
            None,
        )
    };
    match thread {
        Ok(thread) => {
            // The native worker now owns this reference, including if handle cleanup fails.
            std::mem::forget(pin);
            // SAFETY: closing the thread handle does not stop the thread or release its module pin.
            unsafe { CloseHandle(thread)? };
            Ok(())
        },
        Err(error) => {
            // SAFETY: no worker started, so the caller still owns the box and loader reference.
            unsafe { drop(Box::from_raw(pointer)) };
            WORKERS.fetch_sub(1, Ordering::SeqCst);
            Err(error)
        },
    }
}

unsafe extern "system" fn request_worker(context: *mut c_void) -> u32 {
    // SAFETY: dispatch transferred this uniquely owned box to CreateThread.
    let work = unsafe { Box::from_raw(context.cast::<RequestWork>()) };
    let module = work.module;
    // Catch both request and diagnostic failures before exiting native code.
    drop(catch_unwind(AssertUnwindSafe(|| {
        let result = ready_locale().map(|_| ()).and_then(|()| {
            unlinka_integration::request::submit_shell_request(&work.paths, work.locale.as_str())
                .map_err(io_error)
        });
        if let Err(error) = result {
            let text = Translator::new(work.locale).format(
                "shell-request-error",
                &[("error", error.to_string().into())],
            );
            drop(message_box("Unlinka", &text, true));
        }
    })));
    // Destructors must run before unloading: returning through a Rust/Windows trampoline after
    // FreeLibrary would execute potentially unmapped code. This API releases and exits atomically.
    drop(work);
    WORKERS.fetch_sub(1, Ordering::SeqCst);
    // SAFETY: dispatch obtained this module reference and the native thread ends here.
    unsafe { FreeLibraryAndExitThread(module, 0) }
}

#[derive(Clone, Copy)]
enum CommandClass {
    Classic,
    Modern,
}

#[expect(
    clippy::as_pointer_underscore,
    clippy::ref_as_ptr,
    clippy::inline_always,
    reason = "windows implement emits these interface conversions; this scope contains only generated COM bindings"
)]
mod factory_object {
    use super::{CommandClass, IClassFactory, ObjectLease, implement};

    #[implement(IClassFactory)]
    pub(super) struct Factory {
        pub(super) _lease: ObjectLease,
        pub(super) class: CommandClass,
    }
}
use factory_object::{Factory, Factory_Impl};

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        iid: *const GUID,
        output: *mut *mut c_void,
    ) -> Result<()> {
        boundary(|| {
            if iid.is_null() || output.is_null() {
                return Err(Error::from_hresult(E_POINTER));
            }
            // SAFETY: COM supplies writable output storage; initialize it on every failure path.
            unsafe { *output = std::ptr::null_mut() };
            if outer.as_ref().is_some() {
                return Err(Error::from_hresult(CLASS_E_NOAGGREGATION));
            }
            let object: IUnknown = match self.class {
                CommandClass::Classic => ClassicCommand::new().into(),
                CommandClass::Modern => ModernCommand {
                    _lease: ObjectLease::new(),
                }
                .into(),
            };
            // SAFETY: pointers are validated; query adds the requested interface reference.
            unsafe { object.query(iid, output).ok() }
        })
    }

    fn LockServer(&self, lock: BOOL) -> Result<()> {
        boundary(|| {
            let mut current = SERVER_LOCKS.load(Ordering::SeqCst);
            loop {
                let next = if lock.as_bool() {
                    current.checked_add(1)
                } else {
                    current.checked_sub(1)
                }
                .ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
                match SERVER_LOCKS.compare_exchange_weak(
                    current,
                    next,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) => return Ok(()),
                    Err(actual) => current = actual,
                }
            }
        })
    }
}

/// Creates the requested class factory without process startup, registration or loader-lock work.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    class: *const GUID,
    iid: *const GUID,
    output: *mut *mut c_void,
) -> HRESULT {
    boundary(|| {
        if output.is_null() {
            return Err(Error::from_hresult(E_POINTER));
        }
        // SAFETY: COM provides writable output storage; clear it before validating IDs.
        unsafe { *output = std::ptr::null_mut() };
        if class.is_null() || iid.is_null() {
            return Err(Error::from_hresult(E_POINTER));
        }
        // SAFETY: COM supplies readable GUIDs for the duration of this call.
        let requested = unsafe { *class };
        let class = if requested == identity_guid(unlinka_integration::CLASSIC_CLSID)? {
            CommandClass::Classic
        } else if requested == identity_guid(unlinka_integration::MODERN_CLSID)? {
            CommandClass::Modern
        } else {
            return Err(Error::from_hresult(CLASS_E_CLASSNOTAVAILABLE));
        };
        let factory: IClassFactory = Factory {
            _lease: ObjectLease::new(),
            class,
        }
        .into();
        // SAFETY: pointers were validated; COM query transfers an interface reference.
        unsafe { factory.query(iid, output).ok() }
    })
    .map_or_else(|error| error.code(), |()| S_OK)
}

/// Factories, instances, explicit server locks and asynchronous work all prevent unloading.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    boundary(|| {
        Ok(
            if OBJECTS.load(Ordering::SeqCst) == 0
                && SERVER_LOCKS.load(Ordering::SeqCst) == 0
                && WORKERS.load(Ordering::SeqCst) == 0
            {
                S_OK
            } else {
                S_FALSE
            },
        )
    })
    .unwrap_or_else(|error| error.code())
}

#[cfg(test)]
mod tests;
