//! Phase 2.7 — cancel blocks post-cancel effectful dispatch on the kernel Instance.

use aelio_kernel::{compile, EffectClass, Instance, Registry, TurnOutcome};
use aelio_sol::SolValue;

#[test]
fn cancelled_instance_refuses_external_call_dispatch() {
    let program = compile(
        r#"{
          "nid": "root",
          "op": "Call",
          "id": "tool.act_stub@1",
          "args": {},
          "into": "sent"
        }"#,
    )
    .expect("compile");
    let mut registry = Registry::default();
    registry.register("tool.act_stub@1", EffectClass::External, |_| {
        Ok(SolValue::map([("ok", SolValue::Bool(true))]))
    });
    let mut inst = Instance::new(program, &mut registry);
    inst.mark_cancelled();
    match inst.start(SolValue::Null) {
        Ok(_) => panic!("must refuse effectful dispatch after cancel"),
        Err(err) => assert!(
            err.detail.contains("post-cancel") || err.detail.contains("cancelled"),
            "unexpected error: {}",
            err.detail
        ),
    }
}

#[test]
fn non_cancelled_external_call_still_runs() {
    let program = compile(
        r#"{
          "nid": "root",
          "op": "Call",
          "id": "tool.act_stub@1",
          "args": {},
          "into": "sent"
        }"#,
    )
    .expect("compile");
    let mut registry = Registry::default();
    registry.register("tool.act_stub@1", EffectClass::External, |_| {
        Ok(SolValue::map([("ok", SolValue::Bool(true))]))
    });
    let mut inst = Instance::new(program, &mut registry);
    match inst.start(SolValue::Null).expect("start") {
        TurnOutcome::Completed { bag, .. } => {
            let ok = bag
                .as_map()
                .and_then(|m| m.get("sent"))
                .and_then(|v| v.as_map())
                .and_then(|m| m.get("ok"))
                .cloned();
            assert_eq!(ok, Some(SolValue::Bool(true)));
        }
        TurnOutcome::Parked(_) => panic!("unexpected park"),
    }
}

#[test]
fn pure_call_still_allowed_when_cancelled() {
    // Cancel-safe policy targets *effects* (write/external), not pure compute.
    let program = compile(
        r#"{
          "nid": "root",
          "op": "Call",
          "id": "compute.hold@1",
          "args": { "v": { "lit": 7 } },
          "into": "x"
        }"#,
    )
    .expect("compile");
    let mut registry = Registry::default();
    registry.register("compute.hold@1", EffectClass::Pure, |args| {
        Ok(args
            .as_map()
            .and_then(|m| m.get("v"))
            .cloned()
            .unwrap_or(SolValue::Null))
    });
    let mut inst = Instance::new(program, &mut registry);
    inst.mark_cancelled();
    match inst.start(SolValue::Null).expect("pure still ok") {
        TurnOutcome::Completed { bag, .. } => {
            assert_eq!(
                bag.as_map().and_then(|m| m.get("x")).cloned(),
                Some(SolValue::Int(7))
            );
        }
        TurnOutcome::Parked(_) => panic!("unexpected park"),
    }
}
