use std::{os::windows::ffi::OsStrExt, path::Path};

use super::{eligible_path, path_from_utf16};

#[test]
fn roots_and_relative_paths_are_hidden() {
    for path in [
        r"C:\",
        r"C:",
        r"\folder",
        r"\\server\share\",
        r"\\?\C:\",
        r"\\?\UNC\server\share\",
        r"C:\folder\..",
    ] {
        assert!(!eligible_path(Path::new(path)), "{path}");
    }
}

#[test]
fn selected_links_and_extended_paths_are_kept_as_entries() {
    for path in [
        r"C:\folder\shortcut.lnk",
        r"C:\junction",
        r"\\server\share\folder",
        r"\\?\C:\folder\file",
    ] {
        assert!(eligible_path(Path::new(path)), "{path}");
    }
}

#[test]
fn non_unicode_path_is_not_replaced() {
    let path = path_from_utf16(&[67, 58, 92, 0xD800]).unwrap();
    assert_eq!(
        path.as_os_str().encode_wide().collect::<Vec<_>>(),
        [67, 58, 92, 0xD800]
    );
}
