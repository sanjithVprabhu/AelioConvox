//! Database-level mutations: delete, update, and compaction — across the memtable/segment
//! boundary, with crash-safe reopen, exercised through the public `Database` facade.

use ll_query::{
    ColumnKind, Database, HybridQuery, PredOp, TransactionMutation, TransactionMutationResult,
    TransactionPrecondition, Value,
};

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ll_mut_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn schema() -> Vec<(&'static str, ColumnKind)> {
    vec![
        ("embedding", ColumnKind::Vector(2)),
        ("body", ColumnKind::Text),
        ("links", ColumnKind::Edge),
        ("score", ColumnKind::I64),
    ]
}

fn row(emb: [f32; 2], body: &str, links: Vec<u64>, score: i64) -> Vec<(&'static str, Value)> {
    vec![
        ("embedding", Value::Vector(emb.to_vec())),
        ("body", Value::Utf8(body.to_string())),
        ("links", Value::Edges(links)),
        ("score", Value::I64(score)),
    ]
}

fn ids(res: &[(u64, f32)]) -> Vec<u64> {
    res.iter().map(|(id, _)| *id).collect()
}

#[test]
fn delete_removes_a_memtable_row_from_results() {
    let dir = tmpdir("del_mem");
    let mut db = Database::create(&dir).unwrap();
    db.create_table("docs", &schema()).unwrap();
    let a = db.insert("docs", &row([0.0, 0.0], "alpha", vec![], 1)).unwrap();
    let b = db.insert("docs", &row([10.0, 0.0], "beta", vec![], 2)).unwrap();

    assert!(db.delete("docs", a).unwrap());
    let res = db.query("docs", &HybridQuery::new(5).vector("embedding", vec![0.0, 0.0])).unwrap();
    assert_eq!(ids(&res), vec![b], "deleted memtable row must not appear: {res:?}");
}

#[test]
fn transaction_commits_multiple_rows_together_and_recovers() {
    let dir = tmpdir("batch");
    let (first, second) = {
        let mut db = Database::create(&dir).unwrap();
        db.create_table("docs", &schema()).unwrap();
        let result = db
            .transact(vec![
                TransactionMutation::Insert {
                    table: "docs".into(),
                    values: row([0.0, 0.0], "alpha", vec![], 1)
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v))
                        .collect(),
                },
                TransactionMutation::Insert {
                    table: "docs".into(),
                    values: row([10.0, 0.0], "beta", vec![], 2)
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v))
                        .collect(),
                },
            ])
            .unwrap();
        assert_eq!(result.results.len(), 2);
        let first = match result.results[0] {
            TransactionMutationResult::Inserted { row_id } => row_id,
            _ => panic!("expected inserted row"),
        };
        let second = match result.results[1] {
            TransactionMutationResult::Inserted { row_id } => row_id,
            _ => panic!("expected inserted row"),
        };
        (first, second)
    };
    let db = Database::open(&dir).unwrap();
    let results = db.query("docs", &HybridQuery::new(2).vector("embedding", vec![0.0, 0.0])).unwrap();
    let got = ids(&results);
    assert!(got.contains(&first) && got.contains(&second), "batch did not recover: {got:?}");
}

#[test]
fn conditional_transaction_provides_atomic_idempotency_and_revision_cas() {
    let dir = tmpdir("conditional");
    let mut db = Database::create(&dir).unwrap();
    db.create_table("docs", &schema()).unwrap();
    let values = row([0.0, 0.0], "event-1", vec![], 1)
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect::<Vec<_>>();

    let first = db
        .transact_conditional(
            vec![TransactionPrecondition::Absent {
                table: "docs".into(),
                equals: vec![("body".into(), Value::Utf8("event-1".into()))],
            }],
            vec![TransactionMutation::Insert { table: "docs".into(), values }],
        )
        .unwrap();
    assert!(first.applied);
    let id = match first.transaction.unwrap().results[0] {
        TransactionMutationResult::Inserted { row_id } => row_id,
        _ => panic!("expected inserted row"),
    };

    let duplicate = db
        .transact_conditional(
            vec![TransactionPrecondition::Absent {
                table: "docs".into(),
                equals: vec![("body".into(), Value::Utf8("event-1".into()))],
            }],
            vec![TransactionMutation::Insert {
                table: "docs".into(),
                values: row([1.0, 1.0], "event-1", vec![], 1)
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
            }],
        )
        .unwrap();
    assert!(!duplicate.applied, "duplicate claim must append no rows");

    let updated = db
        .transact_conditional(
            vec![TransactionPrecondition::RowMatches {
                table: "docs".into(),
                row_id: id,
                equals: vec![("score".into(), Value::I64(1))],
            }],
            vec![TransactionMutation::Update {
                table: "docs".into(),
                row_id: id,
                values: vec![("score".into(), Value::I64(2))],
            }],
        )
        .unwrap();
    assert!(updated.applied);

    let stale = db
        .transact_conditional(
            vec![TransactionPrecondition::RowMatches {
                table: "docs".into(),
                row_id: id,
                equals: vec![("score".into(), Value::I64(1))],
            }],
            vec![TransactionMutation::Delete { table: "docs".into(), row_id: id }],
        )
        .unwrap();
    assert!(!stale.applied, "a stale revision must not delete the row");
}

#[test]
fn additive_columns_allow_existing_rows_and_persist_across_reopen() {
    let dir = tmpdir("add_columns");
    let row_id = {
        let mut db = Database::create(&dir).unwrap();
        db.create_table("events", &[("key", ColumnKind::Utf8)]).unwrap();
        let row_id = db.insert("events", &[("key", Value::Utf8("e1".into()))]).unwrap();
        db.add_columns("events", &[("attempts", ColumnKind::I64)]).unwrap();
        db.update("events", row_id, &[("attempts", Value::I64(1))]).unwrap();
        row_id
    };
    let db = Database::open(&dir).unwrap();
    assert_eq!(db.columns("events").unwrap().len(), 2);
    let row = db.get_row_values("events", row_id).unwrap().unwrap();
    assert!(row.iter().any(|(name, value)| name == "attempts" && value == &Value::I64(1)));
}

#[test]
fn delete_removes_a_flushed_row_from_results() {
    let dir = tmpdir("del_seg");
    let mut db = Database::create(&dir).unwrap();
    db.create_table("docs", &schema()).unwrap();
    let a = db.insert("docs", &row([0.0, 0.0], "alpha", vec![], 1)).unwrap();
    let b = db.insert("docs", &row([10.0, 0.0], "beta", vec![], 2)).unwrap();
    db.flush().unwrap(); // a, b now live only in the segment

    // Delete the segment-resident row; the tombstone goes through the memtable.
    assert!(db.delete("docs", a).unwrap());
    let res = db.query("docs", &HybridQuery::new(5).vector("embedding", vec![0.0, 0.0])).unwrap();
    assert_eq!(ids(&res), vec![b], "deleted segment row must not resurface: {res:?}");
}

#[test]
fn delete_is_a_noop_for_unknown_or_already_deleted_rows() {
    let dir = tmpdir("del_noop");
    let mut db = Database::create(&dir).unwrap();
    db.create_table("docs", &schema()).unwrap();
    let a = db.insert("docs", &row([0.0, 0.0], "alpha", vec![], 1)).unwrap();

    assert!(!db.delete("docs", 99999).unwrap(), "deleting an unknown row should return false");
    assert!(db.delete("docs", a).unwrap());
    assert!(!db.delete("docs", a).unwrap(), "second delete of the same row should return false");
}

#[test]
fn update_changes_values_and_supersedes_the_segment_copy() {
    let dir = tmpdir("upd_seg");
    let mut db = Database::create(&dir).unwrap();
    db.create_table("docs", &schema()).unwrap();
    let a = db.insert("docs", &row([0.0, 0.0], "alpha", vec![], 1)).unwrap();
    let b = db.insert("docs", &row([10.0, 0.0], "beta", vec![], 2)).unwrap();
    db.flush().unwrap();

    // Move `a` far away and bump its score; only `embedding` and `score` change — `body`
    // and `links` must be preserved from the segment copy.
    assert!(db
        .update("docs", a, &[("embedding", Value::Vector(vec![100.0, 0.0])), ("score", Value::I64(50))])
        .unwrap());

    // Nearest to (0,0) is now `b` (at 10,0); the stale (0,0) copy of `a` must not win.
    let near = db.query("docs", &HybridQuery::new(1).vector("embedding", vec![0.0, 0.0])).unwrap();
    assert_eq!(near.first().map(|(id, _)| *id), Some(b), "stale segment vector won: {near:?}");

    // The preserved `body` text is still searchable, and the new score filter matches.
    let q = HybridQuery::new(5)
        .text("body", "alpha")
        .filter("score", PredOp::Ge, Value::I64(50));
    let res = db.query("docs", &q).unwrap();
    assert!(ids(&res).contains(&a), "updated row lost its preserved text/score: {res:?}");
}

#[test]
fn update_returns_false_for_a_deleted_row() {
    let dir = tmpdir("upd_del");
    let mut db = Database::create(&dir).unwrap();
    db.create_table("docs", &schema()).unwrap();
    let a = db.insert("docs", &row([0.0, 0.0], "alpha", vec![], 1)).unwrap();
    assert!(db.delete("docs", a).unwrap());
    assert!(!db.update("docs", a, &[("score", Value::I64(9))]).unwrap());
}

#[test]
fn compaction_drops_tombstones_and_preserves_live_rows() {
    let dir = tmpdir("compact");
    let mut db = Database::create(&dir).unwrap();
    db.create_table("docs", &schema()).unwrap();
    let a = db.insert("docs", &row([0.0, 0.0], "alpha diffusion", vec![], 1)).unwrap();
    let b = db.insert("docs", &row([10.0, 0.0], "beta", vec![a], 2)).unwrap();
    let c = db.insert("docs", &row([0.5, 0.5], "gamma diffusion", vec![], 3)).unwrap();
    db.flush().unwrap();
    db.delete("docs", a).unwrap(); // tombstone in the memtable
    db.flush().unwrap(); // tombstone persisted to a second segment

    db.compact().unwrap();

    // a is gone; b and c survive with their data intact.
    let all = db.query("docs", &HybridQuery::new(10).vector("embedding", vec![0.0, 0.0])).unwrap();
    let got = ids(&all);
    assert!(!got.contains(&a), "compaction kept a deleted row: {got:?}");
    assert!(got.contains(&b) && got.contains(&c), "compaction dropped a live row: {got:?}");

    // Text index rebuilt: 'diffusion' still matches c (a was deleted).
    let txt = db.query("docs", &HybridQuery::new(10).text("body", "diffusion")).unwrap();
    assert!(ids(&txt).contains(&c) && !ids(&txt).contains(&a), "text index wrong after compaction: {txt:?}");
}

#[test]
fn compaction_survives_reopen() {
    let dir = tmpdir("compact_reopen");
    {
        let mut db = Database::create(&dir).unwrap();
        db.create_table("docs", &schema()).unwrap();
        let a = db.insert("docs", &row([0.0, 0.0], "alpha", vec![], 1)).unwrap();
        let _b = db.insert("docs", &row([10.0, 0.0], "beta", vec![], 2)).unwrap();
        db.flush().unwrap();
        db.delete("docs", a).unwrap();
        db.compact().unwrap();
    }
    // Reopen from disk: the compacted segment + truncated WAL must reconstruct the same state.
    let db = Database::open(&dir).unwrap();
    let res = db.query("docs", &HybridQuery::new(10).vector("embedding", vec![0.0, 0.0])).unwrap();
    assert_eq!(res.len(), 1, "reopened compacted db has wrong row count: {res:?}");
}

#[test]
fn new_writes_after_compaction_still_supersede() {
    let dir = tmpdir("compact_then_write");
    let mut db = Database::create(&dir).unwrap();
    db.create_table("docs", &schema()).unwrap();
    let a = db.insert("docs", &row([0.0, 0.0], "alpha", vec![], 1)).unwrap();
    let b = db.insert("docs", &row([10.0, 0.0], "beta", vec![], 2)).unwrap();
    db.flush().unwrap();
    db.compact().unwrap(); // both rows now stamped at LSN 1 in the compacted segment

    // A fresh update (at a WAL LSN well above 1) must still supersede the compacted copy:
    // move `a` from (0,0) out to (100,0). The query returns RRF scores, so we assert on the
    // resulting *ranking* — nearest to (0,0) must now be `b` (at 10,0), not the stale `a`.
    assert!(db.update("docs", a, &[("embedding", Value::Vector(vec![100.0, 0.0]))]).unwrap());
    let near = db.query("docs", &HybridQuery::new(1).vector("embedding", vec![0.0, 0.0])).unwrap();
    assert_eq!(
        near.first().map(|(id, _)| *id),
        Some(b),
        "post-compaction update didn't supersede the LSN-1 segment copy: {near:?}"
    );
}
