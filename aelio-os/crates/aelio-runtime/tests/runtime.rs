use aelio_runtime::{
    baseline_conversation_flow, Artifact, ArtifactActor, ArtifactClass, ArtifactEffect,
    ArtifactInput, ArtifactInterface, ArtifactPins, ArtifactStatus, ArtifactTier, BoundSpec,
    EffectSpec, FlowPush, HarnessBody, HarnessNode, HarnessSeam, HarnessSink, HarnessSource,
    OriginSpec, Provenance, Runtime, RuntimeConfig, RuntimeError, SandboxCase, SandboxLimits,
    TargetClassSpec, TargetSpec, TurnReply, TurnSubmit, DEFAULT_QUEUE_DEPTH,
};
use aelio_sol::SolValue;
use aelio_store::{EmbeddedStore, Store};

fn config(path: &std::path::Path) -> RuntimeConfig {
    RuntimeConfig {
        data_dir: path.into(),
        host_url: None,
        host_token: None,
        event_key_secret: [21; 32],
        queue_depth: DEFAULT_QUEUE_DEPTH,
    }
}

fn admit(runtime: &Runtime, flow: &FlowPush, cases: Vec<SandboxCase>) {
    let result = runtime
        .gate_flow(
            flow,
            &cases,
            SandboxLimits::default(),
            Some("deployer:test".into()),
        )
        .unwrap();
    assert_eq!(result.record.status, ArtifactStatus::Canary);
}

#[test]
fn flow_push_rejects_unresolved_or_malformed_nominal_boundary_imprints() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(config(directory.path())).unwrap();
    let error = runtime
        .push_flow(FlowPush {
            tenant: "tenant-a".into(),
            flow_id: "bad-imprint".into(),
            flow_rev: "1".into(),
            program: serde_json::json!({"nid":"root","op":"Const","v":{}}),
            targets: vec![TargetSpec {
                id: "tool@1".into(),
                class: TargetClassSpec::Tool,
                effect: EffectSpec::Read,
                input_imprint: "tool.input@1".into(),
                output_imprint: "tool.output@1".into(),
                bounded: BoundSpec::Deadline { max_ms: 1_000 },
                policy_tags: vec![],
                origin: OriginSpec::Tenant,
            }],
            prompts: vec![],
        })
        .unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::Invalid(_) | RuntimeError::NotFound(_)
    ));

    runtime
        .push_flow(FlowPush {
            tenant: "tenant-a".into(),
            flow_id: "nominal-imprint".into(),
            flow_rev: "1".into(),
            program: serde_json::json!({"nid":"root","op":"Const","v":{}}),
            targets: vec![TargetSpec {
                id: "tool@1".into(),
                class: TargetClassSpec::Tool,
                effect: EffectSpec::Pure,
                input_imprint: "aelio.turn.input@1".into(),
                output_imprint: "aelio.turn.output@1".into(),
                bounded: BoundSpec::Cost { max_units: 1 },
                policy_tags: vec![],
                origin: OriginSpec::Tenant,
            }],
            prompts: vec![],
        })
        .expect("resolved nominal target imprints are valid execution contracts");
}

#[test]
fn nominal_target_boundaries_are_resolved_and_enforced_inside_the_gate() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(config(directory.path())).unwrap();
    let flow = FlowPush {
        tenant: "tenant-a".into(),
        flow_id: "nominal-call".into(),
        flow_rev: "1".into(),
        program: serde_json::json!({
            "nid":"call","op":"Call","id":"nominal.tool@1",
            "args":{"payload":{"pull":"turn"}},"into":"result"
        }),
        targets: vec![TargetSpec {
            id: "nominal.tool@1".into(),
            class: TargetClassSpec::Tool,
            effect: EffectSpec::Pure,
            input_imprint: "aelio.turn.input@1".into(),
            output_imprint: "aelio.turn.output@1".into(),
            bounded: BoundSpec::Cost { max_units: 1 },
            policy_tags: vec![],
            origin: OriginSpec::Tenant,
        }],
        prompts: vec![],
    };
    runtime.push_flow(flow.clone()).unwrap();
    let cases = (0..20)
        .map(|index| SandboxCase {
            input: serde_json::json!({"turn":{"index":index}}),
            wakes: vec![],
            expect_park: false,
            expected: serde_json::json!({"result":{"ok":true}}),
            fixtures: vec![aelio_runtime::SandboxFixtureCall {
                target: "nominal.tool@1".into(),
                expected_args_hash: None,
                output: serde_json::json!({"ok":true}),
                usage_tokens: 0,
            }],
        })
        .collect::<Vec<_>>();
    assert_eq!(
        runtime
            .gate_flow(&flow, &cases, SandboxLimits::default(), None)
            .unwrap()
            .record
            .status,
        ArtifactStatus::Canary
    );

    let mut wrong = cases;
    wrong[0].fixtures[0].output = serde_json::json!("not-an-object");
    assert!(runtime
        .gate_flow(&flow, &wrong, SandboxLimits::default(), None)
        .is_err());
}

#[tokio::test]
async fn immutable_flow_executes_and_completed_instance_is_not_reentered() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(config(directory.path())).unwrap();
    let flow = FlowPush {
        tenant: "tenant-a".into(),
        flow_id: "hello".into(),
        flow_rev: "1".into(),
        program: serde_json::json!({"nid":"root","op":"Const","v":{"reply":"hello"}}),
        targets: vec![],
        prompts: vec![],
    };
    runtime.push_flow(flow.clone()).unwrap();
    runtime.push_flow(flow.clone()).unwrap();
    let registered = runtime
        .artifact_repository()
        .unwrap()
        .get("tenant-a", "hello", 1)
        .unwrap()
        .expect("unified flow artifact");
    assert_eq!(registered.status, ArtifactStatus::Proposed);
    assert_eq!(registered.artifact.class, ArtifactClass::Flow);
    assert!(matches!(
        runtime
            .submit(TurnSubmit {
                tenant: "tenant-a".into(),
                instance_id: "pre-gate".into(),
                flow_id: "hello".into(),
                flow_rev: "1".into(),
                input: serde_json::json!({}),
            })
            .await,
        Err(RuntimeError::Conflict(detail)) if detail.contains("only canary/promoted")
    ));
    admit(
        &runtime,
        &flow,
        (0..20)
            .map(|index| SandboxCase {
                input: serde_json::json!({"case":index}),
                wakes: vec![],
                expect_park: false,
                expected: serde_json::json!({"reply":"hello"}),
                fixtures: vec![],
            })
            .collect(),
    );
    let reply = runtime
        .submit(TurnSubmit {
            tenant: "tenant-a".into(),
            instance_id: "instance-1".into(),
            flow_id: "hello".into(),
            flow_rev: "1".into(),
            input: serde_json::json!({}),
        })
        .await
        .unwrap();
    let TurnReply::Completed { bag, .. } = reply else {
        panic!("const flow must complete");
    };
    assert_eq!(bag["reply"], "hello");
    assert!(matches!(
        runtime
            .submit(TurnSubmit {
                tenant: "tenant-a".into(),
                instance_id: "instance-1".into(),
                flow_id: "hello".into(),
                flow_rev: "1".into(),
                input: serde_json::json!({}),
            })
            .await,
        Err(RuntimeError::Conflict(_))
    ));
}

#[test]
fn tool_proxies_are_runtime_owned_gated_and_effect_approval_sensitive() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(config(directory.path())).unwrap();
    let read = runtime
        .install_tool_proxy(
            aelio_runtime::ToolProxySpec {
                tenant: "tenant-a".into(),
                tool_id: "catalog.lookup".into(),
                version: 1,
                effect: ArtifactEffect::Read,
                policy_tags: vec![],
            },
            None,
        )
        .unwrap();
    assert_eq!(read.status, ArtifactStatus::Canary);
    let effectful = aelio_runtime::ToolProxySpec {
        tenant: "tenant-a".into(),
        tool_id: "message.send".into(),
        version: 1,
        effect: ArtifactEffect::External,
        policy_tags: vec!["message.send".into()],
    };
    assert!(runtime.install_tool_proxy(effectful.clone(), None).is_err());
    assert_eq!(
        runtime
            .install_tool_proxy(effectful, Some("catalog-deployer".into()))
            .unwrap()
            .status,
        ArtifactStatus::Canary
    );
}

#[tokio::test]
async fn parked_actor_resumes_after_runtime_and_database_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let flow = FlowPush {
        tenant: "tenant-a".into(),
        flow_id: "login".into(),
        flow_rev: "1".into(),
        program: serde_json::json!({
            "nid":"root","op":"Seq","steps":[
                {"nid":"wait","op":"Park","until":{"kind":"event"},"into":"answer"},
                {"nid":"done","op":"Identity"}
            ]
        }),
        targets: vec![],
        prompts: vec![],
    };
    {
        let runtime = Runtime::open(config(directory.path())).unwrap();
        runtime.push_flow(flow.clone()).unwrap();
        admit(
            &runtime,
            &flow,
            (0..20)
                .map(|index| SandboxCase {
                    input: serde_json::json!({"case":index}),
                    wakes: vec![serde_json::json!({"otp":"123456"})],
                    expect_park: false,
                    expected: serde_json::json!({"answer":{"otp":"123456"}}),
                    fixtures: vec![],
                })
                .collect(),
        );
        assert!(matches!(
            runtime
                .submit(TurnSubmit {
                    tenant: "tenant-a".into(),
                    instance_id: "login-1".into(),
                    flow_id: "login".into(),
                    flow_rev: "1".into(),
                    input: serde_json::json!({}),
                })
                .await
                .unwrap(),
            TurnReply::Parked { .. }
        ));
    }

    let runtime = Runtime::open(config(directory.path())).unwrap();
    let reply = runtime
        .submit(TurnSubmit {
            tenant: "tenant-a".into(),
            instance_id: "login-1".into(),
            flow_id: "login".into(),
            flow_rev: "1".into(),
            input: serde_json::json!({"otp":"123456"}),
        })
        .await
        .unwrap();
    let TurnReply::Completed { bag, .. } = reply else {
        panic!("wake must complete");
    };
    assert_eq!(bag["answer"]["otp"], "123456");
}

#[tokio::test]
async fn parked_child_harness_resumes_exact_frame_after_database_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let child = FlowPush {
        tenant: "tenant-a".into(),
        flow_id: "login.child".into(),
        flow_rev: "1".into(),
        program: serde_json::json!({
            "nid":"root","op":"Seq","steps":[
                {"nid":"wait","op":"Park","until":{"kind":"event"},"into":"answer"},
                {"nid":"done","op":"Identity"}
            ]
        }),
        targets: vec![],
        prompts: vec![],
    };
    let harness = HarnessBody {
        nodes: vec![HarnessNode {
            nid: "login".into(),
            artifact: "login.child@1".into(),
        }],
        seams: vec![
            HarnessSeam {
                from: HarnessSource::ParentInput {
                    slot: "turn".into(),
                },
                to: HarnessSink::NodeInput {
                    node: "login".into(),
                    slot: "turn".into(),
                },
                from_imprint: "aelio.turn.input@1".into(),
                to_imprint: "aelio.turn.input@1".into(),
                converter: None,
            },
            HarnessSeam {
                from: HarnessSource::NodeOutput {
                    node: "login".into(),
                },
                to: HarnessSink::ParentOutput,
                from_imprint: "aelio.turn.output@1".into(),
                to_imprint: "aelio.turn.output@1".into(),
                converter: None,
            },
        ],
    };
    let artifact = Artifact::new(
        "login.harness",
        1,
        ArtifactClass::Harness,
        ArtifactTier::Auto,
        "1",
        env!("CARGO_PKG_VERSION"),
        ArtifactInterface {
            inputs: vec![ArtifactInput {
                name: "turn".into(),
                imprint: "aelio.turn.input@1".into(),
                required: true,
                sensitivity: "internal".into(),
            }],
            output: "aelio.turn.output@1".into(),
        },
        "durable composite login",
        vec!["test".into()],
        vec![ArtifactEffect::Pure],
        ArtifactPins {
            artifacts: vec!["login.child@1".into()],
            ..ArtifactPins::default()
        },
        vec![],
        serde_json::json!({"harness":harness}),
        Provenance::default(),
    )
    .unwrap();
    let gate_cases = (0..20)
        .map(|index| SandboxCase {
            input: serde_json::json!({"turn":{"case":index}}),
            wakes: vec![serde_json::json!({"otp":"123456"})],
            expect_park: false,
            expected: serde_json::json!({"answer":{"otp":"123456"}}),
            fixtures: vec![],
        })
        .collect::<Vec<_>>();
    {
        let runtime = Runtime::open(config(directory.path())).unwrap();
        runtime.push_flow(child.clone()).unwrap();
        admit(&runtime, &child, gate_cases.clone());
        runtime
            .artifact_repository()
            .unwrap()
            .put_proposed("tenant-a", artifact, ArtifactActor::System)
            .unwrap();
        assert_eq!(
            runtime
                .gate_harness(
                    "tenant-a",
                    "login.harness",
                    1,
                    &harness,
                    &gate_cases,
                    SandboxLimits::default(),
                    None,
                )
                .unwrap()
                .record
                .status,
            ArtifactStatus::Canary
        );
        let TurnReply::Parked { park_nid, .. } = runtime
            .submit(TurnSubmit {
                tenant: "tenant-a".into(),
                instance_id: "login-harness-1".into(),
                flow_id: "login.harness".into(),
                flow_rev: "1".into(),
                input: serde_json::json!({"turn":{"case":"live"}}),
            })
            .await
            .unwrap()
        else {
            panic!("nested login child must park");
        };
        assert_eq!(park_nid, "login/wait");
    }

    // Simulate the narrow crash window where the child continuation was durably parked but the
    // parent frame had only committed "prepared". A retry must replay Parked; it must not consume
    // the original deterministic child arguments as an OTP wake.
    let mut store = EmbeddedStore::open(directory.path()).unwrap();
    let row = store
        .get("tenant-a", "harness_continuations", "login-harness-1")
        .unwrap()
        .unwrap();
    let mut state = row.value.as_map().unwrap().clone();
    state.insert(
        "phase".into(),
        SolValue::map([
            ("node_index", SolValue::Int(0)),
            ("phase", SolValue::str("prepared")),
        ]),
    );
    state.insert("state_hash".into(), SolValue::str(""));
    let hash = aelio_sol::value_hash(&SolValue::Map(state.clone()));
    state.insert("state_hash".into(), SolValue::str(hash));
    store
        .cas(
            "tenant-a",
            "harness_continuations",
            "login-harness-1",
            row.version,
            SolValue::Map(state),
        )
        .unwrap();

    {
        let runtime = Runtime::open(config(directory.path())).unwrap();
        let TurnReply::Parked { park_nid, .. } = runtime
            .submit(TurnSubmit {
                tenant: "tenant-a".into(),
                instance_id: "login-harness-1".into(),
                flow_id: "login.harness".into(),
                flow_rev: "1".into(),
                input: serde_json::json!({"turn":{"case":"request-retry"}}),
            })
            .await
            .unwrap()
        else {
            panic!("prepared-frame recovery must replay the parked child");
        };
        assert_eq!(park_nid, "login/wait");
    }

    // Simulate the second crash window: the child completed, but the parent had not yet copied its
    // output/advanced its frame. The replayable completed-child record must let the parent finish
    // without executing the child again.
    let mut store = EmbeddedStore::open(directory.path()).unwrap();
    let row = store
        .get("tenant-a", "harness_continuations", "login-harness-1")
        .unwrap()
        .unwrap();
    let mut state = row.value.as_map().unwrap().clone();
    state.insert(
        "phase".into(),
        SolValue::map([
            ("node_index", SolValue::Int(0)),
            ("phase", SolValue::str("prepared")),
        ]),
    );
    state.insert("state_hash".into(), SolValue::str(""));
    let state_hash = aelio_sol::value_hash(&SolValue::Map(state.clone()));
    state.insert("state_hash".into(), SolValue::str(state_hash));
    store
        .cas(
            "tenant-a",
            "harness_continuations",
            "login-harness-1",
            row.version,
            SolValue::Map(state),
        )
        .unwrap();
    let harness_hash = Runtime::open(config(directory.path()))
        .unwrap()
        .artifact_repository()
        .unwrap()
        .get("tenant-a", "login.harness", 1)
        .unwrap()
        .unwrap()
        .artifact
        .hash;
    let child_identity = SolValue::map([
        ("domain", SolValue::str("aelio.sub_harness.v1")),
        ("tenant", SolValue::str("tenant-a")),
        ("parent", SolValue::str("login-harness-1")),
        ("harness_hash", SolValue::str(harness_hash)),
        ("node_index", SolValue::Int(0)),
        ("artifact", SolValue::str("login.child@1")),
    ]);
    let child_instance_id = format!("h-{}", aelio_sol::value_hash(&child_identity));
    {
        let runtime = Runtime::open(config(directory.path())).unwrap();
        assert!(matches!(
            runtime
                .submit(TurnSubmit {
                    tenant: "tenant-a".into(),
                    instance_id: child_instance_id,
                    flow_id: "login.child".into(),
                    flow_rev: "1".into(),
                    input: serde_json::json!({"otp":"123456"}),
                })
                .await
                .unwrap(),
            TurnReply::Completed { .. }
        ));
    }

    let runtime = Runtime::open(config(directory.path())).unwrap();
    let TurnReply::Completed { bag, .. } = runtime
        .submit(TurnSubmit {
            tenant: "tenant-a".into(),
            instance_id: "login-harness-1".into(),
            flow_id: "login.harness".into(),
            flow_rev: "1".into(),
            input: serde_json::json!({"request":"retry-after-child-complete"}),
        })
        .await
        .unwrap()
    else {
        panic!("wake must resume the exact nested child frame");
    };
    assert_eq!(bag["answer"]["otp"], "123456");
    assert_eq!(bag["turn"]["case"], "live");
}

#[tokio::test]
async fn registered_prompt_is_composed_inside_rust_with_a_verified_hash() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(config(directory.path())).unwrap();
    let stock = baseline_conversation_flow("tenant-a");
    let flow = FlowPush {
        tenant: "tenant-a".into(),
        flow_id: "prompt".into(),
        flow_rev: "1".into(),
        program: serde_json::json!({
            "nid":"compose",
            "op":"Call",
            "id":"aelio.prompt.conversation@1",
            "args":{"message":{"pull":"message"}},
            "into":"composed"
        }),
        // The model pin is part of the admitted prompt contract even though this small test only
        // executes the deterministic composition target.
        targets: stock.targets,
        prompts: stock.prompts,
    };
    runtime.push_flow(flow.clone()).unwrap();
    admit(
        &runtime,
        &flow,
        (0..20)
            .map(|index| SandboxCase {
                input: serde_json::json!({"message":format!("Hello {index}")}),
                wakes: vec![],
                expect_park: false,
                expected: serde_json::json!({"composed":{"template":"aelio.template.conversation@1"}}),
                fixtures: vec![],
            })
            .collect(),
    );

    let TurnReply::Completed { bag, .. } = runtime
        .submit(TurnSubmit {
            tenant: "tenant-a".into(),
            instance_id: "prompt-1".into(),
            flow_id: "prompt".into(),
            flow_rev: "1".into(),
            input: serde_json::json!({"message":"Hello"}),
        })
        .await
        .unwrap()
    else {
        panic!("prompt composition must complete");
    };
    assert_eq!(bag["composed"]["template"], "aelio.template.conversation@1");
    assert!(bag["composed"]["prompt"]
        .as_str()
        .is_some_and(|prompt| prompt.contains("\"Hello\"")));
    assert!(bag["composed"]["prompt_hash"]
        .as_str()
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())));
}
