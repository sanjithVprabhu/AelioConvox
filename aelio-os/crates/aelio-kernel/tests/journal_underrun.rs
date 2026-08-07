//! Q22 / A7 — replay past the end of the journal must be typed `Journal.Underrun`, never dispatch.

use aelio_kernel::{
    compile, replay, EffectClass, Instance, Ledger, ReasonCode, Registry, TurnOutcome,
};
use aelio_sol::SolValue;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn empty_journal_replay_returns_journal_underrun_without_dispatch() {
    static DISPATCHES: AtomicUsize = AtomicUsize::new(0);

    let program = compile(
        r#"{
          "nid": "root",
          "op": "Call",
          "id": "io.echo@1",
          "args": { "x": { "lit": 1 } },
          "into": "out"
        }"#,
    )
    .unwrap();

    let mut reg = Registry::default();
    reg.register("io.echo@1", EffectClass::Read, |args| {
        DISPATCHES.fetch_add(1, Ordering::SeqCst);
        Ok(args.clone())
    });

    let mut inst = Instance::new(program.clone(), &mut reg);
    match inst.start(SolValue::Null).unwrap() {
        TurnOutcome::Completed { .. } => {}
        _ => panic!("live run should complete"),
    };
    assert_eq!(DISPATCHES.load(Ordering::SeqCst), 1, "live run should dispatch once");
    DISPATCHES.store(0, Ordering::SeqCst);

    let err = replay(&program, &Ledger::default(), SolValue::Null).unwrap_err();
    assert_eq!(err.code, ReasonCode::JournalUnderrun);
    assert!(
        err.detail.contains("replay ran past the ledger"),
        "detail={}",
        err.detail
    );
    assert_eq!(
        DISPATCHES.load(Ordering::SeqCst),
        0,
        "replay must not dispatch tools when the journal underruns"
    );
}

#[test]
fn truncated_effect_journal_replays_until_first_missing_read_result() {
    static DISPATCHES: AtomicUsize = AtomicUsize::new(0);

    let program = compile(
        r#"{
          "nid": "root",
          "op": "Seq",
          "steps": [
            {
              "nid": "c1",
              "op": "Call",
              "id": "io.echo@1",
              "args": { "x": { "lit": 1 } },
              "into": "a"
            },
            {
              "nid": "c2",
              "op": "Call",
              "id": "io.echo@1",
              "args": { "x": { "lit": 2 } },
              "into": "b"
            }
          ]
        }"#,
    )
    .unwrap();

    let mut reg = Registry::default();
    reg.register("io.echo@1", EffectClass::Read, |args| {
        DISPATCHES.fetch_add(1, Ordering::SeqCst);
        Ok(args.clone())
    });

    let mut inst = Instance::new(program.clone(), &mut reg);
    let ledger = match inst.start(SolValue::Null).unwrap() {
        TurnOutcome::Completed { .. } => inst.ledger().clone(),
        _ => panic!("live run should complete"),
    };
    assert_eq!(DISPATCHES.load(Ordering::SeqCst), 2);
    DISPATCHES.store(0, Ordering::SeqCst);

    let cutoff = ledger
        .entries()
        .iter()
        .position(|entry| entry.nid.as_deref() == Some("c2"))
        .expect("second call should be ledgered");
    let truncated = Ledger::from_entries(ledger.entries()[..cutoff].to_vec()).unwrap();

    let err = replay(&program, &truncated, SolValue::Null).unwrap_err();
    assert_eq!(err.code, ReasonCode::JournalUnderrun);
    assert_eq!(DISPATCHES.load(Ordering::SeqCst), 0);
}
