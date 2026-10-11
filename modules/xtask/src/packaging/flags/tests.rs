use super::*;

#[test]
fn packaging_preserves_encoded_flags_and_forces_static_crt_last() {
    assert_eq!(
        static_crt_flags(
            Some("-C\u{1f}link-arg=/LIBPATH:C:\\My Libraries\u{1f}-Ctarget-feature=-crt-static"),
            Some("--cfg ignored"),
            None
        ),
        "-C\u{1f}link-arg=/LIBPATH:C:\\My Libraries\u{1f}-Ctarget-feature=-crt-static\u{1f}-C\u{1f}target-feature=+crt-static"
    );
}

#[test]
fn packaging_respects_cargo_environment_flag_precedence() {
    assert_eq!(
        static_crt_flags(None, Some("--cfg release_build"), Some("--cfg ignored")),
        "--cfg\u{1f}release_build\u{1f}-C\u{1f}target-feature=+crt-static"
    );
    assert_eq!(
        static_crt_flags(None, None, Some("--cfg target_build")),
        "--cfg\u{1f}target_build\u{1f}-C\u{1f}target-feature=+crt-static"
    );
    assert_eq!(
        static_crt_flags(Some(""), Some("--cfg ignored"), None),
        "-C\u{1f}target-feature=+crt-static"
    );
    assert_eq!(
        static_crt_flags(None, None, None),
        "-C\u{1f}target-feature=+crt-static"
    );
}
