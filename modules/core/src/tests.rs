use std::{
    ffi::OsString,
    io,
    os::windows::ffi::OsStringExt,
    path::{Path, PathBuf},
};

use crate::{
    DeleteReport,
    paths::{contains_path, redundant, validate_syntax},
    win::{Kind, unsupported},
};

#[test]
fn reject_devices_and_streams_before_lookup() {
    for path in [
        r"\\.\PhysicalDrive0",
        r"\\?\GLOBALROOT\Device\HarddiskVolume1",
        r"C:\file:stream",
        r"C:relative",
    ] {
        assert!(validate_syntax(Path::new(path)).is_err(), "{path}");
    }
    for path in [
        r"C:\folder\file",
        r"\\?\C:\folder\file",
        r"\\server\share\file",
        r"\\?\UNC\server\share\file",
        "relative.txt",
    ] {
        assert!(validate_syntax(Path::new(path)).is_ok(), "{path}");
    }
}

#[test]
fn ancestor_comparison_has_component_boundaries_and_normalizes_prefixes() {
    assert!(contains_path(
        Path::new(r"C:\Tree"),
        Path::new(r"\\?\C:\tree\child")
    ));
    assert!(!contains_path(
        Path::new(r"C:\tree"),
        Path::new(r"C:\tree-other")
    ));
    assert!(contains_path(
        Path::new(r"\\server\share\tree"),
        Path::new(r"\\?\UNC\server\share\tree\child")
    ));
}

#[test]
fn failure_retention_does_not_truncate_the_total() {
    let mut report = DeleteReport::default();
    for _ in 0..130 {
        report.fail(
            "synthetic".into(),
            "delete",
            io::Error::other("synthetic failure"),
        );
    }
    assert_eq!(report.failed, 130);
    assert_eq!(report.failures.len(), 100);
}

#[test]
fn disposition_fallback_excludes_access_and_sharing_failures() {
    for code in [1, 50, 87] {
        assert!(unsupported(&io::Error::from_raw_os_error(code)));
    }
    for code in [5, 32, 145] {
        assert!(!unsupported(&io::Error::from_raw_os_error(code)));
    }
}

#[test]
fn redundant_targets_preserve_case_and_lossless_os_names() {
    assert!(redundant(
        Path::new(r"C:\tree"),
        Kind::Directory,
        Path::new(r"C:\tree\child")
    ));
    assert!(!redundant(
        Path::new(r"C:\FOO"),
        Kind::Directory,
        Path::new(r"C:\foo\child")
    ));
    assert!(!redundant(
        Path::new(r"C:\file"),
        Kind::File,
        Path::new(r"C:\file\child")
    ));
    let first = PathBuf::from(OsString::from_wide(&[0xD800]));
    let second = PathBuf::from(OsString::from_wide(&[0xD801]));
    assert!(!redundant(&first, Kind::File, &second));
}
