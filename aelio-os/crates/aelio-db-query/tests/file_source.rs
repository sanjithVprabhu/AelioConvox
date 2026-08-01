//! Persisted `.vss` rows are queryable through every modality via `FileSource`, with MVCC.
//! This is the symmetric partner to the memtable `Source`: read-your-writes across *all*
//! data (recent + persisted), through one interface.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use aelio_db_engine::{Engine, Row, Value};
use aelio_db_format::read_file;
use aelio_db_query::{FileSource, Source};

const VEC: u32 = 1;
const BODY: u32 = 2;
const FOLLOWS: u32 = 3;
const SCORE: u32 = 4;

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct Tmp(PathBuf);
impl Tmp {
    fn new(ext: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        Tmp(std::env::temp_dir().join(format!("aelio_fs_{}_{n}.{ext}", std::process::id())))
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn row(vec: [f32; 2], body: &str, follows: Vec<u64>, score: i64) -> Row {
    let mut r = Row::new();
    r.insert(VEC, Value::Vector(vec.to_vec()));
    r.insert(BODY, Value::Utf8(body.into()));
    r.insert(FOLLOWS, Value::Edges(follows));
    r.insert(SCORE, Value::I64(score));
    r
}

#[test]
fn query_a_flushed_file_across_all_modalities() {
    let wal = Tmp::new("log");
    let vss = Tmp::new("vss");

    let (l1, l4);
    {
        let mut e = Engine::create(&wal.0, 1).unwrap();
        l1 = e
            .insert(1, row([0.0, 0.0], "alpha quick", vec![2, 3], 100))
            .unwrap();
        e.insert(2, row([10.0, 0.0], "quick quick beta", vec![3], 200))
            .unwrap();
        e.insert(3, row([5.0, 0.0], "gamma", vec![], 300)).unwrap();
        l4 = e
            .insert(4, row([3.0, 0.0], "quick delta", vec![1], 400))
            .unwrap();
        assert_eq!(e.flush_to(&vss.0).unwrap(), 4);
    }
    let _ = l1;

    let file = read_file(&vss.0).unwrap();
    let src = FileSource::new(std::sync::Arc::new(file));
    let snap = u64::MAX - 1; // latest snapshot (u64::MAX is the reserved xmax = ∞ sentinel)

    // Vector: nearest to (4.5,0) is row 3 (5,0) then row 4 (3,0).
    let v = src.vector_search(VEC, &[4.5, 0.0], 2, 128, snap);
    assert_eq!(v.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![3, 4]);

    // Text: "quick" appears in 1,2,4; row 2 twice → ranks first; row 3 absent.
    let t = src.text_search(BODY, "quick", 10, snap);
    let tids: Vec<u64> = t.iter().map(|(id, _)| *id).collect();
    assert_eq!(tids[0], 2);
    assert!(tids.contains(&1) && tids.contains(&4) && !tids.contains(&3));

    // Graph: row 1 follows {2,3}; who follows 3 → {1,2}.
    assert_eq!(src.out_neighbors(FOLLOWS, 1, snap), vec![2, 3]);
    let mut into3 = src.in_neighbors(FOLLOWS, 3, snap);
    into3.sort();
    assert_eq!(into3, vec![1, 2]);

    // Scalar: row 2's score.
    assert_eq!(src.scalar(2, SCORE, snap), Some(Value::I64(200)));

    // MVCC: at a snapshot just before row 4's write, row 4 is invisible.
    let before4 = l4 - 1;
    assert_eq!(src.scalar(4, SCORE, before4), None);
    assert_eq!(src.scalar(1, SCORE, before4), Some(Value::I64(100)));
    // ...and row 4 drops out of vector results at that snapshot (nearest becomes row 3 only among visible).
    let v_old = src.vector_search(VEC, &[3.0, 0.0], 4, 128, before4);
    assert!(
        !v_old.iter().any(|(id, _)| *id == 4),
        "row 4 must be invisible before its write"
    );
}
