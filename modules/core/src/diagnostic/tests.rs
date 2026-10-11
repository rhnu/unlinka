use super::*;

#[test]
fn domain_codes_are_independent_of_display_text() {
    let failure = error(DiagnosticCode::IdentityChanged, "custom description");
    assert_eq!(diagnostic_code(&failure), DiagnosticCode::IdentityChanged);
    assert_eq!(failure.raw_os_error(), None);
}

#[test]
fn system_error_keeps_os_code() {
    let failure = io::Error::from_raw_os_error(5);
    assert_eq!(diagnostic_code(&failure), DiagnosticCode::System);
    assert_eq!(failure.raw_os_error(), Some(5));
}
