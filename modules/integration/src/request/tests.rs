use super::{MAX_SELECTIONS, ShellRequest, valid_id, validate};

fn request(paths: Vec<Vec<u16>>) -> ShellRequest {
    ShellRequest {
        schema_version: 1,
        action: "permanent_delete".into(),
        paths_utf16: paths,
        locale: "en-US".into(),
    }
}

#[test]
fn protocol_preserves_unpaired_utf16() {
    use std::os::windows::ffi::OsStrExt;
    let mut path: Vec<u16> = "C:\\target\\".encode_utf16().collect();
    path.push(0xD800);
    let bytes = serde_json::to_vec(&request(vec![path.clone()])).unwrap();
    let decoded: ShellRequest = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        decoded.paths().unwrap()[0]
            .as_os_str()
            .encode_wide()
            .collect::<Vec<_>>(),
        path
    );
}

#[test]
fn rejects_incomplete_or_unsafe_entire_selection() {
    for path in ["relative", "C:\\", "", "C:\\a\0b"] {
        assert!(validate(&request(vec![path.encode_utf16().collect()])).is_err());
    }
    let valid = "C:\\target".encode_utf16().collect::<Vec<_>>();
    assert!(validate(&request(vec![valid.clone(); MAX_SELECTIONS])).is_ok());
    assert!(validate(&request(vec![valid; MAX_SELECTIONS + 1])).is_err());
    assert!(serde_json::from_str::<ShellRequest>(r#"{"schema_version":1,"action":"permanent_delete","paths_utf16":[],"locale":"en-US","yes":true}"#).is_err());
}

#[test]
fn guid_is_a_filename_not_a_path() {
    assert!(valid_id("6c273e32-2e9b-450a-a683-4aed2d9d6a04"));
    for bad in [
        "../6c273e32-2e9b-450a-a683-4aed2d9d6a04",
        "{6c273e32-2e9b-450a-a683-4aed2d9d6a04}",
        "6c273e32/2e9b-450a-a683-4aed2d9d6a04",
    ] {
        assert!(!valid_id(bad));
    }
}

#[test]
fn serializing_over_capacity_rejects_the_entire_request() {
    let request = request(vec!["C:\\target".encode_utf16().collect()]);
    let expected = serde_json::to_vec(&request).unwrap();
    assert_eq!(
        super::serialize(&request, expected.len()).unwrap(),
        expected
    );
    assert!(super::serialize(&request, expected.len() - 1).is_err());
}
