mod common;

use common::{sample_meta, TempPath};
use ll_format::{
    read_file, write_file, write_file_with, Column, ColumnValues, FormatError, WriteOptions,
};

fn ids(n: u64) -> Vec<u64> {
    (0..n).map(|i| 1000 + i).collect()
}

fn vcol(column_id: u32, dim: u16, data: Vec<Option<Vec<f32>>>) -> Column {
    Column {
        column_id,
        is_system: false,
        values: ColumnValues::Vector { dim, data },
    }
}

#[test]
fn roundtrip_f32_vectors() {
    let tmp = TempPath::new("vec");
    let n = 4u64;
    let meta = sample_meta(n);
    let data = vec![
        Some(vec![1.0, 2.0, 3.0, 4.0]),
        Some(vec![5.0, 6.0, 7.0, 8.0]),
        Some(vec![-1.0, 0.0, 1.0, 2.0]),
        Some(vec![0.5, 0.25, 0.125, 0.0]),
    ];
    let columns = vec![vcol(1, 4, data.clone())];

    write_file(&tmp.0, &meta, &columns, &ids(n)).expect("write");
    let file = read_file(&tmp.0).expect("read");

    assert_eq!(file.columns, columns);
    assert_eq!(file.columns[0].values.vector_dim(), Some(4));
}

#[test]
fn roundtrip_vectors_with_nulls() {
    // Models pending embeddings: some rows have no vector yet.
    let tmp = TempPath::new("vecnull");
    let n = 3u64;
    let meta = sample_meta(n);
    let data = vec![Some(vec![1.0, 1.0]), None, Some(vec![3.0, 4.0])];
    let columns = vec![vcol(2, 2, data.clone())];

    write_file(&tmp.0, &meta, &columns, &ids(n)).expect("write");
    let file = read_file(&tmp.0).expect("read");
    assert_eq!(file.columns, columns);
}

#[test]
fn centroid_zone_map() {
    let tmp = TempPath::new("centroid");
    let n = 4u64;
    let meta = sample_meta(n);
    // Four corners of a square centered at (1,1).
    let data = vec![
        Some(vec![0.0, 0.0]),
        Some(vec![2.0, 0.0]),
        Some(vec![0.0, 2.0]),
        Some(vec![2.0, 2.0]),
    ];
    let columns = vec![vcol(7, 2, data)];

    write_file(&tmp.0, &meta, &columns, &ids(n)).expect("write");
    let file = read_file(&tmp.0).expect("read");

    let z = file.footer.zone_maps.iter().find(|z| z.column_id == 7).unwrap();
    assert_eq!(z.centroid, vec![1.0, 1.0]);
    // distance from (1,1) to any corner = sqrt(2)
    assert!((z.max_radius - 2.0_f32.sqrt()).abs() < 1e-6, "radius {}", z.max_radius);
    assert!(z.min.is_empty() && z.max.is_empty());
}

#[test]
fn multipage_vectors_have_per_page_centroids() {
    let tmp = TempPath::new("vecmulti");
    let n = 6u64;
    let meta = sample_meta(n);
    let data: Vec<Option<Vec<f32>>> = (0..6).map(|i| Some(vec![i as f32, 0.0])).collect();
    let columns = vec![vcol(3, 2, data.clone())];

    write_file_with(&tmp.0, &meta, &columns, &ids(n), &[], &WriteOptions { rows_per_page: 2 })
        .expect("write");
    let file = read_file(&tmp.0).expect("read");

    assert_eq!(file.columns[0].values, ColumnValues::Vector { dim: 2, data });
    let zms: Vec<_> = file.footer.zone_maps.iter().filter(|z| z.column_id == 3).collect();
    assert_eq!(zms.len(), 3); // ceil(6/2)
    // page 0 holds (0,0),(1,0) → centroid (0.5, 0)
    assert_eq!(zms[0].centroid, vec![0.5, 0.0]);
}

#[test]
fn empty_vector_column_preserves_dim() {
    let tmp = TempPath::new("vecempty");
    let meta = sample_meta(0);
    let columns = vec![vcol(1, 8, vec![])];

    write_file(&tmp.0, &meta, &columns, &[]).expect("write");
    let file = read_file(&tmp.0).expect("read");
    assert_eq!(file.columns[0].values.vector_dim(), Some(8));
}

#[test]
fn rejects_wrong_dimension() {
    let tmp = TempPath::new("vecbaddim");
    let n = 2u64;
    let meta = sample_meta(n);
    // Second row has length 3, but dim is 4.
    let data = vec![Some(vec![1.0, 2.0, 3.0, 4.0]), Some(vec![1.0, 2.0, 3.0])];
    let columns = vec![vcol(1, 4, data)];

    let err = write_file(&tmp.0, &meta, &columns, &ids(n)).unwrap_err();
    assert!(matches!(err, FormatError::Decode(_)), "{err:?}");
}
