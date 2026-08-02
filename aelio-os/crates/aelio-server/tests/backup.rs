use aelio_server::backup::{create_backup, restore_backup, verify_backup};
use std::fs;

#[test]
fn whole_root_backup_verifies_and_restores_exact_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir_all(source.join("runtime/ledger")).unwrap();
    fs::create_dir_all(source.join("database/segments")).unwrap();
    fs::create_dir_all(source.join("agent")).unwrap();
    fs::write(source.join("runtime/ledger/turn.bin"), b"ledger").unwrap();
    fs::write(source.join("database/segments/1.seg"), [0, 1, 2, 3]).unwrap();
    fs::write(source.join("agent/catalog.bin"), b"catalog").unwrap();

    let backup = temp.path().join("backup");
    let manifest = create_backup(&source, &backup).unwrap();
    assert_eq!(manifest.files.len(), 3);
    assert_eq!(verify_backup(&backup).unwrap().files.len(), 3);

    let restored = temp.path().join("restored");
    restore_backup(&backup, &restored).unwrap();
    assert_eq!(
        fs::read(restored.join("runtime/ledger/turn.bin")).unwrap(),
        b"ledger"
    );
    assert_eq!(
        fs::read(restored.join("database/segments/1.seg")).unwrap(),
        [0, 1, 2, 3]
    );
}

#[test]
fn corruption_and_overwrite_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("catalog.bin"), b"original").unwrap();
    let backup = temp.path().join("backup");
    create_backup(&source, &backup).unwrap();
    fs::write(backup.join("data/catalog.bin"), b"tampered").unwrap();
    assert!(verify_backup(&backup).is_err());

    let existing = temp.path().join("existing");
    fs::create_dir(&existing).unwrap();
    assert!(restore_backup(&backup, &existing).is_err());
}

#[test]
fn backup_destination_cannot_be_nested_in_source() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    assert!(create_backup(&source, &source.join("backup")).is_err());
}
