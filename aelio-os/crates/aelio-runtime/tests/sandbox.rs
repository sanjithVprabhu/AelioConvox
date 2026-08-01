use aelio_prompt::{MintRequest, MintSlot, MockMintDrafter, PromptArtifact};
use aelio_runtime::{
    baseline_conversation_flow, mint_prompt, ArtifactStatus, BoundSpec, CanaryObservation,
    EffectSpec, FlowPush, OriginSpec, PromptCaseKind, PromptEvaluator, PromptGateCase, Runtime,
    RuntimeConfig, RuntimeError, SandboxCase, SandboxFixtureCall, SandboxLimits, SandboxRunner,
    TargetClassSpec, TargetSpec, DEFAULT_QUEUE_DEPTH,
};
use aelio_sol::{value_hash, SolValue};
use serde_json::Value as Json;
use std::collections::BTreeMap;

fn fixture_flow() -> FlowPush {
    let input = SolValue::map([("value", SolValue::Int(7))]);
    let output = SolValue::map([("answer", SolValue::Int(14))]);
    FlowPush {
        tenant: "real-tenant".into(),
        flow_id: "sandbox.fixture".into(),
        flow_rev: "1".into(),
        program: serde_json::json!({
            "nid":"call",
            "op":"Call",
            "id":"tool.double@1",
            "args":{"value":{"pull":"value"}},
            "into":"result"
        }),
        targets: vec![TargetSpec {
            id: "tool.double@1".into(),
            class: TargetClassSpec::Tool,
            effect: EffectSpec::External,
            input_imprint: aelio_sol::structural_imprint(&input),
            output_imprint: aelio_sol::structural_imprint(&output),
            bounded: BoundSpec::Deadline { max_ms: 1_000 },
            policy_tags: vec!["sandbox.fixture".into()],
            origin: OriginSpec::Tenant,
        }],
        prompts: vec![],
    }
}

fn runtime(path: &std::path::Path) -> Runtime {
    Runtime::open(RuntimeConfig {
        data_dir: path.into(),
        host_url: None,
        host_token: None,
        event_key_secret: [9; 32],
        queue_depth: DEFAULT_QUEUE_DEPTH,
    })
    .unwrap()
}

fn constant_flow(id: &str) -> FlowPush {
    FlowPush {
        tenant: "tenant-a".into(),
        flow_id: id.into(),
        flow_rev: "1".into(),
        program: serde_json::json!({"nid":"root","op":"Const","v":{"ok":true}}),
        targets: vec![],
        prompts: vec![],
    }
}

fn passing_cases(count: i64) -> Vec<SandboxCase> {
    (0..count)
        .map(|index| SandboxCase {
            input: serde_json::json!({"case": index}),
            wakes: vec![],
            expect_park: false,
            expected: serde_json::json!({"ok": true}),
            fixtures: vec![],
        })
        .collect()
}

struct DeterministicPromptEvaluator;

impl PromptEvaluator for DeterministicPromptEvaluator {
    fn evaluate(
        &self,
        _artifact: &PromptArtifact,
        rendered: &str,
        _slots: &BTreeMap<String, Json>,
    ) -> Result<Json, RuntimeError> {
        assert!(rendered.contains("incoming-"));
        Ok(serde_json::json!({
            "class":"example",
            "confidence":0.0,
            "undeterminable":rendered.contains("incoming-0")
        }))
    }
}

#[test]
fn minted_prompt_needs_real_exemplar_admission_before_flow_binding() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime(dir.path());
    let shelf = runtime.mint_shelf().unwrap();
    let request = MintRequest {
        tenant: "tenant-a".into(),
        objective: "classify an incoming customer message".into(),
        system_input: vec![MintSlot {
            name: "message_text".into(),
            ty: "str".into(),
            required: true,
            sensitivity: "public".into(),
        }],
        output: BTreeMap::from([
            ("class".into(), "str".into()),
            ("confidence".into(), "float".into()),
        ]),
        input: None,
        id: Some("prompt.classify_customer".into()),
        version: "1".into(),
    };
    let minted = mint_prompt(Some(&shelf), &request, &MockMintDrafter).unwrap();
    assert_eq!(
        shelf
            .artifact_status("tenant-a", "prompt.classify_customer", 1)
            .unwrap(),
        Some(ArtifactStatus::Proposed)
    );
    let cases: Vec<_> = (0..20)
        .map(|index| PromptGateCase {
            slots: BTreeMap::from([(
                "message_text".into(),
                serde_json::json!(format!("incoming-{index}")),
            )]),
            expected: BTreeMap::from([
                ("class".into(), serde_json::json!("example")),
                ("confidence".into(), serde_json::json!(0.0)),
                ("undeterminable".into(), serde_json::json!(index == 0)),
            ]),
            kind: if index == 0 {
                PromptCaseKind::Negative
            } else {
                PromptCaseKind::Positive
            },
        })
        .collect();
    let gated = runtime
        .gate_prompt(
            "tenant-a",
            &minted.artifact,
            &cases,
            &DeterministicPromptEvaluator,
            Some("deployer:test".into()),
        )
        .unwrap();
    assert_eq!(gated.record.status, ArtifactStatus::Canary);
    assert_eq!(gated.evidence.distinct_inputs, 20);
    assert_eq!(gated.evidence.validation_rate, 1.0);
    assert_eq!(gated.report.cases.len(), 20);
    assert!(!shelf
        .recall(
            "tenant-a",
            "classify customer message",
            aelio_query::RecallModality::Fulltext,
            5,
        )
        .unwrap()
        .is_empty());
}

#[test]
fn flow_cannot_embed_or_mutate_a_prompt_outside_artifact_authority() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime(dir.path());
    let mut flow = baseline_conversation_flow("tenant-a");
    flow.prompts[0].body.push_str(" injected override");
    let error = runtime.push_flow(flow).unwrap_err();
    assert!(matches!(error, RuntimeError::Conflict(_)), "{error}");
}

#[test]
fn sandbox_runs_production_executor_with_exact_ordered_fixtures() {
    let args = SolValue::map([("value", SolValue::Int(7))]);
    let report = SandboxRunner::run_flow(
        &fixture_flow(),
        &[SandboxCase {
            input: serde_json::json!({"value": 7}),
            wakes: vec![],
            expect_park: false,
            expected: serde_json::json!({"result": {"answer": 14}}),
            fixtures: vec![SandboxFixtureCall {
                target: "tool.double@1".into(),
                expected_args_hash: Some(value_hash(&args)),
                output: serde_json::json!({"answer": 14}),
                usage_tokens: 0,
            }],
        }],
        SandboxLimits::default(),
    )
    .unwrap();
    assert_eq!(report.cases.len(), 1);
    assert!(report.total_reactions > 0);
    assert_eq!(report.namespace_hash.len(), 64);
}

#[test]
fn sandbox_cannot_fall_through_to_a_real_host_or_tenant_target() {
    let error = SandboxRunner::run_flow(
        &fixture_flow(),
        &[SandboxCase {
            input: serde_json::json!({"value": 7}),
            wakes: vec![],
            expect_park: false,
            expected: serde_json::json!({}),
            fixtures: vec![],
        }],
        SandboxLimits::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::Kernel { code, .. } if code == "Tool.Permanent"
    ));
}

#[test]
fn sandbox_rejects_argument_drift_unused_fixtures_and_budget_overrun() {
    let bad_hash = SandboxRunner::run_flow(
        &fixture_flow(),
        &[SandboxCase {
            input: serde_json::json!({"value": 7}),
            wakes: vec![],
            expect_park: false,
            expected: serde_json::json!({}),
            fixtures: vec![SandboxFixtureCall {
                target: "tool.double@1".into(),
                expected_args_hash: Some("0".repeat(64)),
                output: serde_json::json!({"answer": 14}),
                usage_tokens: 0,
            }],
        }],
        SandboxLimits::default(),
    )
    .unwrap_err();
    assert!(matches!(
        bad_hash,
        RuntimeError::Kernel { code, .. } if code == "Guard.Violation"
    ));

    let mut fixtures = vec![SandboxFixtureCall {
        target: "tool.double@1".into(),
        expected_args_hash: None,
        output: serde_json::json!({"answer": 14}),
        usage_tokens: 0,
    }];
    fixtures.push(fixtures[0].clone());
    assert!(matches!(
        SandboxRunner::run_flow(
            &fixture_flow(),
            &[SandboxCase {
                input: serde_json::json!({"value": 7}),
                wakes: vec![],
                expect_park: false,
                expected: serde_json::json!({}),
                fixtures,
            }],
            SandboxLimits::default(),
        ),
        Err(RuntimeError::Invalid(detail)) if detail.contains("did not consume")
    ));

    assert!(matches!(
        SandboxRunner::run_flow(
            &fixture_flow(),
            &[SandboxCase {
                input: serde_json::json!({"value": 7}),
                wakes: vec![],
                expect_park: false,
                expected: serde_json::json!({}),
                fixtures: vec![SandboxFixtureCall {
                    target: "tool.double@1".into(),
                    expected_args_hash: None,
                    output: serde_json::json!({"answer": 14}),
                    usage_tokens: 0,
                }],
            }],
            SandboxLimits {
                max_reactions: 1,
                max_wall_ms: 10_000,
            },
        ),
        Err(RuntimeError::Kernel { code, .. }) if code == "Budget.Calls"
    ));
}

#[test]
fn gate_counts_distinct_inputs_and_never_canaries_below_mother_threshold() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = runtime(directory.path());
    let flow = constant_flow("gate.constant");
    runtime.push_flow(flow.clone()).unwrap();
    let cases = passing_cases(19);
    let result = runtime
        .gate_flow(&flow, &cases, SandboxLimits::default(), None)
        .unwrap();
    assert_eq!(result.evidence.distinct_inputs, 19);
    assert_eq!(result.record.status, ArtifactStatus::Shadow);

    let twentieth = [SandboxCase {
        input: serde_json::json!({"case": 19}),
        wakes: vec![],
        expect_park: false,
        expected: serde_json::json!({"ok": true}),
        fixtures: vec![],
    }];
    let result = runtime
        .gate_flow(&flow, &twentieth, SandboxLimits::default(), None)
        .unwrap();
    assert_eq!(result.evidence.distinct_inputs, 20);
    assert_eq!(result.record.status, ArtifactStatus::Canary);
}

#[test]
fn canary_requires_a_gate_applied_proposal_and_demotes_on_guard_violation() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = runtime(directory.path());
    let promoted_flow = constant_flow("gate.promote");
    runtime.push_flow(promoted_flow.clone()).unwrap();
    let gated = runtime
        .gate_flow(
            &promoted_flow,
            &passing_cases(20),
            SandboxLimits::default(),
            None,
        )
        .unwrap();
    assert_eq!(gated.record.status, ArtifactStatus::Canary);
    for index in 0..20 {
        let result = runtime
            .observe_canary(
                "tenant-a",
                "gate.promote",
                1,
                CanaryObservation {
                    input_hash: value_hash(&SolValue::Int(i64::from(index))),
                    downstream_success: true,
                    guard_violation: false,
                    ledger_hash: value_hash(&SolValue::str(format!("ledger-{index}"))),
                },
            )
            .unwrap();
        assert_eq!(result.record.status, ArtifactStatus::Canary);
        if index == 19 {
            let proposal = result
                .promotion_proposal
                .expect("threshold must create a proposal");
            let applied = runtime
                .apply_promotion_proposal("tenant-a", &proposal.proposal_id)
                .unwrap();
            assert_eq!(applied.record.status, ArtifactStatus::Promoted);
            assert_eq!(
                applied.promotion_proposal.unwrap().status,
                aelio_runtime::PromotionProposalStatus::Applied
            );
        } else {
            assert!(result.promotion_proposal.is_none());
        }
    }

    let demoted_flow = constant_flow("gate.demote");
    runtime.push_flow(demoted_flow.clone()).unwrap();
    runtime
        .gate_flow(
            &demoted_flow,
            &passing_cases(20),
            SandboxLimits::default(),
            None,
        )
        .unwrap();
    let demoted = runtime
        .observe_canary(
            "tenant-a",
            "gate.demote",
            1,
            CanaryObservation {
                input_hash: value_hash(&SolValue::str("unsafe-input")),
                downstream_success: true,
                guard_violation: true,
                ledger_hash: value_hash(&SolValue::str("guard-ledger")),
            },
        )
        .unwrap();
    assert_eq!(demoted.record.status, ArtifactStatus::Shadow);
    assert_eq!(demoted.evidence.guard_violations, 1);
}

#[test]
fn reviewed_artifact_requires_identified_deployer_before_canary() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = runtime(directory.path());
    let flow = fixture_flow();
    runtime.push_flow(flow.clone()).unwrap();
    let cases: Vec<_> = (0..20)
        .map(|value| {
            let value = i64::from(value);
            let args = SolValue::map([("value", SolValue::Int(value))]);
            SandboxCase {
                input: serde_json::json!({"value": value}),
                wakes: vec![],
                expect_park: false,
                expected: serde_json::json!({"result": {"answer": value * 2}}),
                fixtures: vec![SandboxFixtureCall {
                    target: "tool.double@1".into(),
                    expected_args_hash: Some(value_hash(&args)),
                    output: serde_json::json!({"answer": value * 2}),
                    usage_tokens: 0,
                }],
            }
        })
        .collect();
    let waiting = runtime
        .gate_flow(&flow, &cases, SandboxLimits::default(), None)
        .unwrap();
    assert_eq!(waiting.evidence.distinct_inputs, 20);
    assert_eq!(waiting.record.status, ArtifactStatus::Shadow);

    let approved = runtime
        .gate_flow(
            &flow,
            &cases,
            SandboxLimits::default(),
            Some("deployer-42".into()),
        )
        .unwrap();
    assert_eq!(approved.record.status, ArtifactStatus::Canary);
    assert!(matches!(
        &approved.record.history.last().unwrap().actor,
        aelio_runtime::ArtifactActor::Deployer(id) if id == "deployer-42"
    ));
}
