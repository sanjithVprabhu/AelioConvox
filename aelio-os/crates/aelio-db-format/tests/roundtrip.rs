mod common;

use aelio_db_format::{read_file, write_file, SectionType};
use common::{sample_meta, TempPath};

#[test]
fn roundtrip_with_rows() {
    let tmp = TempPath::new("roundtrip");
    let meta = sample_meta(5);
    let translation: Vec<u64> = vec![100, 101, 102, 103, 104];

    write_file(&tmp.0, &meta, &[], &translation).expect("write");
    let file = read_file(&tmp.0).expect("read");

    assert!(file.columns.is_empty());

    // Preamble round-trips.
    assert_eq!(file.preamble.file_uuid, meta.file_uuid);
    assert_eq!(file.preamble.min_lsn, meta.min_lsn);
    assert_eq!(file.preamble.max_lsn, meta.max_lsn);
    assert_eq!(file.preamble.row_count, 5);
    assert_eq!(file.preamble.schema_fingerprint, meta.schema_fingerprint);
    assert_eq!(file.preamble.creation_unix_nanos, meta.creation_unix_nanos);

    // Footer round-trips.
    assert_eq!(file.footer.row_count, 5);
    assert_eq!(file.footer.schema_blob, meta.schema_blob);
    assert_eq!(file.footer.writer_version, meta.writer_version);
    assert_eq!(file.footer.mvcc, meta.mvcc);

    // Translation table round-trips and is reachable via the section directory.
    assert_eq!(file.translation_table, translation);
    let entry = file.footer.section(SectionType::TranslationTable).unwrap();
    assert_eq!(entry.length, 5 * 8);
    assert_eq!(entry.offset, aelio_db_format::PREAMBLE_LEN as u64);
}

#[test]
fn roundtrip_empty() {
    let tmp = TempPath::new("empty");
    let meta = sample_meta(0);

    write_file(&tmp.0, &meta, &[], &[]).expect("write");
    let file = read_file(&tmp.0).expect("read");

    assert_eq!(file.preamble.row_count, 0);
    assert!(file.translation_table.is_empty());
    let entry = file.footer.section(SectionType::TranslationTable).unwrap();
    assert_eq!(entry.length, 0);
}

#[test]
fn rejects_translation_length_mismatch() {
    let tmp = TempPath::new("mismatch");
    let meta = sample_meta(3);
    // Only 2 entries for a row_count of 3.
    let err = write_file(&tmp.0, &meta, &[], &[1, 2]).unwrap_err();
    assert!(matches!(err, aelio_db_format::FormatError::Decode(_)));
}
