use super::{Payload, PayloadFile, relative, validate_version};

fn test_payload(signed: bool) -> Payload {
    let mut files: Vec<_> = ["unlinka.exe", "unlinka-desktop.exe", "unlinka-shell.dll"]
        .into_iter()
        .map(|path| PayloadFile {
            path: path.into(),
            sha256: "0".repeat(64),
        })
        .collect();
    if signed {
        files.push(PayloadFile {
            path: "Unlinka.ShellIntegration.msix".into(),
            sha256: "0".repeat(64),
        });
    }
    Payload {
        schema_version: 1,
        package_version: "1.1.0.0".into(),
        publisher: if signed {
            "CN=Test".into()
        } else {
            String::new()
        },
        signed,
        protocol_version: 1,
        files,
    }
}

#[test]
fn payload_modes_require_their_own_package_and_publisher_policy() {
    use super::payload::{validate_build_policy, validate_payload_metadata};
    let unsigned = test_payload(false);
    assert!(validate_payload_metadata(&unsigned).is_ok());
    assert!(validate_build_policy(&unsigned, false, Some("stale environment value")).is_ok());
    assert!(validate_build_policy(&unsigned, true, Some("CN=Test")).is_err());
    let mut signed = test_payload(true);
    assert!(validate_payload_metadata(&signed).is_ok());
    assert!(validate_build_policy(&signed, true, Some("CN=Test")).is_ok());
    assert!(validate_build_policy(&signed, false, None).is_err());
    assert!(validate_build_policy(&signed, true, Some("CN=Other")).is_err());
    assert!(validate_build_policy(&signed, true, None).is_err());
    signed.files.pop();
    assert!(validate_payload_metadata(&signed).is_err());
    signed.signed = false;
    signed.publisher.clear();
    signed.files.push(PayloadFile {
        path: "extra.msix".into(),
        sha256: "0".repeat(64),
    });
    assert!(validate_payload_metadata(&signed).is_err());
    let mut unsigned = unsigned;
    unsigned.publisher = "CN=Dev".into();
    assert!(validate_payload_metadata(&unsigned).is_err());
}

#[test]
fn legacy_metadata_keeps_signature_verification_required() {
    use super::Ownership;
    let old: Payload = serde_json::from_value(serde_json::json!({"schema_version":1,"package_version":"1.0.0.0","publisher":"CN=Test","protocol_version":1,"files":[]})).unwrap();
    assert!(old.signed);
    let old: Ownership = serde_json::from_value(serde_json::json!({"schema_version":1,"publisher":"CN=Test","files":{},"registry":[],"path_owned":null,"package_version":null,"pending":[]})).unwrap();
    assert!(old.signed);
    assert!(!Ownership::default().signed);
}

#[test]
fn upgrades_adopt_signed_publisher_and_reverse_mode_only_at_newer_version() {
    use super::{Ownership, validate_upgrade_policy};
    let mut before = Ownership {
        schema_version: 1,
        ..Ownership::default()
    };
    let signed = test_payload(true);
    assert!(validate_upgrade_policy(&before, &signed, "1.0.0.0").is_ok());
    assert!(validate_upgrade_policy(&before, &signed, "1.1.0.0").is_err());
    before.signed = true;
    before.publisher = "CN=Test".into();
    assert!(validate_upgrade_policy(&before, &signed, "1.1.0.0").is_ok());
    before.publisher = "CN=Other".into();
    assert!(validate_upgrade_policy(&before, &signed, "1.0.0.0").is_err());
    let unsigned = test_payload(false);
    assert!(validate_upgrade_policy(&before, &unsigned, "1.0.0.0").is_ok());
    assert!(validate_upgrade_policy(&before, &unsigned, "1.1.0.0").is_err());
    assert!(validate_upgrade_policy(&before, &unsigned, "1.2.0.0").is_err());
}

#[test]
fn package_rollback_uses_the_policy_that_could_have_registered_the_package() {
    use super::{Ownership, Transaction, journal::rollback_package_publisher};
    let unsigned = Ownership {
        schema_version: 1,
        ..Ownership::default()
    };
    let signed = Ownership {
        signed: true,
        publisher: "CN=Original".into(),
        ..unsigned.clone()
    };
    let mut tx = Transaction {
        schema_version: 1,
        id: String::new(),
        removing: false,
        committed: false,
        before_install: None,
        before: signed.clone(),
        after: unsigned.clone(),
        registry: vec![],
        files: vec![],
        path: vec![],
        package_touched: true,
        artifacts: vec![],
    };
    assert_eq!(rollback_package_publisher(&tx).unwrap(), "CN=Original");
    tx.before = unsigned;
    tx.after = signed;
    assert_eq!(rollback_package_publisher(&tx).unwrap(), "CN=Original");
    tx.after.signed = false;
    assert!(rollback_package_publisher(&tx).is_err());
}

#[test]
fn installed_helper_accepts_current_and_journal_owned_recovery_hashes() {
    use super::{FileChange, Ownership, Transaction, diagnostics::installed_helper_hash_owned};
    let helper = "maintenance/unlinka-maintenance.exe";
    let old = "0".repeat(64);
    let new = "1".repeat(64);
    let foreign = "2".repeat(64);
    let mut owned = Ownership {
        schema_version: 1,
        ..Ownership::default()
    };
    owned.files.insert(helper.into(), old.clone());
    assert!(installed_helper_hash_owned(&owned, None, &old).unwrap());
    assert!(!installed_helper_hash_owned(&owned, None, &new).unwrap());
    let mut tx = Transaction {
        schema_version: 1,
        id: "00000000-0000-0000-0000-000000000001".into(),
        removing: false,
        committed: false,
        before_install: None,
        before: owned.clone(),
        after: owned.clone(),
        registry: vec![],
        files: vec![],
        path: vec![],
        package_touched: false,
        artifacts: vec![],
    };
    tx.after.files.insert(helper.into(), new.clone());
    tx.files.push(FileChange {
        path: helper.into(),
        before: Some(old.clone()),
        after: new.clone(),
        backup: Some(format!("state/backups/{}/helper.exe", tx.id)),
    });
    assert!(installed_helper_hash_owned(&owned, Some(&tx), &new).unwrap());
    assert!(!installed_helper_hash_owned(&owned, Some(&tx), &foreign).unwrap());
    // Recovery may have restored the old helper while the ownership record still has the new hash.
    assert!(installed_helper_hash_owned(&tx.after, Some(&tx), &old).unwrap());
    tx.committed = true;
    assert!(installed_helper_hash_owned(&tx.after, Some(&tx), &new).unwrap());
    tx.files.clear();
    assert!(!installed_helper_hash_owned(&owned, Some(&tx), &new).unwrap());
}

#[test]
fn installed_helper_rejects_invalid_or_foreign_transaction_metadata() {
    use super::{FileChange, Ownership, Transaction, diagnostics::installed_helper_hash_owned};
    let helper = "maintenance/unlinka-maintenance.exe";
    let actual = "1".repeat(64);
    let owned = Ownership {
        schema_version: 1,
        ..Ownership::default()
    };
    let mut tx = Transaction {
        schema_version: 1,
        id: "00000000-0000-0000-0000-000000000001".into(),
        removing: false,
        committed: false,
        before_install: None,
        before: owned.clone(),
        after: owned.clone(),
        registry: vec![],
        files: vec![],
        path: vec![],
        package_touched: false,
        artifacts: vec![],
    };
    tx.after.files.insert(helper.into(), actual.clone());
    tx.files.push(FileChange {
        path: helper.into(),
        before: None,
        after: actual.clone(),
        backup: None,
    });
    tx.schema_version = 2;
    assert!(installed_helper_hash_owned(&owned, Some(&tx), &actual).is_err());
    tx.schema_version = 1;
    tx.id = "foreign".into();
    assert!(installed_helper_hash_owned(&owned, Some(&tx), &actual).is_err());
    tx.id = "00000000-0000-0000-0000-000000000001".into();
    tx.files[0].path = "maintenance/foreign.exe".into();
    assert!(installed_helper_hash_owned(&owned, Some(&tx), &actual).is_err());
    tx.files[0].path = helper.into();
    tx.files[0].backup = Some("state/backups/foreign/helper.exe".into());
    assert!(installed_helper_hash_owned(&owned, Some(&tx), &actual).is_err());
    tx.files[0].backup = None;
    tx.after.publisher = "CN=Foreign".into();
    assert!(installed_helper_hash_owned(&owned, Some(&tx), &actual).is_err());
}

#[test]
fn versions_are_ordered_numerically_and_paths_cannot_escape() {
    assert!(validate_version("1.10.0.0").unwrap() > validate_version("1.2.65535.0").unwrap());
    for bad in ["1.2.3", "1.2.3.65536", "1.02.3.4", "../1.0.0.0", "1.0.0.-1"] {
        assert!(validate_version(bad).is_err());
    }
    for bad in [
        "../unlinka.exe",
        "C:\\file",
        "nested/../../file",
        "file:stream",
        "a/b.",
    ] {
        assert!(relative(bad).is_err());
    }
    assert!(relative("Assets/menu.png").is_ok());
}

#[test]
fn payload_rejects_unknown_metadata() {
    assert!(serde_json::from_str::<Payload>(r#"{"schema_version":1,"package_version":"1.0.0.0","publisher":"CN=Test","protocol_version":1,"files":[],"command":"run"}"#).is_err());
    assert!(
        serde_json::from_str::<PayloadFile>(r#"{"path":"a","sha256":"b","execute":true}"#).is_err()
    );
}

#[test]
fn path_removal_and_rollback_preserve_external_edits_and_preexisting_entries() {
    use super::user_path::{plan_path, reverse_path_tokens};
    let units = |s: &str| s.encode_utf16().collect::<Vec<_>>();
    let match_path = |a: &[u16], b: &[u16]| Ok(a == b);
    let bin = units("C:\\Unlinka\\bin");
    let first = units("C:\\Tools");
    let external = units("C:\\Later");
    let (tokens, owned, change) = plan_path(
        vec![first.clone(), bin.clone()],
        &bin,
        None,
        true,
        match_path,
    )
    .unwrap();
    assert_eq!(tokens, vec![first.clone(), bin.clone()]);
    assert!(owned.is_none());
    assert!(change.is_none());
    let (mut tokens, owned, change) =
        plan_path(vec![first.clone()], &bin, None, true, match_path).unwrap();
    assert_eq!(owned, Some(bin.clone()));
    assert_eq!(change, Some((bin.clone(), true)));
    tokens.insert(1, external.clone());
    let (tokens, owned, _) = plan_path(tokens, &bin, Some(&bin), false, match_path).unwrap();
    assert_eq!(tokens, vec![first.clone(), external.clone()]);
    assert!(owned.is_none());
    let mut tokens = vec![first, external];
    reverse_path_tokens(&mut tokens, &bin, false);
    assert_eq!(tokens.last(), Some(&bin));
    reverse_path_tokens(&mut tokens, &bin, true);
    assert_eq!(tokens.len(), 2);
}

#[test]
fn externally_changed_owned_path_is_not_removed() {
    use super::user_path::plan_path;
    let bin = "C:\\Unlinka\\bin".encode_utf16().collect::<Vec<_>>();
    let changed = "c:\\unlinka\\BIN".encode_utf16().collect::<Vec<_>>();
    assert!(plan_path(vec![changed], &bin, Some(&bin), false, |_, _| Ok(true)).is_err());
}

#[test]
fn compensation_restores_only_our_value_and_is_idempotent() {
    use super::{Value, compensation};
    let old = Some(Value::string("old"));
    let new = Some(Value::string("new"));
    assert_eq!(
        compensation(new.as_ref(), old.as_ref(), new.as_ref()).unwrap(),
        old
    );
    assert_eq!(
        compensation(old.as_ref(), old.as_ref(), new.as_ref()).unwrap(),
        old
    );
    assert!(
        compensation(
            Some(&Value::string("user edit")),
            old.as_ref(),
            new.as_ref()
        )
        .is_err()
    );
    assert_eq!(
        compensation(new.as_ref(), None, new.as_ref()).unwrap(),
        None
    );
}

#[test]
fn ownership_validation_rejects_unknown_schema_and_paths() {
    use super::{Ownership, validate_ownership};
    let mut ownership = Ownership {
        schema_version: 1,
        ..Ownership::default()
    };
    ownership
        .files
        .insert("versions/1.0.0.0/unlinka-shell.dll".into(), "0".repeat(64));
    assert!(validate_ownership(&ownership).is_ok());
    ownership
        .files
        .insert("versions/1.0.0.0/../../foreign.txt".into(), "0".repeat(64));
    assert!(validate_ownership(&ownership).is_err());
    ownership.files.remove("versions/1.0.0.0/../../foreign.txt");
    ownership.pending.push("foreign.txt".into());
    assert!(validate_ownership(&ownership).is_err());
    ownership.pending.clear();
    ownership.schema_version = 2;
    assert!(validate_ownership(&ownership).is_err());
}
