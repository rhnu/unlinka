use super::*;

#[test]
fn counts_use_json_instead_of_localized_terminal_text() {
    let text = "删除未完成\n{\"schema_version\":1,\"files\":2,\"directories\":3,\"links\":4,\"failed\":5}\n诊断详情";
    assert_eq!(
        counts(text).unwrap(),
        Counts {
            files: 2,
            directories: 3,
            links: 4,
            failed: 5
        }
    );
}

#[test]
fn invalid_or_unsupported_results_are_not_treated_as_zero_counts() {
    for text in [
        "Deletion complete",
        "{invalid}",
        r#"{"schema_version":2,"files":1,"directories":0,"links":0,"failed":0}"#,
        r#"{"schema_version":1,"files":1,"directories":0,"links":0}"#,
        r#"{"schema_version":1,"files":-1,"directories":0,"links":0,"failed":0}"#,
    ] {
        assert!(counts(text).is_err(), "{text}");
    }
}
