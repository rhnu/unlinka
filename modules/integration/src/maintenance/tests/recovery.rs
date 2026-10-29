use crate::maintenance::{
    Ownership, PathChange, PreferencesDisposition, RegistryChange, SETTINGS_KEY, Transaction,
    Value,
    uninstall::{menu_preference_restore, pending_purge_settings, record_purge_settings},
    user_path::cancel_unwritten_path_edit,
};

fn transaction() -> Transaction {
    let ownership = Ownership {
        schema_version: 1,
        ..Ownership::default()
    };
    Transaction {
        schema_version: 1,
        id: "00000000-0000-0000-0000-000000000001".into(),
        removing: false,
        preferences: PreferencesDisposition::Retain,
        committed: false,
        before_install: None,
        before: ownership.clone(),
        after: ownership,
        registry: Vec::new(),
        files: Vec::new(),
        path: Vec::new(),
        _legacy_package_touched: (),
        artifacts: Vec::new(),
    }
}

#[test]
fn purge_choice_survives_persistence_and_cannot_be_reversed_on_retry() {
    let mut tx = transaction();
    tx.removing = true;
    record_purge_settings(&mut tx, false);
    assert!(!pending_purge_settings(&tx).unwrap());
    record_purge_settings(&mut tx, true);
    let mut restored: Transaction =
        serde_json::from_slice(&serde_json::to_vec(&tx).unwrap()).unwrap();
    record_purge_settings(&mut restored, false);
    assert!(pending_purge_settings(&restored).unwrap());
}

#[test]
fn older_transactions_default_to_preserving_preferences() {
    let mut metadata = serde_json::to_value(transaction()).unwrap();
    metadata.as_object_mut().unwrap().remove("preferences");
    let restored: Transaction = serde_json::from_value(metadata).unwrap();
    assert_eq!(restored.preferences, PreferencesDisposition::Retain);
}

#[test]
fn pending_purge_requires_an_uninstall_and_valid_transaction_ownership() {
    let mut tx = transaction();
    tx.preferences = PreferencesDisposition::Purge;
    assert!(!pending_purge_settings(&tx).unwrap());
    tx.removing = true;
    assert!(pending_purge_settings(&tx).unwrap());
    tx.schema_version = 2;
    assert!(pending_purge_settings(&tx).is_err());
    tx.schema_version = 1;
    tx.id.clear();
    assert!(pending_purge_settings(&tx).is_err());
    tx.id = "00000000-0000-0000-0000-000000000001".into();
    tx.before.schema_version = 2;
    assert!(pending_purge_settings(&tx).is_err());
    tx.before.schema_version = 1;
    tx.after.schema_version = 2;
    assert!(pending_purge_settings(&tx).is_err());
}

#[test]
fn unchanged_disabled_menu_preference_requires_no_restore_write() {
    let unrelated = RegistryChange {
        key: SETTINGS_KEY.into(),
        name: "Language".into(),
        before: Some(Value::string("auto")),
        after: Some(Value::string("en-US")),
    };
    assert!(menu_preference_restore(&[]).is_none());
    assert!(menu_preference_restore(&[unrelated]).is_none());
}

#[test]
fn changed_menu_preference_restores_the_original_value_or_absence() {
    let mut change = RegistryChange {
        key: SETTINGS_KEY.into(),
        name: "MenusEnabled".into(),
        before: Some(Value::number(1)),
        after: Some(Value::number(0)),
    };
    let restoration = menu_preference_restore(std::slice::from_ref(&change)).unwrap();
    assert_eq!(restoration.before, Some(Value::number(1)));
    change.before = None;
    let restoration = menu_preference_restore(std::slice::from_ref(&change)).unwrap();
    assert!(restoration.before.is_none());
}

#[test]
fn rejected_path_addition_removes_its_unwritten_intent_and_ownership() {
    let mut tx = transaction();
    let entry = "C:\\Tools".encode_utf16().collect::<Vec<_>>();
    tx.after.path_owned = Some(entry.clone());
    tx.path.push(PathChange {
        entry,
        added: true,
        original_missing: false,
    });

    cancel_unwritten_path_edit(&mut tx, None, 0);

    assert!(tx.path.is_empty());
    assert!(tx.after.path_owned.is_none());
}

#[test]
fn rejected_path_removal_restores_ownership_and_preserves_earlier_intents() {
    let mut tx = transaction();
    let previous_entry = "C:\\Previous".encode_utf16().collect::<Vec<_>>();
    let owned = "C:\\Tools".encode_utf16().collect::<Vec<_>>();
    tx.path.push(PathChange {
        entry: previous_entry.clone(),
        added: true,
        original_missing: true,
    });
    tx.path.push(PathChange {
        entry: owned.clone(),
        added: false,
        original_missing: false,
    });

    cancel_unwritten_path_edit(&mut tx, Some(owned.clone()), 1);

    assert_eq!(tx.after.path_owned, Some(owned));
    assert_eq!(tx.path.len(), 1);
    assert_eq!(tx.path[0].entry, previous_entry);
    assert!(tx.path[0].added);
    assert!(tx.path[0].original_missing);
}

#[test]
fn unsigned_metadata_reads_existing_ownership_and_transaction_records() {
    let old_ownership = serde_json::json!({
        "schema_version":1,"publisher":"","signed":false,
        "files":{"versions/1.0.0.0/unlinka.exe":"0".repeat(64)},
        "registry":[],"path_owned":null,"package_version":null,"pending":[]
    });
    let owned: Ownership = serde_json::from_value(old_ownership.clone()).unwrap();
    super::super::validate_ownership(&owned).unwrap();
    assert!(!owned.signed);
    assert_eq!(owned.signer_subject, "");
    assert_eq!(owned.files.len(), 1);
    let mut old_transaction = serde_json::to_value(transaction()).unwrap();
    old_transaction["before"] = old_ownership.clone();
    old_transaction["after"] = old_ownership.clone();
    old_transaction["package_touched"] = false.into();
    let recovered: Transaction = serde_json::from_value(old_transaction.clone()).unwrap();
    assert_eq!(recovered.before.files, owned.files);
    assert_eq!(recovered.after.files, owned.files);
    let restored: Transaction =
        serde_json::from_value(serde_json::to_value(&recovered).unwrap()).unwrap();
    assert_eq!(restored.after.files, owned.files);
    old_transaction["package_touched"] = true.into();
    assert!(serde_json::from_value::<Transaction>(old_transaction).is_err());
    let mut invalid_ownership = old_ownership;
    invalid_ownership["package_version"] = "1.0.0.0".into();
    assert!(serde_json::from_value::<Ownership>(invalid_ownership).is_err());
}
