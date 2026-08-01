//! The cost model is wired into execution: a vector query with a *selective* filter
//! automatically pre-filters (exact recall) instead of post-filtering (which truncates to
//! the global top-ef and misses the selective matches).

use aelio_db_engine::{Memtable, Op, Row, Value};
use aelio_db_query::{execute, explain_plan, PredOp, Query, Source};

const VEC: u32 = 1;
const CAT: u32 = 2; // 1 = rare, 0 = common

fn build() -> Memtable {
    let mut m = Memtable::new();
    for i in 0..200u64 {
        let mut r = Row::new();
        r.insert(VEC, Value::Vector(vec![i as f32, 0.0]));
        r.insert(CAT, Value::I64(if i % 40 == 0 { 1 } else { 0 }));
        m.apply(i, Op::Put(r), i + 1);
    }
    m
}

#[test]
fn selective_filter_triggers_prefilter_and_exact_recall() {
    let m = build();
    let sources: &[&dyn Source] = &[&m];
    let snap = m.snapshot_lsn();

    // Vector near x=0, restricted to the rare category (5 of 200 rows: x = 0,40,80,120,160).
    // True nearest-3 rare to x=0 are rows 0, 40, 80.
    let q = Query::new(3)
        .with_vector(VEC, vec![0.0, 0.0])
        .filter(CAT, PredOp::Eq, Value::I64(1));

    let ids: Vec<u64> = execute(sources, &q, snap)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        ids,
        vec![0, 40, 80],
        "pre-filter must return the exact nearest matches"
    );

    // The cost model chose PreFilter (selectivity = 5/200 = 0.025).
    let plan = explain_plan(sources, &q, snap);
    assert!(plan.contains("PreFilter"), "plan: {plan}");

    // Sanity: a naive post-filter (ANN top-12 then filter) would have returned only row 0
    // (the others aren't in the global nearest-12), i.e. recall 1/3 — the collapse we avoid.
}

#[test]
fn nonselective_filter_uses_postfilter() {
    let m = build();
    let sources: &[&dyn Source] = &[&m];
    let snap = m.snapshot_lsn();

    // The "common" category is 195/200 rows → high selectivity → post-filter is chosen,
    // and is correct here (the nearest common rows are 1,2,3).
    let q = Query::new(3)
        .with_vector(VEC, vec![0.0, 0.0])
        .filter(CAT, PredOp::Eq, Value::I64(0));

    let ids: Vec<u64> = execute(sources, &q, snap)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, vec![1, 2, 3]);

    let plan = explain_plan(sources, &q, snap);
    assert!(plan.contains("PostFilter"), "plan: {plan}");
}
