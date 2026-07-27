//! §9 L0-C ledgered nondeterminism + `matches_format`: the values are generated once live, recorded
//! as INJECT entries, and replayed verbatim — so a turn using `uuid()`/`now()`/a validator is still
//! bit-identical on replay (§12.3, G2).

use aelio_kernel::{compile, replay, EffectClass, Instance, Registry, TurnOutcome};
use aelio_sol::SolValue;

#[test]
fn nondet_sources_replay_bit_identical() {
    // Let binds now()/uuid()/random() into the bag, then a Read call echoes them through.
    let program = compile(
        r#"{"nid":"root","op":"Let",
             "bindings":[
               {"key":"t","value":{"fn":"now","args":[]}},
               {"key":"id","value":{"fn":"uuid","args":[]}},
               {"key":"r","value":{"fn":"random","args":[]}}
             ],
             "body":{"nid":"echo","op":"Call","id":"io.echo@1","args":{"id":{"pull":"id"}},"into":"out"}}"#,
    )
    .unwrap();
    let mut reg = Registry::default();
    reg.register("io.echo@1", EffectClass::Read, |args| Ok(args.clone()));
    let mut inst = Instance::new(program.clone(), &mut reg);

    let (live_hash, ledger) = match inst.start(SolValue::map::<_, &str>([])).unwrap() {
        TurnOutcome::Completed { bag_hash, .. } => (bag_hash, inst.ledger().clone()),
        _ => panic!("should complete"),
    };
    // The nondet values landed in the ledger as INJECT entries.
    let nondet_entries = ledger.entries().iter().filter(|e| e.kind == "nondet_value").count();
    assert_eq!(nondet_entries, 3, "now + uuid + random each ledgered once");

    // Replay reproduces the exact same bag despite the nondeterminism (values injected from ledger).
    let replay_hash = replay(&program, &ledger, SolValue::map::<_, &str>([])).unwrap();
    assert_eq!(live_hash, replay_hash, "nondet turn is bit-identical on replay (§12.3)");
}

#[test]
fn matches_format_dispatches_to_a_registered_validator_and_replays() {
    let program = compile(
        r#"{"nid":"root","op":"Let",
             "bindings":[{"key":"ok","value":{"fn":"matches_format","args":[{"pull":"phone"},{"lit":"e164"}]}}],
             "body":{"nid":"mark","op":"Call","id":"io.mark@1","args":{"ok":{"pull":"ok"}},"into":"out"}}"#,
    )
    .unwrap();
    let mut reg = Registry::default();
    // The e164 validator: value must be a string starting with '+'.
    reg.register("e164", EffectClass::Pure, |args| {
        let v = args.as_map().and_then(|m| m.get("value"));
        let ok = matches!(v, Some(SolValue::Str(s)) if s.starts_with('+'));
        Ok(SolValue::Bool(ok))
    });
    reg.register("io.mark@1", EffectClass::Read, |args| Ok(args.clone()));
    let mut inst = Instance::new(program.clone(), &mut reg);

    let initial = SolValue::map([("phone", SolValue::str("+15551234567"))]);
    let (live_hash, ledger) = match inst.start(initial.clone()).unwrap() {
        TurnOutcome::Completed { bag, bag_hash } => {
            let ok = bag.as_map().unwrap().get("out").and_then(|o| o.as_map()).and_then(|m| m.get("ok"));
            assert_eq!(ok, Some(&SolValue::Bool(true)), "validator passed for +E.164");
            (bag_hash, inst.ledger().clone())
        }
        _ => panic!("should complete"),
    };
    assert!(ledger.entries().iter().any(|e| e.kind == "validate_result"));
    // Replay reproduces the validator result from the ledger (no registry needed in replay).
    let replay_hash = replay(&program, &ledger, initial).unwrap();
    assert_eq!(live_hash, replay_hash, "matches_format is bit-identical on replay");
}
