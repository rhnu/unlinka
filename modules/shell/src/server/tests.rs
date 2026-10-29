use std::mem::size_of;

use windows::{
    Win32::UI::Shell::{CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX},
    core::{PCSTR, PCWSTR, PSTR},
};

use super::{CMIC_MASK_UNICODE, VERB, validate_invocation, wide, write_command_string};

#[test]
fn classic_factory_preserves_identity_and_unload_lifetimes() {
    use windows::{
        Win32::{
            Foundation::{CLASS_E_CLASSNOTAVAILABLE, S_FALSE, S_OK},
            System::Com::IClassFactory,
            UI::Shell::{IContextMenu, IShellExtInit},
        },
        core::{GUID, IUnknown, Interface},
    };

    use super::{DllCanUnloadNow, DllGetClassObject, identity_guid};

    assert_eq!(DllCanUnloadNow(), S_OK);
    let mut output = std::ptr::null_mut();
    let missing = GUID::zeroed();
    let factory_iid = IClassFactory::IID;
    // These calls exercise authored objects without COM activation or registration.
    // SAFETY: readable GUIDs and writable output storage remain live throughout the call.
    let status =
        unsafe { DllGetClassObject(&raw const missing, &raw const factory_iid, &raw mut output) };
    assert_eq!(status, CLASS_E_CLASSNOTAVAILABLE);
    assert!(output.is_null());
    let classic = identity_guid(unlinka_integration::CLASSIC_CLSID).unwrap();
    // SAFETY: valid GUIDs and output storage remain live; success transfers a factory reference.
    let status =
        unsafe { DllGetClassObject(&raw const classic, &raw const factory_iid, &raw mut output) };
    assert_eq!(status, S_OK);
    // SAFETY: the successful call transferred exactly one owned factory interface reference.
    let factory = unsafe { IClassFactory::from_raw(output) };
    assert_eq!(DllCanUnloadNow(), S_FALSE);
    // SAFETY: this authored factory supports the requested interface and no aggregation.
    let menu: IContextMenu = unsafe { factory.CreateInstance(None) }.unwrap();
    let init: IShellExtInit = menu.cast().unwrap();
    let outer: IUnknown = factory.cast().unwrap();
    // SAFETY: the live outer interface deliberately tests the factory's aggregation rejection.
    assert!(unsafe { factory.CreateInstance::<_, IContextMenu>(&outer) }.is_err());
    drop(outer);
    // SAFETY: the live factory owns a balanced server-lock request.
    unsafe { factory.LockServer(true) }.unwrap();
    drop(init);
    drop(menu);
    assert_eq!(DllCanUnloadNow(), S_FALSE);
    // SAFETY: release the preceding lock through its still-live factory.
    unsafe { factory.LockServer(false) }.unwrap();
    // SAFETY: an unbalanced unlock must fail without changing server state.
    assert!(unsafe { factory.LockServer(false) }.is_err());
    drop(factory);
    assert_eq!(DllCanUnloadNow(), S_OK);
}

#[test]
fn numeric_offsets_accept_only_the_added_command() {
    let mut info = CMINVOKECOMMANDINFOEX {
        cbSize: u32::try_from(size_of::<CMINVOKECOMMANDINFOEX>()).unwrap(),
        fMask: CMIC_MASK_UNICODE,
        ..Default::default()
    };
    // SAFETY: the complete initialized structure uses the numeric zero verb; no string is read.
    assert!(unsafe { validate_invocation((&raw const info).cast()) }.is_ok());
    info.lpVerb = PCSTR(std::ptr::dangling::<u8>());
    // SAFETY: numeric resource one is rejected without dereferencing its address.
    assert!(unsafe { validate_invocation((&raw const info).cast()) }.is_err());
    // SAFETY: null is explicitly rejected before any structure access.
    assert!(unsafe { validate_invocation(std::ptr::null()) }.is_err());
}

#[test]
fn canonical_verbs_require_the_complete_name() {
    let expected = wide(VERB);
    let ansi = b"Unlinka.PermanentDelete\0";
    let invalid = wide("Unlinka.PermanentDeleteExtra");
    let mut info = CMINVOKECOMMANDINFOEX {
        cbSize: u32::try_from(size_of::<CMINVOKECOMMANDINFOEX>()).unwrap(),
        fMask: CMIC_MASK_UNICODE,
        lpVerb: PCSTR(ansi.as_ptr()),
        lpVerbW: PCWSTR(expected.as_ptr()),
        ..Default::default()
    };
    // SAFETY: structure and terminated string storage remain live during the bounded comparison.
    assert!(unsafe { validate_invocation((&raw const info).cast()) }.is_ok());
    info.lpVerbW = PCWSTR(invalid.as_ptr());
    // SAFETY: the longer invalid string remains live and is rejected at the first mismatch.
    assert!(unsafe { validate_invocation((&raw const info).cast()) }.is_err());
    info.fMask = 0;
    // SAFETY: the terminated ANSI string remains live; this call selects the ANSI layout.
    assert!(unsafe { validate_invocation((&raw const info).cast()) }.is_ok());
    info.cbSize = u32::try_from(size_of::<CMINVOKECOMMANDINFO>()).unwrap() - 1;
    // SAFETY: the actual complete storage is readable, despite deliberately invalid reported size.
    assert!(unsafe { validate_invocation((&raw const info).cast()) }.is_err());
}

#[test]
fn command_buffers_reserve_a_terminator_and_preserve_surrounding_memory() {
    let mut bytes = [99_u8; 5];
    let mut units = [99_u16; 5];
    // SAFETY: the initialized byte buffer is larger than the supplied three-byte capacity.
    unsafe { write_command_string(PSTR(bytes.as_mut_ptr()), 3, false, "abcd") }.unwrap();
    // SAFETY: aligned initialized UTF-16 storage is larger than the supplied three-unit capacity.
    unsafe { write_command_string(PSTR(units.as_mut_ptr().cast()), 3, true, "删除文件") }.unwrap();
    assert_eq!(bytes, [b'a', b'b', 0, 99, 99]);
    assert_eq!(units, [0x5220, 0x9664, 0, 99, 99]);
}
