//! P2 sugar (§8/§11.5): Switch/Retry/Pipe compile down to core ops; the executor never sees them.

use aelio_kernel::error::{ErrV1, ReasonCode};
use aelio_kernel::sugar::desugar;
use aelio_kernel::{compile, EffectClass, Instance, Registry, TurnOutcome};
use aelio_sol::SolValue;
use serde_json::json;

fn compile_sugared(j: serde_json::Value) -> aelio_kernel::Node {
    let core = desugar(&j).expect("desugar");
    compile(&serde_json::to_string(&core).unwrap()).expect("compile core")
}

#[test]
fn pipe_becomes_seq() {
    let core = desugar(&json!({
        "nid": "p", "op": "Pipe",
        "steps": [{"nid":"a","op":"Identity"}, {"nid":"b","op":"Identity"}]
    }))
    .unwrap();
    assert_eq!(core["op"], "Seq");
    assert_eq!(core["steps"].as_array().unwrap().len(), 2);
}

#[test]
fn switch_becomes_nested_branch_and_routes_the_matching_case() {
    let program = compile_sugared(json!({
        "nid": "s", "op": "Switch",
        "on": {"pull": "input.kind"},
        "cases": {
            "a": {"nid":"ca","op":"Call","id":"io.mark_a@1","args":{},"into":"result"},
            "b": {"nid":"cb","op":"Call","id":"io.mark_b@1","args":{},"into":"result"}
        },
        "default": {"nid":"cd","op":"Call","id":"io.mark_default@1","args":{},"into":"result"}
    }));

    let mut reg = Registry::default();
    reg.register("io.mark_a@1", EffectClass::Read, |_| {
        Ok(SolValue::map([("v", SolValue::str("a"))]))
    });
    reg.register("io.mark_b@1", EffectClass::Read, |_| {
        Ok(SolValue::map([("v", SolValue::str("b"))]))
    });
    reg.register("io.mark_default@1", EffectClass::Read, |_| {
        Ok(SolValue::map([("v", SolValue::str("default"))]))
    });
    let mut inst = Instance::new(program, &mut reg);

    let initial = SolValue::map([("input", SolValue::map([("kind", SolValue::str("b"))]))]);
    match inst.start(initial).unwrap() {
        TurnOutcome::Completed { bag, .. } => {
            let picked = bag
                .as_map()
                .unwrap()
                .get("result")
                .and_then(|r| r.as_map())
                .and_then(|m| m.get("v"));
            assert_eq!(picked, Some(&SolValue::str("b")), "Switch routed to case b");
        }
        _ => panic!("should complete"),
    }
}

#[test]
fn switch_falls_through_to_default() {
    let program = compile_sugared(json!({
        "nid": "s", "op": "Switch",
        "on": {"pull": "input.kind"},
        "cases": {"a": {"nid":"ca","op":"Call","id":"io.mark_a@1","args":{},"into":"result"}},
        "default": {"nid":"cd","op":"Call","id":"io.mark_default@1","args":{},"into":"result"}
    }));
    let mut reg = Registry::default();
    reg.register("io.mark_a@1", EffectClass::Read, |_| {
        Ok(SolValue::map([("v", SolValue::str("a"))]))
    });
    reg.register("io.mark_default@1", EffectClass::Read, |_| {
        Ok(SolValue::map([("v", SolValue::str("default"))]))
    });
    let mut inst = Instance::new(program, &mut reg);
    let initial = SolValue::map([("input", SolValue::map([("kind", SolValue::str("zzz"))]))]);
    if let TurnOutcome::Completed { bag, .. } = inst.start(initial).unwrap() {
        let v = bag
            .as_map()
            .unwrap()
            .get("result")
            .and_then(|r| r.as_map())
            .and_then(|m| m.get("v"));
        assert_eq!(v, Some(&SolValue::str("default")));
    } else {
        panic!("should complete");
    }
}

#[test]
fn retry_compiles_to_try_and_re_runs_on_transient() {
    let program = compile_sugared(json!({
        "nid": "r", "op": "Retry", "max": 2, "retry_on": ["Tool.Transient"],
        "body": {"nid":"call","op":"Call","id":"tool.flaky@1","args":{},"into":"out"}
    }));
    let mut reg = Registry::default();
    // Fails Tool.Transient on the first call, succeeds on the second.
    let mut attempts = 0u32;
    reg.register("tool.flaky@1", EffectClass::External, move |_| {
        attempts += 1;
        if attempts == 1 {
            Err(ErrV1::new(ReasonCode::ToolTransient, "call", "flaky"))
        } else {
            Ok(SolValue::map([("ok", SolValue::Bool(true))]))
        }
    });
    let mut inst = Instance::new(program, &mut reg);
    match inst.start(SolValue::map::<_, &str>([])).unwrap() {
        TurnOutcome::Completed { bag, .. } => {
            let ok = bag
                .as_map()
                .unwrap()
                .get("out")
                .and_then(|o| o.as_map())
                .and_then(|m| m.get("ok"));
            assert_eq!(
                ok,
                Some(&SolValue::Bool(true)),
                "Retry re-ran and succeeded on attempt 2"
            );
        }
        _ => panic!("should complete after retry"),
    }
}
