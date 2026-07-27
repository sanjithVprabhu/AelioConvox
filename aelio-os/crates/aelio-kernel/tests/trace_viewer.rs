//! §26 trace viewer: op-by-op bag writes reconstructed straight from the ledger.

use aelio_kernel::trace::{render, render_string};
use aelio_kernel::{compile, EffectClass, Instance, Registry, TurnOutcome};
use aelio_sol::SolValue;

#[test]
fn trace_shows_op_by_op_bag_writes_from_the_ledger() {
    let program = compile(
        r#"{"nid":"root","op":"Seq","steps":[
             {"nid":"n_read","op":"Call","id":"io.read@1","args":{},"into":"auth"},
             {"nid":"n_mark","op":"Call","id":"tool.mark@1","args":{"a":{"pull":"auth.loggedin"}},"into":"result"}
           ]}"#,
    )
    .unwrap();
    let mut reg = Registry::default();
    reg.register("io.read@1", EffectClass::Read, |_| Ok(SolValue::map([("loggedin", SolValue::Bool(true))])));
    reg.register("tool.mark@1", EffectClass::Write, |_| Ok(SolValue::map([("done", SolValue::Bool(true))])));
    let mut inst = Instance::new(program.clone(), &mut reg);
    assert!(matches!(inst.start(SolValue::map::<_, &str>([])).unwrap(), TurnOutcome::Completed { .. }));

    let entries = render(&program, inst.ledger());

    // The read wrote `auth`; the write wrote `result` — both reconstructed from the ledger.
    assert!(
        entries.iter().any(|e| e.nid.as_deref() == Some("n_read") && e.summary.starts_with("auth :=")),
        "trace shows auth := <io.read output>"
    );
    assert!(
        entries.iter().any(|e| e.nid.as_deref() == Some("n_mark") && e.summary.starts_with("result :=")),
        "trace shows result := <tool.mark output>"
    );
    // The effectful write recorded an intent line (§12.4).
    assert!(entries.iter().any(|e| e.kind == "call_intent"));
    // Turn boundaries + a final bag_hash line are present.
    assert!(entries.first().map(|e| e.kind == "turn_start").unwrap_or(false));
    assert!(entries.iter().any(|e| e.kind == "turn_end" && e.summary.contains("bag_hash=")));

    // The string form renders without panicking and includes the writes.
    let s = render_string(&program, inst.ledger());
    assert!(s.contains("auth :="));
    assert!(s.contains("result :="));
}
