use aelio_runtime::{
    ArtifactEffect, ArtifactInput, ArtifactStatus, BuildAction, BuildBudget, BuildExample,
    BuildOracle, BuildOracleRequest, BuildOracleResponse, BuildPolicy, BuildReactionOutput,
    BuildReactionUsage, BuildScope, BuildSpec, BuildSpecDraft, BuildStage, DecompositionSeam,
    FlowPush, HarnessNode, HarnessSink, HarnessSource, Runtime, RuntimeConfig, RuntimeError,
    SandboxCase, SandboxLimits, SelectionChoice, DEFAULT_QUEUE_DEPTH,
};
use std::collections::VecDeque;
use std::sync::Mutex;

struct ScriptedBuildOracle {
    responses: Mutex<VecDeque<String>>,
    retries: Mutex<Vec<bool>>,
}

impl BuildOracle for ScriptedBuildOracle {
    fn complete(
        &self,
        request: BuildOracleRequest<'_>,
    ) -> Result<BuildOracleResponse, RuntimeError> {
        self.retries.lock().unwrap().push(request.retry);
        let text = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| RuntimeError::Internal("script exhausted".into()))?;
        Ok(BuildOracleResponse {
            text,
            usage_tokens: 5,
        })
    }
}

fn config(path: &std::path::Path) -> RuntimeConfig {
    RuntimeConfig {
        data_dir: path.into(),
        host_url: None,
        host_token: None,
        event_key_secret: [31; 32],
        queue_depth: DEFAULT_QUEUE_DEPTH,
    }
}

fn constant_flow(tenant: &str, id: &str) -> FlowPush {
    FlowPush {
        tenant: tenant.into(),
        flow_id: id.into(),
        flow_rev: "1".into(),
        program: serde_json::json!({"nid":"root","op":"Const","v":{"ok":true}}),
        targets: vec![],
        prompts: vec![],
    }
}

fn cases() -> Vec<SandboxCase> {
    (0..20)
        .map(|index| SandboxCase {
            input: serde_json::json!({"turn":index}),
            wakes: vec![],
            expect_park: false,
            expected: serde_json::json!({"ok":true}),
            fixtures: vec![],
        })
        .collect()
}

fn spec() -> BuildSpec {
    BuildSpec::seal(BuildSpecDraft {
        name: "flow.built".into(),
        description: "immutable flow flow seed".into(),
        inputs: vec![ArtifactInput {
            name: "turn".into(),
            imprint: "aelio.turn.input@1".into(),
            required: true,
            sensitivity: "internal".into(),
        }],
        output: "aelio.turn.output@1".into(),
        budget: BuildBudget {
            max_depth: 4,
            max_children: 4,
            max_llm_calls: 4,
            max_tokens: 10_000,
            max_reactions: 100,
            max_wall_ms: 60_000,
        },
        scope: BuildScope {
            tenant: "tenant-a".into(),
            registries: vec!["flow.*".into()],
        },
        policy: BuildPolicy {
            principal_grants: vec![ArtifactEffect::Pure],
            allowed_effects: vec![ArtifactEffect::Pure],
            denied_effects: vec![ArtifactEffect::Write, ArtifactEffect::External],
        },
        examples: (0..20)
            .map(|index| BuildExample {
                inputs: serde_json::json!({"turn":{"turn":index}}),
                output: serde_json::json!({"ok":true}),
                negative: index == 0,
                fixtures: vec![],
            })
            .collect(),
    })
    .unwrap()
}

#[test]
fn root_builder_resumes_pending_oracle_and_rejects_semantically_wrong_composition() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(config(directory.path())).unwrap();

    // One admitted candidate makes the select→compose direct path available.
    let mut candidate = constant_flow("tenant-a", "flow.seed");
    candidate.program = serde_json::json!({"nid":"root","op":"Const","v":{"ok":false}});
    runtime.push_flow(candidate.clone()).unwrap();
    let candidate_cases = cases()
        .into_iter()
        .map(|mut case| {
            case.expected = serde_json::json!({"ok":false});
            case
        })
        .collect::<Vec<_>>();
    let admitted = runtime
        .gate_flow(&candidate, &candidate_cases, SandboxLimits::default(), None)
        .unwrap();
    assert_eq!(admitted.record.status, ArtifactStatus::Canary);

    let demand = runtime
        .record_capability_request(
            "tenant-a",
            aelio_runtime::CapabilityRequestDraft {
                normalized_need: "compose a constant successful conversation flow".into(),
                inputs: spec().draft.inputs,
                output: spec().draft.output,
                allowed_effects: spec().draft.policy.allowed_effects,
                requester: "test".into(),
                reason: aelio_runtime::CapabilityReason::MissingCapability,
                evidence_refs: vec!["turn:test".into()],
            },
        )
        .unwrap();
    let job = runtime
        .queue_capability_build("tenant-a", &demand.request_id, spec())
        .unwrap()
        .job;
    assert_eq!(job.stage, BuildStage::Admit);
    assert!(matches!(
        runtime
            .advance_build("tenant-a", &job.build_id, None)
            .unwrap(),
        BuildAction::Progress { .. }
    ));
    assert!(matches!(
        runtime
            .advance_build("tenant-a", &job.build_id, None)
            .unwrap(),
        BuildAction::Progress { .. }
    ));
    assert!(matches!(
        runtime
            .advance_build("tenant-a", &job.build_id, None)
            .unwrap(),
        BuildAction::Progress { .. }
    ));
    let select = runtime
        .advance_build("tenant-a", &job.build_id, None)
        .unwrap();
    let BuildAction::Oracle {
        reaction: select_reaction,
        ..
    } = select
    else {
        panic!("select must be a persisted oracle action");
    };

    // Reopening the process returns the exact same pending reaction, never a new paid call.
    drop(runtime);
    let runtime = Runtime::open(config(directory.path())).unwrap();
    let resumed = runtime
        .advance_build("tenant-a", &job.build_id, None)
        .unwrap();
    let BuildAction::Oracle {
        reaction: resumed_reaction,
        ..
    } = resumed
    else {
        panic!("restart must resume the oracle intent");
    };
    assert_eq!(select_reaction, resumed_reaction);

    runtime
        .submit_build_reaction(
            "tenant-a",
            &job.build_id,
            &select_reaction.reaction_id,
            BuildReactionOutput::Selection {
                selected: vec![SelectionChoice {
                    id: "flow.seed@1".into(),
                    score: 0.99,
                    role: "seed".into(),
                }],
                runners_up: vec![],
                unmet: vec![],
                undeterminable: false,
            },
            BuildReactionUsage {
                model_calls: 1,
                tokens: 10,
                wall_ms: 2,
            },
        )
        .unwrap();
    let compose = runtime
        .advance_build("tenant-a", &job.build_id, None)
        .unwrap();
    let BuildAction::Oracle {
        reaction: compose_reaction,
        ..
    } = compose
    else {
        panic!("compose must be a persisted oracle action");
    };
    runtime
        .submit_build_reaction(
            "tenant-a",
            &job.build_id,
            &compose_reaction.reaction_id,
            BuildReactionOutput::Composition {
                tree: vec![HarnessNode {
                    nid: "seed".into(),
                    artifact: "flow.seed@1".into(),
                }],
                seams: vec![
                    DecompositionSeam {
                        from: HarnessSource::ParentInput {
                            slot: "turn".into(),
                        },
                        to: HarnessSink::NodeInput {
                            node: "seed".into(),
                            slot: "turn".into(),
                        },
                    },
                    DecompositionSeam {
                        from: HarnessSource::NodeOutput {
                            node: "seed".into(),
                        },
                        to: HarnessSink::ParentOutput,
                    },
                ],
                undeterminable: false,
            },
            BuildReactionUsage {
                model_calls: 1,
                tokens: 20,
                wall_ms: 3,
            },
        )
        .unwrap();

    assert!(matches!(
        runtime
            .advance_build("tenant-a", &job.build_id, None)
            .unwrap(),
        BuildAction::Progress { .. }
    )); // validate
    assert!(matches!(
        runtime
            .advance_build("tenant-a", &job.build_id, None)
            .unwrap(),
        BuildAction::Progress { .. }
    )); // assemble
    assert!(matches!(
        runtime
            .advance_build("tenant-a", &job.build_id, None)
            .unwrap(),
        BuildAction::Progress { .. }
    )); // gate rejects the semantically wrong selected primitive
    let completed = runtime
        .advance_build("tenant-a", &job.build_id, None)
        .unwrap();
    let BuildAction::Complete { job, result } = completed else {
        panic!("gate must complete the build");
    };
    assert_eq!(job.stage, BuildStage::Complete);
    assert!(
        matches!(result, aelio_runtime::BuildResult::Failed { .. }),
        "unexpected worker result: {result:?}"
    );
    let artifact = runtime
        .artifact_repository()
        .unwrap()
        .get("tenant-a", "flow.built", 1)
        .unwrap()
        .unwrap();
    assert_ne!(artifact.status, ArtifactStatus::Canary);
    assert_eq!(
        artifact.artifact.class,
        aelio_runtime::ArtifactClass::Harness
    );
    assert_eq!(
        aelio_runtime::CapabilityRequestRepository::open(
            aelio_store::EmbeddedStore::open(directory.path()).unwrap()
        )
        .unwrap()
        .get("tenant-a", &demand.request_id)
        .unwrap()
        .unwrap()
        .status,
        aelio_runtime::CapabilityStatus::Failed
    );
}

#[test]
fn bounded_worker_uses_one_closed_schema_retry_and_accounts_for_every_model_call() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(config(directory.path())).unwrap();
    let mut candidate = constant_flow("tenant-a", "flow.seed");
    candidate.program = serde_json::json!({"nid":"root","op":"Const","v":{"ok":false}});
    runtime.push_flow(candidate.clone()).unwrap();
    let candidate_cases = cases()
        .into_iter()
        .map(|mut case| {
            case.expected = serde_json::json!({"ok":false});
            case
        })
        .collect::<Vec<_>>();
    assert_eq!(
        runtime
            .gate_flow(&candidate, &candidate_cases, SandboxLimits::default(), None,)
            .unwrap()
            .record
            .status,
        ArtifactStatus::Canary
    );
    let job = runtime.submit_build(spec()).unwrap();
    let oracle = ScriptedBuildOracle {
        responses: Mutex::new(VecDeque::from([
            serde_json::json!({"selected":[{"id":"invented@1","score":0.99,"role":"seed"}],"runners_up":[],"unmet":[],"undeterminable":false}).to_string(),
            serde_json::json!({"selected":[{"id":"flow.seed@1","score":0.99,"role":"seed"}],"runners_up":[],"unmet":[],"undeterminable":false}).to_string(),
            serde_json::json!({
                "tree":[{"nid":"seed","artifact":"flow.seed@1"}],
                "seams":[
                    {"from":{"kind":"parent_input","slot":"turn"},"to":{"kind":"node_input","node":"seed","slot":"turn"}},
                    {"from":{"kind":"node_output","node":"seed"},"to":{"kind":"parent_output"}}
                ],
                "undeterminable":false
            }).to_string(),
        ])),
        retries: Mutex::new(Vec::new()),
    };
    let BuildAction::Complete { job, result } = runtime
        .run_build_worker("tenant-a", &job.build_id, None, &oracle, 64)
        .unwrap()
    else {
        panic!("worker must finish the scripted build");
    };
    assert!(
        matches!(result, aelio_runtime::BuildResult::Failed { .. }),
        "unexpected worker result: {result:?}"
    );
    assert_eq!(job.cost.llm_calls, 3);
    assert_eq!(job.cost.tokens, 15);
    assert_eq!(*oracle.retries.lock().unwrap(), [false, true, false]);
}

#[test]
fn model_confidence_cannot_override_low_kernel_similarity() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(config(directory.path())).unwrap();
    let mut candidate = constant_flow("tenant-a", "flow.seed");
    candidate.program = serde_json::json!({"nid":"root","op":"Const","v":{"ok":false}});
    runtime.push_flow(candidate.clone()).unwrap();
    let negative_cases = cases()
        .into_iter()
        .map(|mut case| {
            case.expected = serde_json::json!({"ok":false});
            case
        })
        .collect::<Vec<_>>();
    runtime
        .gate_flow(&candidate, &negative_cases, SandboxLimits::default(), None)
        .unwrap();

    let mut draft = spec().draft;
    draft.description = "send a quarterly customer retention report".into();
    let job = runtime
        .submit_build(BuildSpec::seal(draft).unwrap())
        .unwrap();
    for _ in 0..3 {
        runtime
            .advance_build("tenant-a", &job.build_id, None)
            .unwrap();
    }
    let BuildAction::Oracle { reaction, .. } = runtime
        .advance_build("tenant-a", &job.build_id, None)
        .unwrap()
    else {
        panic!("search must produce a selection reaction");
    };
    let updated = runtime
        .submit_build_reaction(
            "tenant-a",
            &job.build_id,
            &reaction.reaction_id,
            BuildReactionOutput::Selection {
                selected: vec![SelectionChoice {
                    id: "flow.seed@1".into(),
                    score: 1.0,
                    role: "model_claims_perfect".into(),
                }],
                runners_up: vec![],
                unmet: vec![],
                undeterminable: false,
            },
            BuildReactionUsage::default(),
        )
        .unwrap();
    assert_eq!(updated.stage, BuildStage::Decompose);
    assert!(updated
        .workspace
        .diagnostic
        .as_deref()
        .is_some_and(|detail| detail.contains("select_abstained")));
}

#[test]
fn atomic_failure_is_durable_and_creates_one_readable_demand() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(config(directory.path())).unwrap();
    let job = runtime.submit_build(spec()).unwrap();
    for _ in 0..3 {
        runtime
            .advance_build("tenant-a", &job.build_id, None)
            .unwrap();
    }
    let action = runtime
        .advance_build("tenant-a", &job.build_id, None)
        .unwrap();
    let BuildAction::Oracle { reaction, .. } = action else {
        panic!("an empty registry must go directly to decomposition");
    };
    runtime
        .submit_build_reaction(
            "tenant-a",
            &job.build_id,
            &reaction.reaction_id,
            BuildReactionOutput::Decomposition {
                verdict: aelio_runtime::DecompositionVerdict::Atomic,
                undeterminable: false,
                children: vec![],
                seams: vec![],
                parent_complexity: 0,
                child_complexities: std::collections::BTreeMap::new(),
                detail: "requires a human-authored primitive".into(),
            },
            BuildReactionUsage {
                model_calls: 1,
                tokens: 7,
                wall_ms: 2,
            },
        )
        .unwrap();
    let completed = runtime
        .advance_build("tenant-a", &job.build_id, None)
        .unwrap();
    let BuildAction::Complete { result, .. } = completed else {
        panic!("failed build must finish durably");
    };
    let aelio_runtime::BuildResult::Failed { failure, cost, .. } = result else {
        panic!("atomic miss must fail honestly");
    };
    assert_eq!(cost.llm_calls, 1);
    assert_eq!(cost.tokens, 7);
    assert_eq!(failure.insufficiency_ids.len(), 1);
    let demands = runtime.list_capability_requests("tenant-a", 10).unwrap();
    assert_eq!(demands.len(), 1);
    assert_eq!(demands[0].request_id, failure.insufficiency_ids[0]);

    // Completion and demand creation are idempotent from the public control plane.
    assert!(matches!(
        runtime
            .advance_build("tenant-a", &job.build_id, None)
            .unwrap(),
        BuildAction::Complete { .. }
    ));
    assert_eq!(
        runtime.list_capability_requests("tenant-a", 10).unwrap()[0].demand_count,
        1
    );
}

#[test]
fn bootstrap_root_harness_is_a_promoted_self_describing_vendor_axiom() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(config(directory.path())).unwrap();
    let record = runtime
        .artifact_repository()
        .unwrap()
        .get("aelio.vendor", "aelio.root_harness", 2)
        .unwrap()
        .expect("root harness registry entry #1");
    assert_eq!(record.status, ArtifactStatus::Promoted);
    assert_eq!(record.artifact.class, aelio_runtime::ArtifactClass::Harness);
    assert_eq!(
        record.artifact.interface.inputs[0].imprint,
        "aelio.build_spec@2"
    );
    assert_eq!(record.artifact.interface.output, "aelio.build_result@2");
    let declared = record.artifact.body["stages"].as_array().unwrap();
    assert_eq!(declared.len(), 12);
    let self_spec: BuildSpec =
        serde_json::from_value(record.artifact.body["self_spec"].clone()).unwrap();
    self_spec.validate().unwrap();
}

fn child_spec(name: &str, input_name: &str, input_imprint: &str) -> BuildSpec {
    let parent = spec();
    BuildSpec::seal(BuildSpecDraft {
        name: name.into(),
        description: format!("bounded child {name}"),
        inputs: vec![ArtifactInput {
            name: input_name.into(),
            imprint: input_imprint.into(),
            required: true,
            sensitivity: "internal".into(),
        }],
        output: "aelio.turn.output@1".into(),
        budget: parent.draft.budget,
        scope: parent.draft.scope,
        policy: parent.draft.policy,
        examples: (0..20)
            .map(|index| BuildExample {
                inputs: serde_json::json!({(input_name):{"index":index}}),
                output: serde_json::json!({"ok":true}),
                negative: index == 0,
                fixtures: vec![],
            })
            .collect(),
    })
    .unwrap()
}

#[test]
fn decomposition_proves_typed_closure_and_admits_children_in_topological_order() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(config(directory.path())).unwrap();
    let job = runtime.submit_build(spec()).unwrap();
    for _ in 0..3 {
        runtime
            .advance_build("tenant-a", &job.build_id, None)
            .unwrap();
    }
    let BuildAction::Oracle { reaction, .. } = runtime
        .advance_build("tenant-a", &job.build_id, None)
        .unwrap()
    else {
        panic!("decomposition reaction expected");
    };
    let source = child_spec("flow.child_source", "turn", "aelio.turn.input@1");
    let sink = child_spec("flow.child_sink", "intermediate", "aelio.turn.output@1");
    runtime
        .submit_build_reaction(
            "tenant-a",
            &job.build_id,
            &reaction.reaction_id,
            BuildReactionOutput::Decomposition {
                verdict: aelio_runtime::DecompositionVerdict::Children,
                undeterminable: false,
                children: vec![sink, source], // deliberately reverse emission order
                seams: vec![
                    aelio_runtime::DecompositionSeam {
                        from: aelio_runtime::HarnessSource::ParentInput {
                            slot: "turn".into(),
                        },
                        to: aelio_runtime::HarnessSink::NodeInput {
                            node: "flow.child_source".into(),
                            slot: "turn".into(),
                        },
                    },
                    aelio_runtime::DecompositionSeam {
                        from: aelio_runtime::HarnessSource::NodeOutput {
                            node: "flow.child_source".into(),
                        },
                        to: aelio_runtime::HarnessSink::NodeInput {
                            node: "flow.child_sink".into(),
                            slot: "intermediate".into(),
                        },
                    },
                    aelio_runtime::DecompositionSeam {
                        from: aelio_runtime::HarnessSource::NodeOutput {
                            node: "flow.child_sink".into(),
                        },
                        to: aelio_runtime::HarnessSink::ParentOutput,
                    },
                ],
                parent_complexity: 10,
                child_complexities: std::collections::BTreeMap::from([
                    ("flow.child_source".into(), 3),
                    ("flow.child_sink".into(), 4),
                ]),
                detail: String::new(),
            },
            BuildReactionUsage {
                model_calls: 1,
                tokens: 10,
                wall_ms: 1,
            },
        )
        .unwrap();
    let BuildAction::Progress { job } = runtime
        .advance_build("tenant-a", &job.build_id, None)
        .unwrap()
    else {
        panic!("admit_children should progress");
    };
    assert_eq!(job.stage, BuildStage::Recurse);
    assert_eq!(
        job.workspace.topological_children,
        ["flow.child_source", "flow.child_sink"]
    );
    assert_eq!(job.workspace.child_build_ids.len(), 2);
    assert!(job
        .workspace
        .resolved_seams
        .iter()
        .all(|seam| { seam.from_imprint == seam.to_imprint && seam.converter.is_none() }));
}

#[tokio::test]
async fn recursive_build_assembles_executes_and_gates_a_harness_end_to_end() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(config(directory.path())).unwrap();
    let job = runtime.submit_build(spec()).unwrap();
    for _ in 0..3 {
        runtime
            .advance_build("tenant-a", &job.build_id, None)
            .unwrap();
    }
    let BuildAction::Oracle { reaction, .. } = runtime
        .advance_build("tenant-a", &job.build_id, None)
        .unwrap()
    else {
        panic!("decomposition reaction expected");
    };
    runtime
        .submit_build_reaction(
            "tenant-a",
            &job.build_id,
            &reaction.reaction_id,
            BuildReactionOutput::Decomposition {
                verdict: aelio_runtime::DecompositionVerdict::Children,
                undeterminable: false,
                children: vec![child_spec(
                    "flow.child_source",
                    "turn",
                    "aelio.turn.input@1",
                )],
                seams: vec![
                    aelio_runtime::DecompositionSeam {
                        from: aelio_runtime::HarnessSource::ParentInput {
                            slot: "turn".into(),
                        },
                        to: aelio_runtime::HarnessSink::NodeInput {
                            node: "flow.child_source".into(),
                            slot: "turn".into(),
                        },
                    },
                    aelio_runtime::DecompositionSeam {
                        from: aelio_runtime::HarnessSource::NodeOutput {
                            node: "flow.child_source".into(),
                        },
                        to: aelio_runtime::HarnessSink::ParentOutput,
                    },
                ],
                parent_complexity: 10,
                child_complexities: std::collections::BTreeMap::from([(
                    "flow.child_source".into(),
                    3,
                )]),
                detail: String::new(),
            },
            BuildReactionUsage {
                model_calls: 1,
                tokens: 10,
                wall_ms: 1,
            },
        )
        .unwrap();
    let BuildAction::Progress { job } = runtime
        .advance_build("tenant-a", &job.build_id, None)
        .unwrap()
    else {
        panic!("child admission must progress");
    };

    // Admit a reusable primitive only after decomposition, then allow the child to resolve it.
    // This proves recursive jobs, immutable Harness assembly, nested lowering, evidence, and the
    // final lifecycle gate as one closed path.
    let seed = constant_flow("tenant-a", "flow.seed");
    runtime.push_flow(seed.clone()).unwrap();
    assert_eq!(
        runtime
            .gate_flow(&seed, &cases(), SandboxLimits::default(), None)
            .unwrap()
            .record
            .status,
        ArtifactStatus::Canary
    );
    for child_id in &job.workspace.child_build_ids {
        for _ in 0..3 {
            runtime.advance_build("tenant-a", child_id, None).unwrap();
        }
        let child = runtime.get_build("tenant-a", child_id).unwrap().unwrap();
        assert_eq!(child.stage, BuildStage::Complete);
        assert!(matches!(
            child.result,
            Some(aelio_runtime::BuildResult::Reused { .. })
        ));
    }

    assert!(matches!(
        runtime
            .advance_build("tenant-a", &job.build_id, None)
            .unwrap(),
        BuildAction::Progress { .. }
    )); // recurse
    assert!(matches!(
        runtime
            .advance_build("tenant-a", &job.build_id, None)
            .unwrap(),
        BuildAction::Progress { .. }
    )); // assemble
    let BuildAction::Complete { result, .. } = runtime
        .advance_build("tenant-a", &job.build_id, None)
        .unwrap()
    else {
        panic!("typed Harness must execute its examples and pass the gate");
    };
    assert!(matches!(result, aelio_runtime::BuildResult::Built { .. }));
    let harness = runtime
        .artifact_repository()
        .unwrap()
        .get("tenant-a", "flow.built", 1)
        .unwrap()
        .unwrap();
    assert_eq!(
        harness.artifact.class,
        aelio_runtime::ArtifactClass::Harness
    );
    assert_eq!(harness.status, ArtifactStatus::Canary);
    let aelio_runtime::TurnReply::Completed { bag, .. } = runtime
        .submit(aelio_runtime::TurnSubmit {
            tenant: "tenant-a".into(),
            instance_id: "built-harness-live-1".into(),
            flow_id: "flow.built".into(),
            flow_rev: "1".into(),
            input: serde_json::json!({"turn":{"turn":99}}),
        })
        .await
        .unwrap()
    else {
        panic!("admitted Harness must execute on the live turn path");
    };
    assert_eq!(bag["ok"], true);
}
