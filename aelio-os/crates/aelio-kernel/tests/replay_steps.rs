//! Q21 / A1 — per-step replay divergence reports the first diverging seq.

use harness_core::exec::{hash_step_payload, verify_replay, Step};

#[test]
fn verify_replay_reports_first_diverging_seq() {
    let live = vec![
        Step {
            seq: 1,
            inputs_hash: hash_step_payload("in", b"a"),
            output_hash: hash_step_payload("out", b"1"),
        },
        Step {
            seq: 2,
            inputs_hash: hash_step_payload("in", b"b"),
            output_hash: hash_step_payload("out", b"2"),
        },
    ];
    let replay = vec![
        Step {
            seq: 1,
            inputs_hash: hash_step_payload("in", b"a"),
            output_hash: hash_step_payload("out", b"1"),
        },
        Step {
            seq: 2,
            inputs_hash: hash_step_payload("in", b"b"),
            output_hash: hash_step_payload("out", b"wrong"),
        },
    ];
    let report = verify_replay(&live, &replay);
    assert!(!report.matched);
    assert_eq!(report.diverged_at, Some(2));
}
