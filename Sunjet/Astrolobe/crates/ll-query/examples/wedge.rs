//! The wedge: ONE query plan over four modalities + read-your-writes.
//!
//! Run: `cargo run --release -p ll-query --example wedge`
//!
//! Scenario — a paper recommender. Each paper has an embedding (vector), an abstract (text),
//! a citation list (graph edges), and a year (scalar). A researcher submits a brand-new
//! preprint and immediately asks, in a SINGLE query:
//!
//!   "Of the papers my new preprint builds on (its citation closure), which are about
//!    *diffusion* (text), semantically near my topic vector, and published since 2020?"
//!
//! That one [`HybridQuery`] combines vector search + BM25 text + graph reachability + a
//! scalar filter, fuses the rankings with Reciprocal Rank Fusion, and runs across BOTH the
//! flushed segment (the persisted corpus) AND the in-memory memtable (the preprint we just
//! inserted) — so the brand-new, un-flushed rows participate with no reindex step.
//!
//! No single mainstream system answers this in one plan. pgvector has vector + scalar in SQL
//! but no graph traversal and no fused BM25; Qdrant has vector + payload filters but no SQL
//! joins, no BM25 fusion, no graph hops. The real-world alternative is Postgres + Qdrant + a
//! text engine + a graph store joined in application code — with no cross-store consistency
//! and no read-your-writes. LL does it in one operator tree over one substrate.

use std::path::PathBuf;

use ll_query::{ColumnKind, Database, HybridQuery, PredOp, Value};

fn schema() -> Vec<(&'static str, ColumnKind)> {
    vec![
        ("embedding", ColumnKind::Vector(3)),
        ("abstract", ColumnKind::Text),
        ("cites", ColumnKind::Edge),
        ("year", ColumnKind::I64),
    ]
}

fn paper(emb: [f32; 3], abstract_: &str, cites: Vec<u64>, year: i64) -> Vec<(&'static str, Value)> {
    vec![
        ("embedding", Value::Vector(emb.to_vec())),
        ("abstract", Value::Utf8(abstract_.to_string())),
        ("cites", Value::Edges(cites)),
        ("year", Value::I64(year)),
    ]
}

fn main() {
    let dir: PathBuf = std::env::temp_dir().join(format!("ll_wedge_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut db = Database::create(&dir).unwrap();
    db.create_table("papers", &schema()).unwrap();

    // --- The persisted corpus (flushed to a .vss segment) ------------------------------------
    // Vectors: the "diffusion" cluster sits near [1,0,0]; attention/foundational near [0,1,0];
    // off-topic near [0,0,1]. Citations point from a paper to the work it builds on.
    let id1 = db.insert("papers", &paper([0.0, 1.0, 0.0], "attention mechanisms for sequence models", vec![], 2017)).unwrap();
    let _id2 = db.insert("papers", &paper([0.9, 0.1, 0.0], "diffusion models for image synthesis", vec![id1], 2020)).unwrap();
    let _id3 = db.insert("papers", &paper([0.5, 0.5, 0.0], "a survey of transformer architectures", vec![id1], 2021)).unwrap();
    let id4 = db.insert("papers", &paper([1.0, 0.0, 0.0], "denoising diffusion probabilistic methods", vec![_id2], 2022)).unwrap();
    let id5 = db.insert("papers", &paper([0.95, 0.05, 0.0], "latent diffusion for efficient generation", vec![id4], 2023)).unwrap();
    let _id6 = db.insert("papers", &paper([0.0, 0.0, 1.0], "protein folding with graph networks", vec![], 2022)).unwrap();
    let _id7 = db.insert("papers", &paper([0.0, 0.0, 1.0], "heat diffusion in commercial cooking", vec![], 2023)).unwrap();
    db.flush().unwrap(); // ids 1..7 now live only in the persisted segment

    // --- Brand-new, un-flushed rows (live only in the memtable) ------------------------------
    // A companion note and the researcher's preprint, which cites the persisted paper 5 AND
    // the just-added companion 8. Neither is flushed/indexed on disk yet.
    let id8 = db.insert("papers", &paper([1.0, 0.0, 0.0], "practical tricks for diffusion sampling", vec![], 2024)).unwrap();
    let preprint = db.insert("papers", &paper([0.98, 0.0, 0.0], "fast samplers for latent diffusion", vec![id5, id8], 2024)).unwrap();

    println!("corpus: papers 1..7 flushed to a segment; papers {id8} and {preprint} (the preprint) just inserted, un-flushed.\n");

    // --- ONE query, four modalities, across both sources -------------------------------------
    // From the preprint's citation closure (depth 2), find diffusion-topic papers near the
    // query vector, since 2020 — fused by vector + text rank.
    let q = HybridQuery::new(5)
        .vector("embedding", vec![1.0, 0.0, 0.0]) // 1. semantic: the diffusion direction
        .text("abstract", "diffusion sampling")    // 2. lexical: BM25 over abstracts
        .graph("cites", vec![preprint], 2)          // 3. graph: reachable from the new preprint
        .filter("year", PredOp::Ge, Value::I64(2020)); // 4. scalar: recent work only

    println!("EXPLAIN:\n  {}\n", db.explain("papers", &q).unwrap());

    let results = db.query("papers", &q).unwrap();
    println!("results (one fused ranking):");
    for (id, score) in &results {
        let origin = if *id == id8 { "MEMTABLE (read-your-writes)" } else { "segment (persisted)" };
        println!("  paper {id}  rrf_score={score:.4}   [{origin}]");
    }

    println!("\nwhat happened in this single plan:");
    println!("  - graph: traversed the preprint's citations across the memtable->segment boundary");
    println!("    (preprint -> paper {id5} in the segment, and -> paper {id8} still in the memtable)");
    println!("  - vector + text: ranked the reachable papers by topic similarity and 'diffusion sampling'");
    println!("  - scalar: kept only year >= 2020");
    println!("  - read-your-writes: paper {id8} was inserted moments ago, never flushed, yet it is");
    println!("    a first-class participant — no reindex, no separate write path, one consistent plan.");

    let _ = std::fs::remove_dir_all(&dir);
}
