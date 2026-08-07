use harness_core::exec::{
    execute_effect, hash_step_payload, verify_replay, BudgetDimension, BudgetPool, EffectClass,
    EffectJournal, EffectOutcome, ExecutionMode, ReplayReport, Step,
};
use std::sync::Arc;
use std::thread;

fn step(seq: u64, input: &str, output: &str) -> Step {
    Step {
        seq,
        inputs_hash: hash_step_payload("inputs", input.as_bytes()),
        output_hash: hash_step_payload("output", output.as_bytes()),
    }
}

#[test]
fn verify_replay_matches_identical_traces() {
    let live = vec![step(1, "a", "b"), step(2, "c", "d")];
    let replay = live.clone();
    let report = verify_replay(&live, &replay);
    assert_eq!(
        report,
        ReplayReport {
            matched: true,
            diverged_at: None,
            expected_output_hash: Some(live[1].output_hash),
            actual_output_hash: Some(replay[1].output_hash),
        }
    );
}

#[test]
fn verify_replay_reports_first_diverging_seq() {
    let live = vec![step(1, "a", "b"), step(2, "c", "d")];
    let replay = vec![step(1, "a", "b"), step(2, "c", "DIFF")];
    let report = verify_replay(&live, &replay);
    assert!(!report.matched);
    assert_eq!(report.diverged_at, Some(2));
    assert_eq!(report.expected_output_hash, Some(live[1].output_hash));
    assert_eq!(report.actual_output_hash, Some(replay[1].output_hash));
}

#[test]
fn verify_replay_detects_length_mismatch_as_underrun() {
    let live = vec![step(1, "a", "b"), step(2, "c", "d")];
    let replay = vec![step(1, "a", "b")];
    let report = verify_replay(&live, &replay);
    assert!(!report.matched);
    assert_eq!(report.diverged_at, Some(2));
}

#[test]
fn budget_pool_cas_drain_is_exact() {
    let pool = BudgetPool::new(1_000_000);
    let shared = Arc::new(pool);
    let mut handles = Vec::new();
    for _ in 0..8 {
        let pool = Arc::clone(&shared);
        handles.push(thread::spawn(move || {
            for _ in 0..125_000 {
                pool.try_drain_result_rows(1).expect("drain");
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(shared.remaining_result_rows(), 0);
    assert_eq!(shared.consumed_result_rows(1_000_000), 1_000_000);
}

#[test]
fn budget_pool_refuses_overshoot() {
    let pool = BudgetPool::new(10);
    pool.try_drain_result_rows(7).unwrap();
    let err = pool.try_drain_result_rows(5).unwrap_err();
    assert_eq!(err.dimension, BudgetDimension::ResultRows);
    assert_eq!(err.frame, "root");
    assert_eq!(pool.remaining_result_rows(), 3);
}

#[test]
fn nested_frames_share_one_pool() {
    let pool = BudgetPool::new(10);
    let outer = pool.frame("outer");
    let nested = pool.frame("outer.call_harness");

    outer.try_drain_result_rows(6).unwrap();
    let err = nested.try_drain_result_rows(5).unwrap_err();

    assert_eq!(err.dimension, BudgetDimension::ResultRows);
    assert_eq!(err.frame, "outer.call_harness");
    assert_eq!(pool.remaining_result_rows(), 4);
}

#[test]
fn model_calls_default_to_zero() {
    let pool = BudgetPool::new(10);
    let err = pool.frame("model.ask").try_take_model_call().unwrap_err();

    assert_eq!(err.dimension, BudgetDimension::ModelCalls);
    assert_eq!(err.frame, "model.ask");
    assert_eq!(pool.remaining_model_calls(), 0);
}

#[test]
fn replay_underrun_never_dispatches_a_different_path() {
    let journal = EffectJournal::default();
    let mut replay = journal.replay_cursor();
    let mut dispatches = 0;

    let err = execute_effect(
        ExecutionMode::Replay,
        EffectClass::Read,
        "tool.search",
        &mut EffectJournal::default(),
        Some(&mut replay),
        || {
            dispatches += 1;
            "must not run"
        },
    )
    .unwrap_err();

    assert_eq!(err.seq, 0);
    assert_eq!(dispatches, 0);
}

#[test]
fn stubbed_writes_are_marked_in_the_journal() {
    let mut journal = EffectJournal::default();
    let mut dispatches = 0;

    let outcome = execute_effect(
        ExecutionMode::Shadow,
        EffectClass::Write,
        "tool.create_ticket",
        &mut journal,
        None,
        || {
            dispatches += 1;
            "must not run"
        },
    )
    .unwrap();

    assert_eq!(outcome, EffectOutcome::Stubbed);
    assert_eq!(dispatches, 0);
    assert_eq!(journal.entries().len(), 1);
    assert!(journal.entries()[0].stubbed);
    assert_eq!(journal.entries()[0].effect, "tool.create_ticket");
}
