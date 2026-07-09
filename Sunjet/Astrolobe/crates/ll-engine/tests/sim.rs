//! Deterministic model-based simulation: drive the engine through random sequences of
//! insert / update / delete / crash-recover / flush, and verify it always matches a
//! reference oracle. Many seeds → broad, reproducible coverage of the write+recovery loop.
//! This is the core correctness discipline for a database.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ll_engine::{Engine, Row, Value, SYS_XMAX_COL};
use ll_format::{read_file, ColumnValues};

/// Deterministic SplitMix64.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed ^ 0xD1B5_4A32_D192_ED03)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn f(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct TmpDir(PathBuf);
impl TmpDir {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("ll_sim_{}_{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn random_row(r: &mut Rng) -> Row {
    let mut row = Row::new();
    row.insert(1, Value::I64(r.below(1000) as i64));
    row.insert(2, Value::Utf8(format!("doc-{}", r.below(50))));
    row.insert(3, Value::F64(r.f()));
    row.insert(4, Value::Vector(vec![r.f() as f32, r.f() as f32]));
    row
}

fn run_seed(seed: u64) {
    let dir = TmpDir::new();
    let wal = dir.0.join("wal.log");
    let mut eng = Engine::create(&wal, 1).unwrap();
    let mut model: BTreeMap<u64, Row> = BTreeMap::new();
    let mut r = Rng::new(seed);
    let mut next_id = 1u64;
    let mut ids: Vec<u64> = Vec::new();

    for _ in 0..400 {
        match r.below(6) {
            0 | 1 => {
                // insert
                let id = next_id;
                next_id += 1;
                let row = random_row(&mut r);
                eng.insert(id, row.clone()).unwrap();
                model.insert(id, row);
                ids.push(id);
            }
            2 => {
                // update an existing id
                if !ids.is_empty() {
                    let id = ids[r.below(ids.len() as u64) as usize];
                    let row = random_row(&mut r);
                    eng.insert(id, row.clone()).unwrap();
                    model.insert(id, row);
                }
            }
            3 => {
                // delete an existing id
                if !ids.is_empty() {
                    let id = ids[r.below(ids.len() as u64) as usize];
                    eng.delete(id).unwrap();
                    model.remove(&id);
                }
            }
            4 => {
                // simulate a crash and recover from the WAL
                drop(eng);
                eng = Engine::recover(&wal).unwrap();
            }
            _ => {
                // spot-check a known id against the model right now
                if !ids.is_empty() {
                    let id = ids[r.below(ids.len() as u64) as usize];
                    assert_eq!(eng.get(id), model.get(&id), "seed {seed} spot id {id}");
                }
            }
        }
    }

    // Final crash + recover, then verify the whole state matches the oracle.
    drop(eng);
    let eng = Engine::recover(&wal).unwrap();
    for (id, row) in &model {
        assert_eq!(eng.get(*id), Some(row), "seed {seed}: row {id} after recovery");
    }
    assert_eq!(eng.memtable().live_count(), model.len(), "seed {seed}: live count");
    assert!(eng.get(next_id + 12345).is_none(), "seed {seed}: phantom row");

    // Flush persists every live row (and may also persist tombstones for deleted ids). The
    // live (non-tombstone) rows in the segment must be exactly the oracle's rows.
    let vss = dir.0.join("final.vss");
    let n = eng.flush_to(&vss).unwrap();
    assert!(n >= model.len(), "seed {seed}: flush count {n} < live {}", model.len());
    let file = read_file(&vss).unwrap();
    let xmax = file.columns.iter().find(|c| c.column_id == SYS_XMAX_COL);
    let mut live_ids: Vec<u64> = match xmax.map(|c| &c.values) {
        Some(ColumnValues::I64(v)) => file
            .translation_table
            .iter()
            .zip(v)
            .filter(|(_, x)| **x == Some(-1)) // xmax == u64::MAX (live)
            .map(|(id, _)| *id)
            .collect(),
        _ => file.translation_table.clone(),
    };
    live_ids.sort();
    let mut keys: Vec<u64> = model.keys().copied().collect();
    keys.sort();
    assert_eq!(live_ids, keys, "seed {seed}: flushed live row ids");
}

#[test]
fn simulation_matches_oracle_across_seeds() {
    for seed in 0..40 {
        run_seed(seed);
    }
}
