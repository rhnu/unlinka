use std::io;

use super::*;
use crate::Locale;

#[test]
fn complete_paths_keep_local_and_unc_meaning() {
    let local = r"C:\目录\sample.txt";
    assert_eq!(path_text(Path::new(&format!(r"\\?\{local}"))), local);
    assert_eq!(
        path_text(Path::new(r"\\?\UNC\server\share\sample.txt")),
        r"\\server\share\sample.txt"
    );
    assert_eq!(path_text(Path::new("relative.txt")), "relative.txt");
}

#[test]
fn known_windows_errors_are_localized_and_unknown_errors_keep_detail() {
    for locale in [Locale::ZhCn, Locale::EnUs] {
        let tr = Translator::new(locale);
        let sharing = diagnostic(&tr, "system", &io::Error::from_raw_os_error(32));
        let locked = diagnostic(&tr, "system", &io::Error::from_raw_os_error(33));
        let access = diagnostic(&tr, "system", &io::Error::from_raw_os_error(5));
        assert_eq!(sharing, locked);
        assert_ne!(sharing, access);
        assert!(!sharing.contains("32"));
        assert_eq!(
            diagnostic(&tr, "system", &io::Error::other("custom failure")),
            "custom failure"
        );
        let unknown = io::Error::from_raw_os_error(1234);
        assert_eq!(diagnostic(&tr, "system", &unknown), unknown.to_string());
    }
}

#[test]
fn failures_and_cancellation_cannot_be_reported_as_completion() {
    for locale in [Locale::ZhCn, Locale::EnUs] {
        let tr = Translator::new(locale);
        for dry_run in [false, true] {
            let success = outcome(&tr, dry_run, false, 0);
            let partial = outcome(&tr, dry_run, false, 2);
            let cancelled = outcome(&tr, dry_run, true, 0);
            assert_ne!(success, partial);
            assert_ne!(cancelled, partial);
            assert_eq!(outcome(&tr, dry_run, true, 2), cancelled);
        }
    }
}
