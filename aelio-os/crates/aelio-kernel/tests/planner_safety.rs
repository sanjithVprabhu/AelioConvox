use aelio_kernel::{compile, InstanceConfig};
use aelio_kernel::{
    driver::Instance,
    registry::{Boundedness, Declaration, EffectClass, Origin, Registry, TargetClass},
};
use aelio_sol::SolValue;
use aelio_store::MemoryStore;

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

#[test]
fn park_inside_once_is_rejected_before_any_effect_can_start() {
    let unsafe_plan = r#"{"nid":"once","op":"Once","idem_key":{"template":[{"lit":"k"}]},
      "body":{"nid":"park","op":"Park","until":{"kind":"event"}}}"#;
    let error = compile(unsafe_plan).expect_err("Once cannot retain an ambiguous parked intent");
    assert_eq!(error.code.code(), "Shape");
    assert!(error.detail.contains("Park is forbidden inside Once"));
}

#[test]
fn production_registry_requires_policy_bounds_imprints_and_tenant_scope() {
    let mut registry = Registry::default();
    let unsafe_effect = Declaration {
        id: "tool.send@1".into(),
        class: TargetClass::Tool,
        input_imprint: "send.in@1".into(),
        output_imprint: "send.out@1".into(),
        boundedness: Boundedness::DeadlineCompliant { max_ms: 1_000 },
        effect_class: EffectClass::External,
        policy_tags: vec![],
        tenant: "tenant-a".into(),
        origin: Origin::Tenant,
    };
    assert!(registry
        .register_declared(unsafe_effect, |_| Ok(SolValue::Null))
        .is_err());

    registry
        .register_declared(
            Declaration {
                id: "io.read@1".into(),
                class: TargetClass::Io,
                input_imprint: "read.in@1".into(),
                output_imprint: "read.out@1".into(),
                boundedness: Boundedness::DeadlineCompliant { max_ms: 1_000 },
                effect_class: EffectClass::Read,
                policy_tags: vec![],
                tenant: "tenant-a".into(),
                origin: Origin::Tenant,
            },
            |_| Ok(SolValue::Null),
        )
        .unwrap();
    let program =
        compile(r#"{"nid":"call","op":"Call","id":"io.read@1","args":{},"into":"out"}"#).unwrap();
    assert!(Instance::with_store(
        program,
        &mut registry,
        Box::new(MemoryStore::new()),
        InstanceConfig {
            tenant: "tenant-b".into(),
            instance_id: "instance-1".into(),
            flow_id: "flow".into(),
            flow_rev: "1".into(),
            event_key_secret: [1; 32],
        }
    )
    .is_err());
}

#[test]
fn registered_flow_call_graph_is_acyclic_and_tenant_scoped() {
    let mut registry = Registry::default();
    for id in ["flow.a@1", "flow.b@1"] {
        registry
            .register_declared(
                Declaration {
                    id: id.into(),
                    class: TargetClass::Flow,
                    input_imprint: "flow.in@1".into(),
                    output_imprint: "flow.out@1".into(),
                    boundedness: Boundedness::RegisteredFlow,
                    effect_class: EffectClass::Pure,
                    policy_tags: vec![],
                    tenant: "tenant-a".into(),
                    origin: Origin::Tenant,
                },
                |_| Ok(SolValue::Null),
            )
            .unwrap();
    }
    registry
        .set_flow_calls("flow.a@1", vec!["flow.b@1".into()])
        .unwrap();
    registry
        .set_flow_calls("flow.b@1", vec!["flow.a@1".into()])
        .unwrap();
    assert!(registry.validate_call_graph("tenant-a").is_err());

    registry.set_flow_calls("flow.b@1", vec![]).unwrap();
    assert!(registry.validate_call_graph("tenant-a").is_ok());
}
