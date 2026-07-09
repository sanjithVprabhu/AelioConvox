mod common;

use common::{sample_meta, TempPath};
use ll_format::{
    read_file, write_file, write_file_with, Column, ColumnValues, FormatError, WriteOptions,
};

fn ids(n: u64) -> Vec<u64> {
    (0..n).map(|i| 1000 + i).collect()
}

fn col(column_id: u32, is_system: bool, values: ColumnValues) -> Column {
    Column {
        column_id,
        is_system,
        values,
    }
}

#[test]
fn roundtrip_all_scalar_types() {
    let tmp = TempPath::new("cols");
    let n = 5u64;
    let meta = sample_meta(n);
    let columns = vec![
        col(1, false, ColumnValues::I64(vec![Some(1), Some(2), Some(3), Some(-7), Some(100)])),
        col(
            2,
            false,
            ColumnValues::Utf8(vec![
                Some("alpha".into()),
                Some("beta".into()),
                Some("gamma".into()),
                Some("delta".into()),
                Some("epsilon".into()),
            ]),
        ),
        col(
            3,
            false,
            ColumnValues::Bool(vec![Some(true), Some(false), Some(true), Some(true), Some(false)]),
        ),
        col(
            4,
            false,
            ColumnValues::F64(vec![Some(1.5), Some(2.5), Some(-3.25), Some(0.0), Some(42.0)]),
        ),
        col(
            5,
            true,
            ColumnValues::TimestampNanos(vec![
                Some(1000),
                Some(2000),
                Some(3000),
                Some(4000),
                Some(5000),
            ]),
        ),
        col(6, false, ColumnValues::I32(vec![Some(-1), Some(0), Some(1), Some(2), Some(3)])),
        col(
            7,
            false,
            ColumnValues::F32(vec![Some(1.0), Some(2.0), Some(3.0), Some(4.0), Some(5.0)]),
        ),
    ];

    write_file(&tmp.0, &meta, &columns, &ids(n)).expect("write");
    let file = read_file(&tmp.0).expect("read");

    assert_eq!(file.columns, columns);
    assert!(file.columns[4].is_system, "system flag preserved");
    assert!(!file.columns[0].is_system);
}

#[test]
fn roundtrip_with_nulls() {
    let tmp = TempPath::new("nulls");
    let n = 4u64;
    let meta = sample_meta(n);
    let columns = vec![
        col(1, false, ColumnValues::I64(vec![Some(5), None, Some(7), None])),
        col(
            2,
            false,
            ColumnValues::Utf8(vec![None, Some("x".into()), None, Some("yy".into())]),
        ),
    ];

    write_file(&tmp.0, &meta, &columns, &ids(n)).expect("write");
    let file = read_file(&tmp.0).expect("read");
    assert_eq!(file.columns, columns);
}

#[test]
fn multipage_roundtrip_and_zone_map_count() {
    let tmp = TempPath::new("multipage");
    let n = 20u64;
    let meta = sample_meta(n);
    let values = ColumnValues::I64((0..20).map(|i| Some(i as i64 * 2)).collect());
    let columns = vec![col(9, false, values.clone())];

    write_file_with(&tmp.0, &meta, &columns, &ids(n), &[], &WriteOptions { rows_per_page: 7 })
        .expect("write");
    let file = read_file(&tmp.0).expect("read");

    assert_eq!(file.columns[0].values, values);
    let zms: Vec<_> = file.footer.zone_maps.iter().filter(|z| z.column_id == 9).collect();
    assert_eq!(zms.len(), 3, "ceil(20/7) pages → 3 zone maps");
}

#[test]
fn zone_map_minmax_and_null_count() {
    let tmp = TempPath::new("zonemap");
    let n = 5u64;
    let meta = sample_meta(n);
    let columns = vec![col(
        3,
        false,
        ColumnValues::I64(vec![Some(5), Some(-3), Some(10), None, Some(2)]),
    )];

    write_file(&tmp.0, &meta, &columns, &ids(n)).expect("write");
    let file = read_file(&tmp.0).expect("read");

    let z = file.footer.zone_maps.iter().find(|z| z.column_id == 3).unwrap();
    assert_eq!(i64::from_le_bytes(z.min.clone().try_into().unwrap()), -3);
    assert_eq!(i64::from_le_bytes(z.max.clone().try_into().unwrap()), 10);
    assert_eq!(z.null_count, 1);
    assert_eq!(z.total, 5);
}

#[test]
fn detects_column_page_corruption_via_section_crc() {
    let tmp = TempPath::new("colcorrupt");
    let n = 10u64;
    let meta = sample_meta(n);
    let columns = vec![col(1, false, ColumnValues::I64((0..10).map(Some).collect()))];

    write_file(&tmp.0, &meta, &columns, &ids(n)).expect("write");

    let mut bytes = std::fs::read(&tmp.0).expect("read raw");
    // Column chunk starts at file offset 64 (after preamble); header(50)+page header(16) → payload.
    bytes[64 + 50 + 16 + 3] ^= 0xFF;

    let dst = TempPath::new("colcorrupt_bad");
    std::fs::write(&dst.0, &bytes).expect("write corrupted");

    // The whole-file reader validates the section CRC before touching pages.
    let err = read_file(&dst.0).expect_err("corruption must be detected");
    assert!(matches!(err, FormatError::SectionCrcMismatch { .. }), "{err:?}");
}
