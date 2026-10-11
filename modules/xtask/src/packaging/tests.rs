use super::*;

fn options() -> Options {
    Options {
        publisher: None,
        certificate_thumbprint: None,
        timestamp_url: None,
        build_number: 0,
        iscc: None,
        sdk_bin: None,
    }
}

#[test]
fn packaging_defaults_to_unsigned_and_requires_complete_signing_options() {
    for mask in 0..8 {
        let mut candidate = options();
        candidate.publisher = (mask & 1 != 0).then(|| "CN=Example".into());
        candidate.certificate_thumbprint = (mask & 2 != 0).then(|| "a".repeat(40));
        candidate.timestamp_url = (mask & 4 != 0).then(|| "https://example.test/timestamp".into());
        assert_eq!(validate_options(&candidate).is_ok(), mask == 0 || mask == 7);
        if mask == 0 {
            assert!(!candidate.signed());
            assert_eq!(candidate.publisher(), "");
        } else if mask == 7 {
            assert!(candidate.signed());
        }
    }
}

#[test]
fn signing_rejects_invalid_certificate_metadata() {
    let mut candidate = options();
    candidate.publisher = Some("CN=Example".into());
    candidate.certificate_thumbprint = Some("a".repeat(40));
    candidate.timestamp_url = Some("https://example.test/timestamp".into());
    for bad in ["", "CN=Example\nInjected", "\0"] {
        candidate.publisher = Some(bad.into());
        assert!(validate_options(&candidate).is_err());
    }
    candidate.publisher = Some("CN=Example".into());
    for bad in ["", "a", "g000000000000000000000000000000000000000"] {
        candidate.certificate_thumbprint = Some(bad.into());
        assert!(validate_options(&candidate).is_err());
    }
    candidate.certificate_thumbprint = Some("a".repeat(40));
    for bad in ["", "file:///timestamp", "https://example.test/ bad"] {
        candidate.timestamp_url = Some(bad.into());
        assert!(validate_options(&candidate).is_err());
    }
}

#[test]
fn version_components_are_bounded_and_prereleases_rejected() {
    assert_eq!(package_version("2.7.3", 12).unwrap(), "2.7.3.12");
    assert!(package_version("1.0.0-beta", 0).is_err());
    assert!(package_version("70000.0.0", 0).is_err());
}

#[test]
fn manifest_attributes_escape_publisher_distinguished_name() {
    assert_eq!(
        xml("CN=A&B, O=\"Example\""),
        "CN=A&amp;B, O=&quot;Example&quot;"
    );
}
