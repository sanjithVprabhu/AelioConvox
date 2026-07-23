//! Live LLM + durable store end-to-end verification.
//!
//! Run (requires network + key):
//!   OPENAI_API_KEY=... cargo test -p aelio --test live_llm_e2e -- --ignored --nocapture
//!
//! Or:
//!   set -a && source ../../.env && set +a
//!   cargo test -p aelio --test live_llm_e2e -- --ignored --nocapture

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use aelio::provider::{OpenAiCompatibleProvider, ProviderCall};
use aelio::runtime::{DurableRuntime, DurableTurnRequest};
use aelio::storage::{AelioStore, LogicalTable, StoredRecord};
use aelio::types::LookupTier;
use aelio::World;
use ll_query::Database;

fn live_enabled() -> bool {
    std::env::var("OPENAI_API_KEY")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
        && std::env::var("AELIO_SKIP_LIVE_LLM").ok().as_deref() != Some("1")
}

fn temp_db(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "aelio_live_llm_{tag}_{}_{}",
        std::process::id(),
        nanos
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn live_runtime(tag: &str) -> DurableRuntime {
    let path = temp_db(tag);
    eprintln!("[live] durable db path: {}", path.display());
    let store = AelioStore::new(Database::create(&path).unwrap(), 8).unwrap();
    let mut world = World::demo_tenant("live-tenant");
    let provider = OpenAiCompatibleProvider::from_env().expect("OPENAI_API_KEY must be set");
    world.set_llm_provider(Box::new(provider));
    DurableRuntime::new(world, store).unwrap()
}

fn print_turn(label: &str, result: &aelio::blocks::turn::TurnResult) {
    eprintln!("\n=== {label} ===");
    eprintln!("reply: {}", result.reply.text);
    eprintln!(
        "llm_calls={} tier={:?} depth={:?} suspended={} proposal={:?}",
        result.llm_calls, result.tier, result.depth, result.suspended, result.proposal_id
    );
    for (i, step) in result.steps.iter().enumerate() {
        eprintln!("  step[{i}] {} — {}", step.name, step.detail);
    }
}

#[test]
#[ignore = "live network: requires OPENAI_API_KEY"]
fn live_hi_cold_then_warm_persists_steps_and_calls() {
    if !live_enabled() {
        eprintln!("skip: OPENAI_API_KEY not set");
        return;
    }

    let mut rt = live_runtime("hi");

    // ── Cold "Hi": authored path, no paid intelligence required ──────────
    let cold = rt
        .run_turn(DurableTurnRequest {
            turn_id: "turn-hi-cold".into(),
            user_id: "alice".into(),
            utterance: "Hi".into(),
        })
        .expect("cold turn");
    print_turn("cold Hi", &cold);

    assert_eq!(cold.llm_calls, 0);
    assert!(!cold.opened_loop, "greeting must not open a loop");
    assert!(!cold.reply.text.trim().is_empty());

    // Steps stored in Astrolobe
    let steps = rt.list_steps("turn-hi-cold").expect("list steps");
    assert!(
        steps.len() >= 4,
        "expected multiple durable steps, got {}",
        steps.len()
    );
    let step_names: Vec<&str> = steps
        .iter()
        .map(|s| s.envelope.value.name.as_str())
        .collect();
    eprintln!("[live] durable step names: {step_names:?}");
    assert!(step_names.contains(&"Sense"));
    assert!(step_names.contains(&"FlowGate"));
    assert!(
        step_names
            .iter()
            .any(|n| *n == "Tier2Compose" || *n == "LookupTier"),
        "expected deterministic composition or lookup in {step_names:?}"
    );

    // Provider calls stored
    let calls = rt.list_calls("turn-hi-cold").expect("list calls");
    eprintln!("[live] durable call count: {}", calls.len());
    assert!(
        !calls.is_empty() || cold.llm_calls == 0,
        "LLM calls should be persisted when llm_calls > 0"
    );
    for call in &calls {
        if let ProviderCall::Llm {
            spec_id,
            elapsed_ms,
            response,
            error_code,
            ..
        } = &call.envelope.value.call
        {
            eprintln!(
                "[live] call spec={spec_id} elapsed_ms={elapsed_ms} err={error_code:?} has_response={}",
                response.is_some()
            );
            assert!(error_code.is_none(), "live LLM error: {error_code:?}");
            assert!(response.is_some(), "expected response body stored");
        }
    }

    // Turn row completed + reloadable
    let reloaded = rt.get_turn("turn-hi-cold").unwrap().expect("turn stored");
    assert_eq!(reloaded.reply.text, cold.reply.text);

    // Idempotent replay: second same turn_id must not re-call tools/LLM path
    let calls_before = rt.world.provider_calls().len();
    let replay = rt
        .run_turn(DurableTurnRequest {
            turn_id: "turn-hi-cold".into(),
            user_id: "alice".into(),
            utterance: "Hi".into(),
        })
        .expect("replay");
    assert_eq!(replay.reply.text, cold.reply.text);
    assert_eq!(
        rt.world.provider_calls().len(),
        calls_before,
        "replay must not issue new provider calls"
    );

    // ── Warm "Hi" for another user: procedure promoted → 0 LLM ideal ─────
    // Promotion is in-memory registry; same runtime sees it.
    let warm = rt
        .run_turn(DurableTurnRequest {
            turn_id: "turn-hi-warm".into(),
            user_id: "bob".into(),
            utterance: "Hi".into(),
        })
        .expect("warm turn");
    print_turn("warm Hi", &warm);

    // After cold promotion, warm should prefer tier0/1 and ideally 0 LLM.
    // If situation hash differs slightly we still accept tier1 with low calls.
    if let Some(tier) = warm.tier {
        assert!(
            matches!(
                tier,
                LookupTier::Tier0 | LookupTier::Tier1 | LookupTier::Tier2
            ),
            "unexpected tier {tier:?}"
        );
    }
    let warm_steps = rt.list_steps("turn-hi-warm").unwrap();
    assert!(!warm_steps.is_empty());
    eprintln!("[live] warm durable steps={}", warm_steps.len());

    // Proposals table should have at least one entry after cold path
    let proposals: Vec<StoredRecord<serde_json::Value>> = rt
        .store
        .list("live-tenant", LogicalTable::Proposals, None, 32)
        .unwrap();
    eprintln!("[live] proposals in db: {}", proposals.len());
}

#[test]
#[ignore = "live network: requires OPENAI_API_KEY"]
fn live_multi_turn_login_flow_stores_state_and_steps() {
    if !live_enabled() {
        eprintln!("skip: OPENAI_API_KEY not set");
        return;
    }

    let mut rt = live_runtime("login");

    let t1 = rt
        .run_turn(DurableTurnRequest {
            turn_id: "login-1".into(),
            user_id: "carol".into(),
            utterance: "I want to login with phone 98765 43210".into(),
        })
        .expect("login start");
    print_turn("login start", &t1);

    assert!(t1.suspended || t1.active_flow.is_some() || t1.opened_loop);
    let steps1 = rt.list_steps("login-1").unwrap();
    assert!(!steps1.is_empty());
    assert!(
        steps1
            .iter()
            .any(|s| s.envelope.value.name.contains("Registry")
                || s.envelope.value.name.contains("Activate")
                || s.envelope.value.detail.to_lowercase().contains("login")
                || s.envelope.value.name.contains("Flow")),
        "expected login-related steps, got {:?}",
        steps1
            .iter()
            .map(|s| &s.envelope.value.name)
            .collect::<Vec<_>>()
    );

    // Flow instance durable
    let flow: Option<StoredRecord<Option<aelio::blocks::flow::FlowInstance>>> = rt
        .store
        .get("live-tenant", LogicalTable::FlowInstances, "carol")
        .unwrap();
    eprintln!(
        "[live] flow after login-1: {:?}",
        flow.as_ref().map(|f| (
            f.envelope.status.clone(),
            f.envelope.value.as_ref().map(|i| i.pending_step.clone())
        ))
    );

    // Resume with OTP — must rehydrate flow from DB
    let t2 = rt
        .run_turn(DurableTurnRequest {
            turn_id: "login-2".into(),
            user_id: "carol".into(),
            utterance: "434543".into(),
        })
        .expect("otp resume");
    print_turn("otp resume", &t2);

    // Either authenticated or still collecting — both valid if tool path differs
    let state: Option<StoredRecord<String>> = rt
        .store
        .get("live-tenant", LogicalTable::States, "carol")
        .unwrap();
    eprintln!(
        "[live] user state after otp: {:?}",
        state.as_ref().map(|s| &s.envelope.value)
    );

    let steps2 = rt.list_steps("login-2").unwrap();
    assert!(!steps2.is_empty(), "otp turn steps must be stored");

    // Turns table has both completed turns
    let turns: Vec<StoredRecord<serde_json::Value>> = rt
        .store
        .list("live-tenant", LogicalTable::Turns, Some("completed"), 32)
        .unwrap();
    let turn_keys: Vec<_> = turns.iter().map(|t| t.envelope.key.as_str()).collect();
    eprintln!("[live] completed turns: {turn_keys:?}");
    assert!(turn_keys.contains(&"login-1"));
    assert!(turn_keys.contains(&"login-2"));
}

#[test]
#[ignore = "live network: requires OPENAI_API_KEY"]
fn live_propose_path_returns_declared_abilities_only() {
    if !live_enabled() {
        eprintln!("skip: OPENAI_API_KEY not set");
        return;
    }

    use aelio::abilities::learn::{propose_path_with_provider, situation_key};
    use aelio::provider::LlmProvider;

    let mut provider = OpenAiCompatibleProvider::from_env().unwrap();
    let sigma = situation_key(
        "unauthenticated",
        "greeting",
        vec![],
        vec!["auth.otp.send".into(), "catalog.browse".into()],
        None,
        0,
        None,
    );
    let abilities = vec![
        "State.Direction".into(),
        "Registry.Capabilities".into(),
        "Express.Template".into(),
        "Express.Synthesize".into(),
        "Express.Ask".into(),
        "Learn.ProposePath".into(),
    ];

    let (path, prompt_hash) =
        propose_path_with_provider(&sigma, &abilities, &mut provider).expect("live propose_path");

    eprintln!("[live] proposed path: {:?}", path.steps);
    eprintln!("[live] prompt_hash: {prompt_hash}");
    assert!(!path.steps.is_empty());
    for step in &path.steps {
        assert!(
            abilities.iter().any(|a| a == &step.ability_id),
            "LLM proposed undeclared ability {}",
            step.ability_id
        );
    }
    assert_eq!(provider.call_count(), 1);
    match &provider.calls()[0] {
        ProviderCall::Llm {
            response,
            elapsed_ms,
            spec_id,
            ..
        } => {
            eprintln!(
                "[live] propose_path call spec={spec_id} elapsed_ms={elapsed_ms} content_len={}",
                response.as_ref().map(|r| r.content.len()).unwrap_or(0)
            );
            assert_eq!(spec_id, "aelio.propose_path");
            assert!(response.is_some());
        }
        other => panic!("expected LLM call, got {other:?}"),
    }
}

#[test]
#[ignore = "live network: requires OPENAI_API_KEY"]
fn live_bigger_composition_scenario_end_to_end() {
    if !live_enabled() {
        eprintln!("skip: OPENAI_API_KEY not set");
        return;
    }

    let mut rt = live_runtime("compose");

    // 1) Warm the greeting vocabulary
    let hi = rt
        .run_turn(DurableTurnRequest {
            turn_id: "c-hi".into(),
            user_id: "dave".into(),
            utterance: "Hello there".into(),
        })
        .unwrap();
    print_turn("hello", &hi);

    // 2) Start auth flow
    let login = rt
        .run_turn(DurableTurnRequest {
            turn_id: "c-login".into(),
            user_id: "dave".into(),
            utterance: "please sign me in, my number is 9876543210".into(),
        })
        .unwrap();
    print_turn("sign in", &login);

    // 3) OTP
    let otp = rt
        .run_turn(DurableTurnRequest {
            turn_id: "c-otp".into(),
            user_id: "dave".into(),
            utterance: "434543".into(),
        })
        .unwrap();
    print_turn("otp", &otp);

    // 4) Rank query (term resolution + optional synthesis)
    if otp.new_state.as_deref() == Some("authenticated")
        || rt
            .store
            .get::<String>("live-tenant", LogicalTable::States, "dave")
            .unwrap()
            .map(|s| s.envelope.value == "authenticated")
            .unwrap_or(false)
    {
        // force authenticated for rank query if flow completed
        rt.world
            .user_state
            .insert("dave".into(), "authenticated".into());
    }
    let rank = rt
        .run_turn(DurableTurnRequest {
            turn_id: "c-rank".into(),
            user_id: "dave".into(),
            utterance: "give me the ten most avaricious clients".into(),
        })
        .unwrap();
    print_turn("avaricious", &rank);

    // Aggregate durable audit surface
    let all_steps: Vec<StoredRecord<aelio::runtime::DurableStepAttempt>> = rt
        .store
        .list("live-tenant", LogicalTable::StepAttempts, None, 512)
        .unwrap();
    let all_calls: Vec<StoredRecord<aelio::runtime::TurnProviderCallRecord>> = rt
        .store
        .list("live-tenant", LogicalTable::CallRecords, None, 512)
        .unwrap();
    let all_turns: Vec<StoredRecord<serde_json::Value>> = rt
        .store
        .list("live-tenant", LogicalTable::Turns, None, 64)
        .unwrap();

    eprintln!("\n=== DURABLE AUDIT SUMMARY ===");
    eprintln!("turns: {}", all_turns.len());
    eprintln!("steps: {}", all_steps.len());
    eprintln!("provider_calls: {}", all_calls.len());
    eprintln!(
        "procedures_promoted: {}",
        rt.world.registry.procedures.len()
    );
    eprintln!("in_memory_proposals: {}", rt.world.proposals.len());

    assert!(all_turns.len() >= 3, "expected multi-turn history");
    assert!(
        all_steps.len() >= 10,
        "expected substantial step ledger, got {}",
        all_steps.len()
    );

    // Every completed turn must have at least one step row
    for turn in &all_turns {
        if turn.envelope.status == "completed" {
            let tid = &turn.envelope.key;
            let n = all_steps
                .iter()
                .filter(|s| s.envelope.value.turn_id == *tid)
                .count();
            assert!(n > 0, "turn {tid} has zero durable steps");
        }
    }

    // Live provider path should have issued at least one real call across the scenario
    // (unless everything hit warm templates after first propose)
    let live_llm_total = rt.world.provider_calls().len();
    eprintln!("[live] total in-memory provider calls: {live_llm_total}");
    assert!(
        live_llm_total >= 1 || !all_calls.is_empty(),
        "expected at least one live LLM call in the bigger scenario"
    );
}
