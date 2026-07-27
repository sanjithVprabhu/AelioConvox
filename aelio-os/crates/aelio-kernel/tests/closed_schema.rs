//! Closed-schema plan-time rejections (App E, §4.4): unknown fields and oversized Const literals
//! must fail at compile/push time, never silently at runtime.

use aelio_kernel::compile;

#[test]
fn unknown_field_is_rejected() {
    // `arg` is a misspelling of `args` — must not silently default to empty.
    let err = compile(r#"{"nid":"c","op":"Call","id":"x@1","arg":{},"into":"out"}"#).unwrap_err();
    assert_eq!(err.code.code(), "Shape");
    assert!(err.detail.contains("unknown field"), "{}", err.detail);
}

#[test]
fn unknown_field_on_control_op_is_rejected() {
    let err = compile(r#"{"nid":"i","op":"Identity","bogus":123}"#).unwrap_err();
    assert!(
        err.detail.contains("unknown field `bogus`"),
        "{}",
        err.detail
    );
}

#[test]
fn known_fields_still_compile() {
    // Sanity: a well-formed node with exactly its allowed fields compiles.
    assert!(compile(r#"{"nid":"c","op":"Call","id":"x@1","args":{},"into":"out"}"#).is_ok());
}

#[test]
fn oversized_const_literal_is_rejected_at_plan_time() {
    // A Const list literal beyond the 10k cap → Budget.Size at compile (plan) time.
    let items: String = (0..10_001)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let plan = format!(r#"{{"nid":"c","op":"Const","v":[{items}]}}"#);
    let err = compile(&plan).unwrap_err();
    assert_eq!(err.code.code(), "Budget.Size", "{}", err.detail);
}
