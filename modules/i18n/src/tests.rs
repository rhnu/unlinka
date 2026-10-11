use super::*;

#[test]
fn explicit_auto_bypasses_environment_and_saved_language() {
    assert_eq!(
        select_locale(
            Some("auto"),
            Some("zh-CN"),
            Some("zh-CN"),
            &["en-GB".into()]
        ),
        Locale::EnUs
    );
}

#[test]
fn precedence_and_traditional_chinese_fallback() {
    assert_eq!(
        select_locale(Some("en-US"), Some("zh-CN"), None, &[]),
        Locale::EnUs
    );
    assert_eq!(
        select_locale(None, Some("zh-CN"), Some("en-US"), &[]),
        Locale::ZhCn
    );
    assert_eq!(
        select_locale(
            None,
            None,
            Some("auto"),
            &["zh-TW".into(), "zh-Hant".into(), "zh-SG".into()]
        ),
        Locale::ZhCn
    );
    assert_eq!(
        select_locale(None, None, None, &["zh-Hant-CN".into()]),
        Locale::EnUs
    );
}

#[test]
fn embedded_messages_parse_and_pluralize() {
    for locale in [Locale::EnUs, Locale::ZhCn] {
        let translator = Translator::new(locale);
        assert_ne!(translator.text("menu-delete"), "menu-delete");
        assert_ne!(translator.count("failures-omitted", 2), "failures-omitted");
    }
    assert!(
        Translator::new(Locale::EnUs)
            .count("failures-omitted", 1)
            .contains("One")
    );
}

#[test]
fn report_plural_selection_uses_numeric_arguments() {
    let translator = Translator::new(Locale::EnUs);
    let text = translator.format(
        "report",
        &[
            ("mode", "Deletion".into()),
            ("state", "finished".into()),
            ("seconds", "0.001".into()),
            ("files", 1u64.into()),
            ("directories", 2u64.into()),
            ("links", 0u64.into()),
            ("failed", 1u64.into()),
        ],
    );
    assert!(text.contains("1 file,"));
    assert!(text.contains("2 directories,"));
    assert!(text.contains("1 failure."));
}
