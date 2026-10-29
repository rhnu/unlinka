use super::*;

#[test]
fn tool_paths_preserve_relative_hierarchy_and_ignore_package_metadata() {
    assert_eq!(
        tool_path("tools/Languages/ChineseSimplified.isl").unwrap(),
        Some(PathBuf::from("Languages/ChineseSimplified.isl"))
    );
    assert_eq!(tool_path("Tools.InnoSetup.nuspec").unwrap(), None);
}

#[test]
fn tool_paths_reject_traversal_windows_prefixes_and_alternate_streams() {
    for name in [
        "tools/../ISCC.exe",
        "tools/./ISCC.exe",
        "tools//ISCC.exe",
        "tools/C:/ISCC.exe",
        "tools/ISCC.exe:payload",
        "tools/..\\ISCC.exe",
        "tools/",
        "tools/ISCC\0.exe",
    ] {
        assert_eq!(
            tool_path(name).unwrap_err().kind(),
            io::ErrorKind::InvalidData,
            "{name:?}"
        );
    }
}

#[test]
fn fingerprints_detect_same_length_content_changes() {
    let original = fingerprint(Cursor::new(b"compiler-a")).unwrap();
    let changed = fingerprint(Cursor::new(b"compiler-b")).unwrap();
    assert_eq!(original.bytes, changed.bytes);
    assert_ne!(original, changed);
    assert_eq!(original, fingerprint(Cursor::new(b"compiler-a")).unwrap());
}

#[test]
fn archive_hash_mismatch_is_invalid_data() {
    assert_eq!(
        verify_archive(b"untrusted compiler").unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}
