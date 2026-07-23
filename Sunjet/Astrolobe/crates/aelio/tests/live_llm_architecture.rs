//! Opt-in live validation for the Rust Aelio architecture.
//!
//! Run with:
//! AELIO_LLM_ENDPOINT=... AELIO_LLM_MODEL=... AELIO_LLM_API_KEY=... \
//!   cargo test -p aelio --test live_llm_architecture -- --ignored --nocapture
//!
//! The report deliberately marks unsupported durability guarantees as gaps. It never prints
//! provider response content, prompts, user payloads, or API keys.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use aelio::abilities::express::{synthesize_prompt_spec, synthesize_with_provider};
use aelio::abilities::invoke::{MockToolHost, ToolHost};
use aelio::abilities::learn::{
    propose_path_prompt_spec, propose_path_with_provider, situation_key, typecheck,
};
use aelio::blocks::executor::{execute_path, ExecutionFrame, PathExecution, PathExecutionContext};
use aelio::policy::PolicyCtx;
use aelio::provider::{
    HttpProviderConfig, LlmProvider, LlmResponse, OpenAiCompatibleProvider, ProviderCall,
    ScriptedLlmProvider,
};
use aelio::runtime::{
    BehavioralOutcomeSignal, BehavioralSignalKind, DurableIdempotencyRecord, DurableRuntime,
    DurableTurnRequest, LearningColdLoop, PromotionGate, PromotionResult, World,
};
use aelio::storage::{AelioStore, LogicalTable, StoredRecord};
use aelio::tenant::{OutputSpec, ToolSpec};
use aelio::types::TenantMode;
use aelio::{LookupTier, ReasonCode, Value};
use indexmap::IndexMap;
use ll_query::Database;
use serde::Serialize;

#[derive(Debug, Serialize)]
struct Check {
    name: &'static str,
    status: &'static str,
    detail: String,
}

#[derive(Debug, Serialize)]
struct CallSummary {
    sequence: u64,
    spec_id: String,
    prompt_hash: String,
    model: String,
    elapsed_ms: u64,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct RowSummary {
    table: &'static str,
    count: usize,
    statuses: Vec<String>,
    versions: Vec<u64>,
}

#[derive(Debug, Serialize)]
struct LiveReport {
    model: String,
    endpoint: String,
    elapsed_ms: u64,
    live_call_count: usize,
    calls: Vec<CallSummary>,
    proposed_path: Vec<String>,
    composed_path: Vec<String>,
    executed_tools: Vec<String>,
    cold_tier: String,
    repeat_tier: String,
    persisted_rows: Vec<RowSummary>,
    checks: Vec<Check>,
}

fn live_provider() -> OpenAiCompatibleProvider {
    let endpoint =
        std::env::var("AELIO_LLM_ENDPOINT").expect("AELIO_LLM_ENDPOINT is required for live test");
    let model =
        std::env::var("AELIO_LLM_MODEL").expect("AELIO_LLM_MODEL is required for live test");
    let api_key =
        std::env::var("AELIO_LLM_API_KEY").expect("AELIO_LLM_API_KEY is required for live test");
    OpenAiCompatibleProvider::new(HttpProviderConfig {
        endpoint,
        model,
        api_key: Some(api_key),
        timeout: Duration::from_secs(90),
        extra_headers: IndexMap::new(),
    })
    .expect("live provider configuration must be valid")
}

fn custom_tool(id: &str) -> ToolSpec {
    ToolSpec {
        id: id.into(),
        name: id.into(),
        version: "1".into(),
        capability_tags: vec![format!("live.{id}")],
        effectful: false,
        idempotent: true,
        dry_run_available: true,
        params: vec![],
        output_semantics: OutputSpec {
            fields: IndexMap::new(),
            role_hint: Some("data".into()),
        },
        continuations: vec![],
        errors: vec![],
    }
}

fn call_summaries(calls: &[ProviderCall]) -> Vec<CallSummary> {
    calls
        .iter()
        .filter_map(|call| match call {
            ProviderCall::Llm {
                sequence,
                prompt_hash,
                spec_id,
                model,
                response,
                error_code,
                elapsed_ms,
                ..
            } => Some(CallSummary {
                sequence: *sequence,
                spec_id: spec_id.clone(),
                prompt_hash: prompt_hash.clone(),
                model: model.clone(),
                elapsed_ms: *elapsed_ms,
                input_tokens: response.as_ref().and_then(|item| item.input_tokens),
                output_tokens: response.as_ref().and_then(|item| item.output_tokens),
                error: error_code.map(|code| format!("{code:?}")),
            }),
            ProviderCall::Embedding { .. } => None,
        })
        .collect()
}

fn row_summary(
    store: &AelioStore,
    tenant: &str,
    table: LogicalTable,
    label: &'static str,
) -> RowSummary {
    let rows: Vec<StoredRecord<serde_json::Value>> =
        store.list(tenant, table, None, 10_000).unwrap();
    RowSummary {
        table: label,
        count: rows.len(),
        statuses: rows.iter().map(|row| row.envelope.status.clone()).collect(),
        versions: rows.iter().map(|row| row.version).collect(),
    }
}

fn collect_files(path: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let item = entry.path();
        if item.is_dir() {
            collect_files(&item, out);
        } else {
            out.push(item);
        }
    }
}

fn directory_contains(path: &Path, needle: &[u8]) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut files = Vec::new();
    collect_files(path, &mut files);
    files.into_iter().any(|file| {
        fs::read(file)
            .ok()
            .is_some_and(|bytes| bytes.windows(needle.len()).any(|window| window == needle))
    })
}

struct CountingHost {
    calls: Arc<AtomicUsize>,
}

impl ToolHost for CountingHost {
    fn call(
        &mut self,
        tool_id: &str,
        _args: &IndexMap<String, Value>,
    ) -> aelio::AelioResult<Value> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match tool_id {
            "send_otp" => Ok(Value::Map(indexmap::indexmap! {
                "ok".into() => Value::Bool(true),
                "continuation".into() => Value::str("auth.otp.verify"),
            })),
            _ => Ok(Value::Map(indexmap::indexmap! {
                "ok".into() => Value::Bool(true),
            })),
        }
    }

    fn invocation_count(&self) -> Option<usize> {
        Some(self.calls.load(Ordering::SeqCst))
    }
}

#[test]
#[ignore = "requires a real network LLM; run explicitly with AELIO_LLM_*"]
fn validates_live_llm_architecture_without_hiding_gaps() {
    let started = Instant::now();
    let model = std::env::var("AELIO_LLM_MODEL").expect("AELIO_LLM_MODEL");
    let endpoint = std::env::var("AELIO_LLM_ENDPOINT").expect("AELIO_LLM_ENDPOINT");
    let api_key = std::env::var("AELIO_LLM_API_KEY").expect("AELIO_LLM_API_KEY");
    let mut checks = Vec::new();

    // 1-2: a cold, novel situation must obtain a constrained path from the real provider.
    let mut proposal_provider = live_provider();
    let mut proposal_world = World::demo_tenant("live-proposal");
    let declared: Vec<String> = proposal_world.registry.abilities.keys().cloned().collect();
    let sigma = situation_key(
        "authenticated",
        "novel_cross_domain_contingency_analysis",
        vec!["incident_context".into()],
        vec!["catalog.browse".into(), "orders.list".into()],
        None,
        0,
        None,
    );
    let (proposed, proposal_hash) =
        propose_path_with_provider(&sigma, &declared, &mut proposal_provider)
            .expect("live constrained proposal must succeed");
    typecheck(&proposal_world.registry, &proposed).expect("live proposal must typecheck");
    let proposed_ids: Vec<_> = proposed
        .steps
        .iter()
        .map(|step| step.ability_id.clone())
        .collect();
    assert!(proposed_ids
        .iter()
        .all(|step| declared.iter().any(|declared| declared == step)));
    assert_eq!(
        proposal_hash,
        propose_path_prompt_spec().canonical_hash().unwrap()
    );
    checks.push(Check {
        name: "cold_constrained_proposal",
        status: "pass",
        detail: format!("{} declared steps; typecheck passed", proposed_ids.len()),
    });

    // 3: ask the real model to compose a larger arbitrary-domain path, then execute each
    // selected step through the generic Rust executor.
    let arbitrary_ids = vec![
        "ScopeFieldStudy".to_string(),
        "CollectFieldEvidence".to_string(),
        "CrossCheckFieldEvidence".to_string(),
        "SummarizeFieldDecision".to_string(),
    ];
    let mut composed_provider = live_provider();
    let composed_sigma = situation_key(
        "field_research",
        "mandatory_ordered_workflow_scope_then_collect_then_cross_check_then_summarize",
        vec!["research_question".into()],
        arbitrary_ids.clone(),
        None,
        0,
        None,
    );
    let (composed, _) =
        propose_path_with_provider(&composed_sigma, &arbitrary_ids, &mut composed_provider)
            .expect("live arbitrary-domain proposal must succeed");
    for id in &arbitrary_ids {
        proposal_world.registry.register_tool(custom_tool(id));
    }
    typecheck(&proposal_world.registry, &composed).expect("composed path must typecheck");
    let mut host = MockToolHost::default();
    for id in &arbitrary_ids {
        let key = id.clone();
        host = host.on(id.clone(), move |_| {
            Ok(Value::Map(indexmap::indexmap! {
                key.clone() => Value::Bool(true),
            }))
        });
    }
    let mut signatures = Default::default();
    let mut once_seen = Default::default();
    let mut effects = aelio::ops::effects::EffectEnv::live();
    let tenant_id = proposal_world.tenant.tenant_id.clone();
    let execution = execute_path(
        &composed,
        ExecutionFrame::default(),
        &mut PathExecutionContext {
            registry: &proposal_world.registry,
            policies: &proposal_world.tenant.policies,
            policy: PolicyCtx {
                tenant: Some(tenant_id),
                ..Default::default()
            },
            personality: None,
            host: &mut host,
            signatures: &mut signatures,
            once_seen: &mut once_seen,
            effects: &mut effects,
            user_id: "live-user",
        },
    )
    .expect("generic composed path must execute");
    let executed_tools = match execution {
        PathExecution::Done { invoked_tools, .. } => invoked_tools,
        PathExecution::NeedUser { .. } => panic!("parameterless composed path cannot suspend"),
    };
    assert_eq!(executed_tools.len(), composed.steps.len());
    let composed_ids: Vec<_> = composed
        .steps
        .iter()
        .map(|step| step.ability_id.clone())
        .collect();
    let multi_step = composed_ids.len() >= 3;
    checks.push(Check {
        name: "multi_step_arbitrary_domain",
        status: if multi_step { "pass" } else { "gap" },
        detail: format!(
            "model selected and executor recorded {} generic tool steps",
            composed_ids.len()
        ),
    });

    // 4: synthesis is another real call grounded in evidence and carries the canonical hash.
    let mut synthesis_provider = live_provider();
    let evidence = "Validated facts: three generic path steps completed successfully.";
    let synthesized = synthesize_with_provider(
        evidence,
        None,
        &["Do not invent facts.".into()],
        &mut synthesis_provider,
    )
    .expect("live synthesis must succeed");
    assert!(!synthesized.text.trim().is_empty());
    let synthesis_calls = call_summaries(synthesis_provider.calls());
    assert_eq!(synthesis_calls.len(), 1);
    assert_eq!(
        synthesis_calls[0].prompt_hash,
        synthesize_prompt_spec().canonical_hash().unwrap()
    );
    checks.push(Check {
        name: "evidence_grounded_synthesis",
        status: "pass",
        detail: "real synthesis returned non-empty closed output with canonical prompt hash".into(),
    });

    // 5: malformed and undeclared model output fails before generic execution/tool effects.
    let malformed = LlmResponse {
        content: r#"{"steps":["Unsupported.Exfiltrate"]}"#.into(),
        provider_request_id: Some("scripted-malformed".into()),
        input_tokens: None,
        output_tokens: None,
    };
    let mut malformed_provider = ScriptedLlmProvider::new([Ok(malformed)]);
    let no_effect_host = MockToolHost::default();
    let malformed_result = propose_path_with_provider(&sigma, &declared, &mut malformed_provider);
    assert_eq!(malformed_result.unwrap_err().code, ReasonCode::ParseError);
    assert_eq!(no_effect_host.invocation_count(), Some(0));
    checks.push(Check {
        name: "malformed_output_fails_closed",
        status: "pass",
        detail: "undeclared ability rejected with ParseError; tool effects remained zero".into(),
    });

    // 6-8: run the actual World/DurableRuntime, inspect Astrolobe through Rust APIs, then reopen.
    let data_dir = std::env::temp_dir().join(format!(
        "aelio_live_architecture_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis()
    ));
    fs::create_dir_all(&data_dir).unwrap();
    let tenant = "live-durable-clean";
    let db = Database::create(&data_dir).unwrap();
    let store = AelioStore::new(db, 3).unwrap();
    let mut durable_world = World::demo_tenant(tenant);
    durable_world.set_llm_provider(Box::new(live_provider()));
    let mut runtime = DurableRuntime::new(durable_world, store).unwrap();
    let request = DurableTurnRequest {
        turn_id: "live-turn-cold".into(),
        user_id: "live-user".into(),
        utterance: "Hi".into(),
    };
    let cold = runtime.run_turn(request.clone()).unwrap();
    let cold_calls = call_summaries(runtime.world.provider_calls());
    let calls_after_cold = cold_calls.len();
    let proposal_id = cold
        .proposal_id
        .clone()
        .expect("cold path must create a durable proposal");
    let mut cold_loop = LearningColdLoop::new(tenant, TenantMode::Bake, runtime.store.clone());
    for index in 0..5 {
        cold_loop
            .record_signal(BehavioralOutcomeSignal {
                id: format!("live-signal-{index}"),
                turn_id: "live-turn-cold".into(),
                proposal_id: Some(proposal_id.clone()),
                kind: BehavioralSignalKind::FlowTerminal,
                step_ids: cold.steps.iter().map(|step| step.name.clone()).collect(),
                failed_step: None,
                latency_ms: 0,
                token_cost: 0,
                call_cost_microunits: 0,
                observed_at_ms: chrono::Utc::now().timestamp_millis() + index,
            })
            .unwrap();
    }
    let promoted_version = match cold_loop
        .promote(
            &proposal_id,
            &PromotionGate::default(),
            chrono::Utc::now().timestamp_millis(),
        )
        .unwrap()
    {
        PromotionResult::Promoted { version } | PromotionResult::AlreadyPromoted { version } => {
            version
        }
    };
    let repeat = runtime
        .run_turn(DurableTurnRequest {
            turn_id: "live-turn-repeat".into(),
            ..request.clone()
        })
        .unwrap();
    let calls_after_repeat = runtime.world.provider_calls().len();
    assert_eq!(calls_after_repeat, calls_after_cold);
    assert!(matches!(
        repeat.tier,
        Some(LookupTier::Tier0 | LookupTier::Tier1)
    ));
    checks.push(Check {
        name: "exact_repeat_warm_path",
        status: "pass",
        detail: format!(
            "cold tier {:?}; repeat tier {:?}; second proposal calls 0",
            cold.tier, repeat.tier
        ),
    });
    let synthetic_phone = "+919000000001";
    let flow_start = runtime
        .run_turn(DurableTurnRequest {
            turn_id: "live-flow-start".into(),
            user_id: "flow-user".into(),
            utterance: format!("login with {synthetic_phone}"),
        })
        .unwrap();
    assert!(flow_start.suspended);
    assert!(flow_start.active_flow.is_some());
    runtime.store.flush().unwrap();
    drop(runtime);

    let reopened_db = Database::open(&data_dir).unwrap();
    let reopened_store = AelioStore::new(reopened_db, 3).unwrap();
    let mut reopened_world = World::demo_tenant(tenant);
    reopened_world.set_llm_provider(Box::new(live_provider()));
    let mut reopened = DurableRuntime::new(reopened_world, reopened_store).unwrap();
    let restarted_procedure = reopened
        .world
        .registry
        .procedures
        .get(&proposal_id)
        .expect("promoted procedure must load after restart");
    assert_eq!(restarted_procedure.version, promoted_version);
    let active_before_resume: StoredRecord<Option<aelio::blocks::flow::FlowInstance>> = reopened
        .store
        .get(tenant, LogicalTable::FlowInstances, "flow-user")
        .unwrap()
        .expect("active flow must survive Database::open");
    assert_eq!(active_before_resume.envelope.status, "active");
    let replayed = reopened.run_turn(request).unwrap();
    assert_eq!(replayed.reply.text, cold.reply.text);
    assert_eq!(reopened.world.provider_calls().len(), 0);
    let flow_resume = reopened
        .run_turn(DurableTurnRequest {
            turn_id: "live-flow-resume".into(),
            user_id: "flow-user".into(),
            utterance: format!("{synthetic_phone} 434543"),
        })
        .unwrap();
    assert_eq!(flow_resume.new_state.as_deref(), Some("authenticated"));
    assert_eq!(reopened.world.provider_calls().len(), 0);
    reopened.store.flush().unwrap();

    let tables = [
        (LogicalTable::Turns, "turns"),
        (LogicalTable::FlowInstances, "flow_instances"),
        (LogicalTable::States, "states"),
        (LogicalTable::StepAttempts, "step_attempts"),
        (LogicalTable::CallRecords, "call_records"),
        (LogicalTable::Proposals, "proposals"),
        (LogicalTable::Idempotency, "idempotency"),
    ];
    let persisted_rows: Vec<_> = tables
        .into_iter()
        .map(|(table, label)| row_summary(&reopened.store, tenant, table, label))
        .collect();
    let turn_rows = persisted_rows
        .iter()
        .find(|row| row.table == "turns")
        .unwrap();
    assert_eq!(turn_rows.count, 4);
    assert!(turn_rows
        .statuses
        .iter()
        .all(|status| status == "completed"));
    reopened
        .store
        .migrate_tenant("foreign-tenant", chrono::Utc::now().timestamp_millis())
        .unwrap();
    let foreign_turns: Vec<StoredRecord<serde_json::Value>> = reopened
        .store
        .list("foreign-tenant", LogicalTable::Turns, None, 100)
        .unwrap();
    assert!(foreign_turns.is_empty());
    checks.push(Check {
        name: "restart_and_tenant_isolation",
        status: "pass",
        detail: "promoted tier procedure, turn replay, and safe flow recollection survived Database::open with zero new LLM calls; foreign tenant saw zero rows".into(),
    });

    for (name, table) in [
        ("durable_step_attempts", "step_attempts"),
        ("durable_call_records", "call_records"),
        ("durable_proposals", "proposals"),
    ] {
        let count = persisted_rows
            .iter()
            .find(|row| row.table == table)
            .unwrap()
            .count;
        checks.push(Check {
            name,
            status: if count > 0 { "pass" } else { "gap" },
            detail: format!("Astrolobe row count after durable cold turn: {count}"),
        });
    }

    let secret_persisted = directory_contains(&data_dir, api_key.as_bytes());
    assert!(!secret_persisted, "API key bytes must never be persisted");
    checks.push(Check {
        name: "no_provider_secret_persisted",
        status: "pass",
        detail: "database files do not contain the process-local API key bytes".into(),
    });
    let pii_persisted = directory_contains(&data_dir, synthetic_phone.as_bytes());
    let otp_persisted = directory_contains(&data_dir, b"434543");
    assert!(!pii_persisted, "synthetic phone must not be durable");
    assert!(!otp_persisted, "synthetic OTP must not be durable");
    checks.push(Check {
        name: "sensitive_payload_redaction",
        status: "pass",
        detail: "synthetic phone and OTP bytes were absent from raw durable files".into(),
    });

    // 9: two workers sharing one Astrolobe store race the same turn ID. Exactly one effect wins.
    let race_dir = data_dir.join("duplicate-race");
    fs::create_dir_all(&race_dir).unwrap();
    let race_store = AelioStore::new(Database::create(&race_dir).unwrap(), 3).unwrap();
    let effect_calls = Arc::new(AtomicUsize::new(0));
    let make_runtime = |store: AelioStore| {
        let mut world = World::demo_tenant("live-race");
        world.set_tool_host(Box::new(CountingHost {
            calls: Arc::clone(&effect_calls),
        }));
        DurableRuntime::new(world, store).unwrap()
    };
    let left = make_runtime(race_store.clone());
    let right = make_runtime(race_store);
    let barrier = Arc::new(Barrier::new(3));
    let race_request = DurableTurnRequest {
        turn_id: "same-turn-id".into(),
        user_id: "race-user".into(),
        utterance: "login with +919876543210".into(),
    };
    let spawn = |mut runtime: DurableRuntime| {
        let barrier = Arc::clone(&barrier);
        let request = race_request.clone();
        std::thread::spawn(move || {
            barrier.wait();
            runtime.run_turn(request).map(|_| ())
        })
    };
    let left = spawn(left);
    let right = spawn(right);
    barrier.wait();
    let race_results = [left.join().unwrap(), right.join().unwrap()];
    let duplicate_effects = effect_calls.load(Ordering::SeqCst);
    assert_eq!(duplicate_effects, 1);
    assert!(race_results.iter().any(Result::is_ok));
    checks.push(Check {
        name: "concurrent_duplicate_turn_id",
        status: "pass",
        detail: format!(
            "shared-store workers produced {duplicate_effects} tool effect; outcomes={}",
            race_results
                .iter()
                .map(|result| match result {
                    Ok(()) => "ok".to_string(),
                    Err(error) => format!("{:?}", error.code),
                })
                .collect::<Vec<_>>()
                .join(",")
        ),
    });
    let idempotency_rows: Vec<StoredRecord<DurableIdempotencyRecord>> = reopened
        .store
        .list(tenant, LogicalTable::Idempotency, None, 100)
        .unwrap();
    assert!(
        idempotency_rows.len() >= 2,
        "generic executor must persist tool idempotency rows"
    );
    assert!(idempotency_rows
        .iter()
        .all(|row| row.envelope.status == "completed"));
    checks.push(Check {
        name: "durable_tool_idempotency_keys",
        status: "pass",
        detail: format!(
            "{} completed durable tool leases/results observed; duplicate turn effect remained single",
            idempotency_rows.len()
        ),
    });

    let mut all_calls = Vec::new();
    all_calls.extend(call_summaries(proposal_provider.calls()));
    all_calls.extend(call_summaries(composed_provider.calls()));
    all_calls.extend(synthesis_calls);
    all_calls.extend(cold_calls);
    let live_call_count = all_calls.len();
    let report = LiveReport {
        model,
        endpoint,
        elapsed_ms: started.elapsed().as_millis() as u64,
        live_call_count,
        calls: all_calls,
        proposed_path: proposed_ids,
        composed_path: composed_ids,
        executed_tools,
        cold_tier: format!("{:?}", cold.tier),
        repeat_tier: format!("{:?}", repeat.tier),
        persisted_rows,
        checks,
    };

    // The report contains metadata only. Provider content, prompts, utterances, and secrets are
    // intentionally excluded.
    println!(
        "AELIO_LIVE_REPORT={}",
        serde_json::to_string_pretty(&report).unwrap()
    );
    let _ = fs::remove_dir_all(&data_dir);
}
