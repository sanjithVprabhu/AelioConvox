use aelio_kernel::compile;

#[test]
fn nested_instruction_shapes_are_closed() {
    for invalid in [
        r#"{"nid":"b","op":"Branch","pred":{"lit":true,"pull":"x"},"then":{"nid":"i","op":"Identity"}}"#,
        r#"{"nid":"l","op":"Let","bindings":[{"key":"x","value":{"lit":1},"extra":true}],"body":{"nid":"i","op":"Identity"}}"#,
        r#"{"nid":"o","op":"Once","idem_key":{"template":[],"extra":true},"body":{"nid":"i","op":"Identity"}}"#,
        r#"{"nid":"p","op":"Park","until":{"kind":"event","ms":1}}"#,
    ] {
        let error = compile(invalid).expect_err("nested unknown fields must be rejected");
        assert_eq!(error.code.code(), "Shape");
    }
}

#[test]
fn nids_are_nonempty_and_unique_across_the_plan() {
    let duplicate = r#"{"nid":"same","op":"Seq","steps":[
      {"nid":"same","op":"Identity"}
    ]}"#;
    assert!(compile(duplicate).is_err());
    assert!(compile(r#"{"nid":"","op":"Identity"}"#).is_err());
}

#[test]
fn tee_side_cannot_feed_a_later_main_path_read() {
    let unsafe_plan = r#"{"nid":"seq","op":"Seq","steps":[
      {"nid":"tee","op":"Tee","side_root":"telemetry",
       "body":{"nid":"main","op":"Identity"},
       "side":{"nid":"side","op":"Call","id":"write@1","args":{},"into":"telemetry.score"}},
      {"nid":"consumer","op":"Call","id":"read@1",
       "args":{"score":{"pull":"telemetry.score"}},"into":"out"}
    ]}"#;
    let error = compile(unsafe_plan).expect_err("Tee side must not contaminate later dataflow");
    assert_eq!(error.code.code(), "Shape");
}

#[test]
fn map_import_aliases_are_read_only() {
    let unsafe_plan = r#"{"nid":"map","op":"Map","over":"items","into":"out","max_items":2,
      "imports":{"context":"tenant.context"},
      "body":{"nid":"write","op":"Call","id":"bad@1","args":{},"into":"context.changed"}
    }"#;
    let error = compile(unsafe_plan).expect_err("Map body must not write projected imports");
    assert_eq!(error.code.code(), "Policy");
}
