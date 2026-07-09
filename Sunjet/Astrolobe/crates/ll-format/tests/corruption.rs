mod common;

use common::{sample_meta, TempPath};
use ll_format::{read_file, write_file, FormatError, PREAMBLE_LEN, TRAILER_LEN};

/// Write a valid file, flip one byte at `mutate(len)`, and return the read error.
fn corrupt_and_read(tag: &str, rows: u64, translation: &[u64], mutate: impl Fn(usize) -> usize) -> FormatError {
    let src = TempPath::new(tag);
    let meta = sample_meta(rows);
    write_file(&src.0, &meta, &[], translation).expect("write");

    let mut bytes = std::fs::read(&src.0).expect("read raw");
    let idx = mutate(bytes.len());
    bytes[idx] ^= 0xFF;

    let dst = TempPath::new(&format!("{tag}_bad"));
    std::fs::write(&dst.0, &bytes).expect("write corrupted");

    read_file(&dst.0).expect_err("expected corruption to be detected")
}

#[test]
fn detects_bad_end_magic() {
    // Last byte of the file is part of end_magic.
    let err = corrupt_and_read("magic", 4, &[10, 11, 12, 13], |len| len - 1);
    assert!(matches!(err, FormatError::BadMagic { .. }), "got {err:?}");
}

#[test]
fn detects_footer_corruption() {
    // The byte immediately before the trailer is the last byte of the footer blob.
    let err = corrupt_and_read("footer", 4, &[10, 11, 12, 13], |len| len - TRAILER_LEN - 1);
    assert!(
        matches!(err, FormatError::FooterCrcMismatch { .. }),
        "got {err:?}"
    );
}

#[test]
fn detects_translation_section_corruption() {
    // The TranslationTable section body starts right after the preamble.
    let err = corrupt_and_read("section", 4, &[10, 11, 12, 13], |_len| PREAMBLE_LEN);
    assert!(
        matches!(err, FormatError::SectionCrcMismatch { .. }),
        "got {err:?}"
    );
}
