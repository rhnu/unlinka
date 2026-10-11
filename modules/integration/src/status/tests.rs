use std::ffi::OsString;

use super::{bootstrap, document, query_command};

#[test]
fn status_arguments_reject_deletion_and_accept_query_options() {
    assert!(
        query_command()
            .try_get_matches_from([
                "unlinka",
                "--integration-status",
                "--json",
                "--lang",
                "zh-CN"
            ])
            .is_ok()
    );
    for argument in ["target", "--recursive", "--yes", "--dry-run"] {
        assert!(
            query_command()
                .try_get_matches_from(["unlinka", "--integration-status", argument])
                .is_err()
        );
    }
    assert!(
        query_command()
            .try_get_matches_from(["unlinka", "--integration-status", "--threads", "1"])
            .is_err()
    );
}

#[test]
fn query_document_matches_versioned_result_contract() {
    let value = document(2, None, Some(("invalid_arguments", "parse_arguments")));
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["outcome"], "invalid_input");
    assert_eq!(value["exit_code"], 2);
    assert_eq!(value["diagnostics"][0]["code"], "invalid_arguments");
    assert!(value.get("integration").is_none());
    let args = [
        OsString::from("--integration-status"),
        OsString::from("--"),
        OsString::from("--lang=zh-CN"),
        OsString::from("--json"),
    ];
    assert_eq!(bootstrap(&args), (None, false));
}
