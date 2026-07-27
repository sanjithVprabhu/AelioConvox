//! §6.2 wave-legality conformance. R/W sets are recorded now; parallelism later is an executor
//! change, never a spec change.

use aelio_kernel::compile;
use aelio_kernel::waves::{can_share_wave, rw_set};

/// Compile a single Call node (bypassing the login registry — the Planner only needs schema/paths).
fn call(nid: &str, into: &str, args_reads: &[&str]) -> aelio_kernel::Node {
    let args: String = args_reads
        .iter()
        .enumerate()
        .map(|(i, p)| format!(r#""a{i}":{{"pull":"{p}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let json =
        format!(r#"{{"nid":"{nid}","op":"Call","id":"t@1","args":{{{args}}},"into":"{into}"}}"#);
    compile(&json).expect("compiles")
}

#[test]
fn rw_sets_are_derived_from_literal_paths() {
    let n = call("c", "out.result", &["in.phone", "in.lang"]);
    let s = rw_set(&n);
    assert_eq!(s.writes.len(), 1, "one write: out.result");
    assert_eq!(s.reads.len(), 2, "two reads: in.phone, in.lang");
}

#[test]
fn disjoint_ops_can_share_a_wave() {
    // a writes out.a reading in.x; b writes out.b reading in.y — fully disjoint.
    let a = call("a", "out.a", &["in.x"]);
    let b = call("b", "out.b", &["in.y"]);
    assert!(can_share_wave(&a, &b), "disjoint R/W ⇒ shareable (§6.2)");
}

#[test]
fn write_read_conflict_forbids_sharing() {
    // a writes shared.v; b reads shared.v ⇒ W_a ∩ R_b ≠ ∅.
    let a = call("a", "shared.v", &["in.x"]);
    let b = call("b", "out.b", &["shared.v"]);
    assert!(
        !can_share_wave(&a, &b),
        "write→read dependency ⇒ not shareable"
    );
}

#[test]
fn prefix_aware_conflict_is_detected() {
    // a writes P=shared; b reads P.child=shared.v. A write to P covers all descendants (§6.2).
    let a = call("a", "shared", &["in.x"]);
    let b = call("b", "out.b", &["shared.v"]);
    assert!(
        !can_share_wave(&a, &b),
        "prefix-aware: write to P covers P.* (§6.2)"
    );
}

#[test]
fn write_write_conflict_forbids_sharing() {
    let a = call("a", "out.same", &["in.x"]);
    let b = call("b", "out.same", &["in.y"]);
    assert!(!can_share_wave(&a, &b), "W ∩ W ≠ ∅ ⇒ not shareable");
}
