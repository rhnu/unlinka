use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf};

use super::*;

#[test]
fn json_preserves_raw_utf16_and_stable_result_fields() {
    let path = PathBuf::from(OsString::from_wide(&[0x43, 0x3A, 0x5C, 0xD800]));
    let document = ResultDocument::new(ExitStatus::Cancelled, false, &[path]);
    let value = serde_json::to_value(document).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["paths_utf16"][0][3], 0xD800);
    assert_eq!(value["exit_code"], 3);
    assert_eq!(value["cancelled"], true);
}

#[test]
fn default_failures_are_bounded_but_verbose_retains_paths_and_codes() {
    let tr = Translator::new(unlinka_i18n::Locale::EnUs);
    let report = DeleteReport {
        files: 2,
        failed: 9,
        failures: (0..7)
            .map(|index| unlinka_core::Failure {
                path: PathBuf::from(format!(r"\\?\C:\fixture\locked-{index}.txt")),
                operation: "open",
                error: std::io::Error::from_raw_os_error(32),
            })
            .collect(),
        ..DeleteReport::default()
    };
    let normal = report_text(&report, false, false, &tr);
    assert!(normal.starts_with("Deletion incomplete\n"));
    assert!(normal.contains("9 failures"));
    assert!(normal.contains(r"C:\fixture\locked-4.txt"));
    assert!(!normal.contains("locked-5.txt"));
    assert!(!normal.contains(r"\\?\"));
    assert!(!normal.contains("Windows error"));
    assert!(normal.contains("2 more failures"));
    assert!(normal.contains("2 additional failures omitted"));
    let verbose = report_text(&report, false, true, &tr);
    assert!(verbose.contains("locked-6.txt"));
    assert!(verbose.contains("Windows error 32"));
    assert!(verbose.contains("open · system"));
    assert!(!verbose.contains("use -v"));
}

#[test]
fn cancelled_dry_run_keeps_failures_and_found_counts() {
    let report = DeleteReport {
        files: 4,
        cancelled: true,
        failed: 1,
        ..DeleteReport::default()
    };
    let text = report_text(
        &report,
        true,
        false,
        &Translator::new(unlinka_i18n::Locale::EnUs),
    );
    assert!(text.starts_with("Scan cancelled\nFound: 4 files"));
    assert!(text.contains("Nothing was deleted."));
    assert!(text.contains("1 failure"));
    assert!(text.contains("One additional failure omitted"));
}

#[test]
fn compact_human_output_does_not_truncate_json_diagnostics() {
    let report = DeleteReport {
        failed: 6,
        failures: (0..6)
            .map(|index| unlinka_core::Failure {
                path: PathBuf::from(format!("file-{index}.txt")),
                operation: "delete",
                error: std::io::Error::from_raw_os_error(5),
            })
            .collect(),
        ..DeleteReport::default()
    };
    let document = ResultDocument::from_report(&report, false, &[], false);
    let value = serde_json::to_value(document).unwrap();
    assert_eq!(value["diagnostics"].as_array().unwrap().len(), 6);
    assert_eq!(value["diagnostics"][5]["os_error"], 5);
    assert_eq!(value["exit_code"], 1);
}
