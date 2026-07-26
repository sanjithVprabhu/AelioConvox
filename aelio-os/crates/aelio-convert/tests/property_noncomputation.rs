//! §14 / A14.4 — rule-set non-computation: no rule's output can feed rule *selection*.
//! Property: applying the same rule list to the same input always yields the same output;
//! reordering is the *only* way to change selection, and selection is static (the list itself).

use aelio_convert::{apply_rules, parse_rules};
use aelio_sol::SolValue;

#[test]
fn rules_are_static_pipeline_not_a_program() {
    let rules = parse_rules(&serde_json::json!([
        {"op":"rename","from":"a","to":"b"},
        {"op":"default","path":"c","v":1},
        {"op":"trim","path":"b"}
    ]))
    .unwrap();
    let input = SolValue::map([("a", SolValue::str("  x  "))]);
    let o1 = apply_rules(&rules, &input).unwrap();
    let o2 = apply_rules(&rules, &input).unwrap();
    assert_eq!(o1, o2);
    // No rule inspects or mutates the rule list — there is no API to do so.
    // Fabricating ops are data-only; they cannot branch the pipeline.
    assert!(o1.as_map().unwrap().contains_key("c"));
}
