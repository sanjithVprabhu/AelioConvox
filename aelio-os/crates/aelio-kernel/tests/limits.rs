//! §4.4 structural limits are enforced on every bag commit (§4.2), mapping to Budget.Size (§11).

use aelio_kernel::{compile, EffectClass, Instance, Registry};
use aelio_sol::SolValue;

/// Build a right-nested map `{a:{a:{a:...}}}` of the given depth.
fn nested(depth: usize) -> SolValue {
    let mut v = SolValue::Int(0);
    for _ in 0..depth {
        v = SolValue::map([("a", v)]);
    }
    v
}

/// Returns the error reason-code string, or "OK" if the write completed within limits.
fn write_code(value: SolValue) -> String {
    // A Call whose Read output is written into `out` — the write triggers the §4.4 check.
    let program =
        compile(r#"{"nid":"c","op":"Call","id":"io.big@1","args":{},"into":"out"}"#).unwrap();
    let mut reg = Registry::default();
    reg.register("io.big@1", EffectClass::Read, move |_| Ok(value.clone()));
    let mut inst = Instance::new(program, &mut reg);
    match inst.start(SolValue::map::<_, &str>([])) {
        Ok(_) => "OK".into(),
        Err(e) => e.code.code().to_string(),
    }
}

#[test]
fn depth_over_32_is_rejected() {
    // out + 33 nested = depth 34 at the deepest scalar → over the cap.
    assert_eq!(
        write_code(nested(33)),
        "Budget.Size",
        "deep nest → §4.4 Budget.Size"
    );
}

#[test]
fn map_over_1024_keys_is_rejected() {
    let big = SolValue::Map(
        (0..1100)
            .map(|i| (format!("k{i}"), SolValue::Int(i)))
            .collect(),
    );
    assert_eq!(write_code(big), "Budget.Size");
}

#[test]
fn list_over_10k_is_rejected() {
    let big = SolValue::List((0..10_001).map(SolValue::Int).collect());
    assert_eq!(write_code(big), "Budget.Size");
}

#[test]
fn within_limits_is_accepted() {
    // A modest value writes fine and completes.
    assert_eq!(write_code(nested(5)), "OK");
}
