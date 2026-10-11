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
