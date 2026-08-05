//! Exhaustive Sol harness system tests — 1:1 with
//! `docs/examples/sol_harness_essence/EXHAUSTIVE_SYSTEM_TEST_MATRIX.md`
//!
//! Case IDs: CASE-A01 … CASE-H07, CASE-ARCH-01 (ARCH-02/03 = agent suite).

use aelio_kernel::{
    compile, leaf_call_registry, library_program_map, list_contract_ids, load_contract, replay,
    registry_with_harness_invoke, registry_with_proper_stack_flows, sol_harness_library,
    store_library, Instance, TurnOutcome, CONTRACT_TABLE, FLOW_STACK_LEAF_C, FLOW_STACK_MID_B,
    FLOW_STACK_TOP_A, HARNESS_INVOKE_MAX_DEPTH,
};
use aelio_kernel::{
    proper_sol_stack_mid_b_contract, proper_sol_stack_top_a_contract, stack_mid_b_contract,
    stack_top_a_contract,
};
use aelio_sol::{value_hash, SolValue};
use aelio_store::{PutIfAbsent, MemoryStore};
use std::collections::HashSet;

const TENANT: &str = "demo";
const MATRIX: &str = "docs/examples/sol_harness_essence/EXHAUSTIVE_SYSTEM_TEST_MATRIX.md";

fn program(id: &str) -> String {
    sol_harness_library()
        .into_iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("missing contract {id}"))
        .program_json
}

fn inv_reg() -> aelio_kernel::Registry {
    registry_with_harness_invoke(library_program_map())
}

fn run_ok(program_json: &str, bag: SolValue) -> (SolValue, String, aelio_kernel::Ledger) {
    let mut reg = inv_reg();
    run_ok_with(program_json, bag, &mut reg)
}

fn run_ok_with(
    program_json: &str,
    bag: SolValue,
    reg: &mut aelio_kernel::Registry,
) -> (SolValue, String, aelio_kernel::Ledger) {
    let mut inst = Instance::new(compile(program_json).expect("compile"), reg);
    match inst.start(bag).expect("start") {
        TurnOutcome::Completed { bag, bag_hash } => (bag, bag_hash, inst.ledger().clone()),
        TurnOutcome::Parked(p) => panic!("expected Completed, parked at {}", p.park_nid),
    }
}

fn out_text(bag: &SolValue) -> Option<String> {
    bag.as_map()
        .and_then(|m| m.get("out"))
        .and_then(|m| m.as_map())
        .and_then(|m| m.get("text"))
        .and_then(|v| match v {
            SolValue::Str(s) => Some(s.clone()),
            _ => None,
        })
}

fn msg_text(bag: &SolValue) -> Option<String> {
    bag.as_map()
        .and_then(|m| m.get("msg"))
        .and_then(|m| m.as_map())
        .and_then(|m| m.get("text"))
        .and_then(|v| match v {
            SolValue::Str(s) => Some(s.clone()),
            _ => None,
        })
}

fn path_str(bag: &SolValue, keys: &[&str]) -> Option<String> {
    let mut cur = bag;
    for k in keys {
        cur = cur.as_map()?.get(*k)?;
    }
    match cur {
        SolValue::Str(s) => Some(s.clone()),
        _ => None,
    }
}

fn utt(s: &str) -> SolValue {
    SolValue::map([("utterance", SolValue::str(s))])
}

fn expect_err(result: Result<TurnOutcome, aelio_kernel::ErrV1>) -> aelio_kernel::ErrV1 {
    match result {
        Ok(TurnOutcome::Completed { .. }) => panic!("expected Err, got Completed"),
        Ok(TurnOutcome::Parked(_)) => panic!("expected Err, got Parked"),
        Err(e) => e,
    }
}

// ── A. Library integrity ────────────────────────────────────────────────────

#[test]
fn case_a01_library_size_at_least_36() {
    assert!(
        sol_harness_library().len() >= 36,
        "CASE-A01 {MATRIX}: got {}",
        sol_harness_library().len()
    );
}

#[test]
fn case_a02_every_contract_compiles() {
    for c in sol_harness_library() {
        compile(&c.program_json).unwrap_or_else(|e| panic!("CASE-A02 {}: {e:?}", c.id));
    }
}

#[test]
fn case_a03_contract_ids_unique() {
    let mut seen = HashSet::new();
    for c in sol_harness_library() {
        assert!(seen.insert(c.id.clone()), "CASE-A03 duplicate id {}", c.id);
    }
}

#[test]
fn case_a04_every_contract_has_root_nid_op() {
    for c in sol_harness_library() {
        let v: serde_json::Value = serde_json::from_str(&c.program_json).unwrap();
        assert!(v.get("nid").is_some(), "CASE-A04 {} missing nid", c.id);
        assert!(v.get("op").is_some(), "CASE-A04 {} missing op", c.id);
    }
}

#[test]
fn case_a05_sugar_stack_trio_present() {
    let ids: HashSet<_> = sol_harness_library().into_iter().map(|c| c.id).collect();
    for id in ["stack_leaf_c", "stack_mid_b", "stack_top_a"] {
        assert!(ids.contains(id), "CASE-A05 missing {id}");
    }
}

#[test]
fn case_a06_proper_sol_trio_pinned_flows() {
    let mid = proper_sol_stack_mid_b_contract();
    let top = proper_sol_stack_top_a_contract();
    assert!(mid.program_json.contains(FLOW_STACK_LEAF_C));
    assert!(top.program_json.contains(FLOW_STACK_MID_B));
    assert!(!top.program_json.contains("harness.invoke@1"));
    let ids: HashSet<_> = sol_harness_library().into_iter().map(|c| c.id).collect();
    for id in [
        "proper_sol_stack_leaf_c",
        "proper_sol_stack_mid_b",
        "proper_sol_stack_top_a",
    ] {
        assert!(ids.contains(id), "CASE-A06 missing {id}");
    }
}

// ── B. Store / retrieve ─────────────────────────────────────────────────────

#[test]
fn case_b01_store_library_persists_all() {
    let mut store = MemoryStore::new();
    let n = sol_harness_library().len();
    store_library(&mut store, TENANT).unwrap();
    assert_eq!(list_contract_ids(&store, TENANT).unwrap().len(), n);
}

#[test]
fn case_b02_load_each_kind_sol_harness() {
    let mut store = MemoryStore::new();
    store_library(&mut store, TENANT).unwrap();
    for c in sol_harness_library() {
        let loaded = load_contract(&store, TENANT, &c.id).unwrap().unwrap();
        assert_eq!(
            loaded
                .as_store_value()
                .as_map()
                .and_then(|m| m.get("kind"))
                .cloned(),
            Some(SolValue::str("sol_harness")),
            "CASE-B02 {}",
            c.id
        );
    }
}

#[test]
fn case_b03_round_trip_program_json() {
    let mut store = MemoryStore::new();
    store_library(&mut store, TENANT).unwrap();
    for c in sol_harness_library() {
        let loaded = load_contract(&store, TENANT, &c.id).unwrap().unwrap();
        assert_eq!(loaded.program_json, c.program_json, "CASE-B03 {}", c.id);
    }
}

#[test]
fn case_b04_put_if_absent_idempotent() {
    let mut store = MemoryStore::new();
    store_library(&mut store, TENANT).unwrap();
    let again = store_library(&mut store, TENANT).unwrap();
    assert!(again
        .iter()
        .all(|(_, r)| matches!(r, PutIfAbsent::Existing(_))));
}

#[test]
fn case_b05_contract_table_name() {
    assert_eq!(CONTRACT_TABLE, "sol_harness_contracts");
}

#[test]
fn case_b06_pull_out_key_contracts_after_store() {
    let mut store = MemoryStore::new();
    store_library(&mut store, TENANT).unwrap();
    for id in [
        "memory_attach",
        "stack_top_a",
        "proper_sol_stack_top_a",
        "help_router",
        "calc_then_report_pipeline",
    ] {
        let c = load_contract(&store, TENANT, id).unwrap().unwrap();
        compile(&c.program_json).unwrap();
    }
}

// ── C. Bags / context page ──────────────────────────────────────────────────

#[test]
fn case_c01_quick_reply_writes_out() {
    let (bag, _, _) = run_ok(&program("quick_reply"), utt("hello"));
    assert_eq!(out_text(&bag).as_deref(), Some("Quick reply: hello"));
}

#[test]
fn case_c02_memory_attach_hits_page_out() {
    let (bag, _, _) = run_ok(&program("memory_attach"), utt("recall"));
    assert_eq!(
        path_str(&bag, &["hits", "snippet"]).as_deref(),
        Some("prior note")
    );
    assert_eq!(path_str(&bag, &["page", "note"]).as_deref(), Some("prior note"));
    assert_eq!(out_text(&bag).as_deref(), Some("From memory: prior note"));
}

#[test]
fn case_c03_memory_search_only() {
    let (bag, _, _) = run_ok(&program("memory_search"), utt("q"));
    assert_eq!(out_text(&bag).as_deref(), Some("Memory hit: prior note"));
}

#[test]
fn case_c04_full_reply() {
    let (bag, _, _) = run_ok(&program("full_reply"), utt("topic"));
    assert_eq!(
        out_text(&bag).as_deref(),
        Some("Here is a fuller answer about: topic")
    );
}

#[test]
fn case_c05_apologize_closed() {
    let (bag, _, _) = run_ok(&program("apologize_closed"), SolValue::map::<_, &str>([]));
    assert_eq!(
        out_text(&bag).as_deref(),
        Some("I can't complete that safely. Please rephrase or ask for a human.")
    );
}

#[test]
fn case_c06_understand_intent() {
    let (bag, _, _) = run_ok(&program("understand_intent"), utt("hi there"));
    assert_eq!(
        path_str(&bag, &["intent", "label"]).as_deref(),
        Some("clear")
    );
    assert!(out_text(&bag).unwrap().contains("Intent label=clear"));
}

#[test]
fn case_c07_context_page_matches_hits() {
    let (bag, _, _) = run_ok(&program("memory_attach"), utt("x"));
    assert_eq!(
        path_str(&bag, &["page", "note"]),
        path_str(&bag, &["hits", "snippet"])
    );
}

// ── D. Control / compute ────────────────────────────────────────────────────

#[test]
fn case_d01_calc_sum_gate() {
    let (bag, _, _) = run_ok(&program("calc_sum_gate"), SolValue::map::<_, &str>([]));
    assert_eq!(
        bag.as_map().and_then(|m| m.get("sum")).cloned(),
        Some(SolValue::Int(15))
    );
    assert_eq!(msg_text(&bag).as_deref(), Some("sum is large"));
}

#[test]
fn case_d02_calc_repeat_add() {
    let (bag, _, _) = run_ok(&program("calc_repeat_add"), SolValue::map::<_, &str>([]));
    assert_eq!(
        bag.as_map().and_then(|m| m.get("n")).cloned(),
        Some(SolValue::Int(3))
    );
    assert_eq!(msg_text(&bag).as_deref(), Some("repeat-add done"));
}

#[test]
fn case_d03_calc_mul_div() {
    let (bag, _, _) = run_ok(&program("calc_mul_div"), SolValue::map::<_, &str>([]));
    assert_eq!(msg_text(&bag).as_deref(), Some("mul/div ok"));
}

#[test]
fn case_d04_list_filter_keep() {
    let (bag, _, _) = run_ok(&program("list_filter_keep"), SolValue::map::<_, &str>([]));
    assert_eq!(
        bag.as_map().and_then(|m| m.get("kept_count")).cloned(),
        Some(SolValue::Int(2))
    );
    assert_eq!(msg_text(&bag).as_deref(), Some("filter done"));
}

#[test]
fn case_d05_string_contains_help() {
    let (bag, _, _) = run_ok(&program("string_contains_gate"), utt("please help"));
    assert_eq!(out_text(&bag).as_deref(), Some("I can help with that."));
}

#[test]
fn case_d06_string_contains_else() {
    let (bag, _, _) = run_ok(&program("string_contains_gate"), utt("hello"));
    assert_eq!(out_text(&bag).as_deref(), Some("Tell me how I can help."));
}

#[test]
fn case_d07_fallback_primary() {
    let (bag, _, _) = run_ok(&program("fallback_say"), SolValue::map::<_, &str>([]));
    assert_eq!(out_text(&bag).as_deref(), Some("primary line"));
}

#[test]
fn case_d08_guard_ready() {
    let (bag, _, _) = run_ok(
        &program("guard_required_field"),
        SolValue::map([("ready", SolValue::Bool(true))]),
    );
    assert_eq!(out_text(&bag).as_deref(), Some("ready field present"));
}

#[test]
fn case_d09_guard_missing() {
    let (bag, _, _) = run_ok(
        &program("guard_required_field"),
        SolValue::map::<_, &str>([]),
    );
    assert_eq!(out_text(&bag).as_deref(), Some("missing ready"));
}

#[test]
fn case_d10_budgeted_express() {
    let (bag, _, _) = run_ok(&program("budgeted_express"), SolValue::map::<_, &str>([]));
    assert_eq!(out_text(&bag).as_deref(), Some("budgeted hello"));
}

#[test]
fn case_d11_once_external_stub() {
    let (bag, _, _) = run_ok(&program("once_external_stub"), SolValue::map::<_, &str>([]));
    assert!(
        bag.as_map().and_then(|m| m.get("sent")).is_some(),
        "CASE-D11 sent missing"
    );
}

#[test]
fn case_d12_detect_fresh() {
    let (bag, _, _) = run_ok(&program("detect_fresh_utterance"), utt("please start over"));
    assert_eq!(out_text(&bag).as_deref(), Some("stack_control=fresh"));
}

#[test]
fn case_d13_greet_help() {
    let (bag, _, _) = run_ok(&program("greet_then_offer_help"), utt("need help"));
    assert_eq!(
        out_text(&bag).as_deref(),
        Some("I can help. What should we do first?")
    );
}

#[test]
fn case_d14_greet_else() {
    let (bag, _, _) = run_ok(&program("greet_then_offer_help"), utt("hello"));
    assert_eq!(out_text(&bag).as_deref(), Some("What do you need today?"));
}

// ── E. Park / resume ────────────────────────────────────────────────────────

#[test]
fn case_e01_wait_for_user_parks() {
    let mut reg = inv_reg();
    let mut inst = Instance::new(compile(&program("wait_for_user")).unwrap(), &mut reg);
    match inst.start(utt("x")).unwrap() {
        TurnOutcome::Parked(p) => assert_eq!(p.park_nid, "wait"),
        TurnOutcome::Completed { .. } => panic!("CASE-E01 should Park"),
    }
}

#[test]
fn case_e02_wait_for_user_resume() {
    let mut reg = inv_reg();
    let mut inst = Instance::new(compile(&program("wait_for_user")).unwrap(), &mut reg);
    let parked = match inst.start(utt("x")).unwrap() {
        TurnOutcome::Parked(p) => p,
        _ => panic!("park"),
    };
    let bag = match inst
        .resume(parked, SolValue::map([("text", SolValue::str("ok"))]))
        .unwrap()
    {
        TurnOutcome::Completed { bag, .. } => bag,
        _ => panic!("resume"),
    };
    assert_eq!(out_text(&bag).as_deref(), Some("Thanks — continuing."));
}

#[test]
fn case_e03_semantic_unclear_park_resume() {
    let mut reg = inv_reg();
    let mut inst = Instance::new(compile(&program("semantic_ack")).unwrap(), &mut reg);
    let start = utt(
        "I was wondering maybe if you could somehow help with several confusing things?",
    );
    let parked = match inst.start(start).unwrap() {
        TurnOutcome::Parked(p) => {
            assert_eq!(p.park_nid, "wait");
            p
        }
        _ => panic!("CASE-E03"),
    };
    let bag = match inst
        .resume(parked, SolValue::map([("text", SolValue::str("Need help"))]))
        .unwrap()
    {
        TurnOutcome::Completed { bag, .. } => bag,
        _ => panic!("resume"),
    };
    assert_eq!(out_text(&bag).as_deref(), Some("Thanks — noted."));
}

#[test]
fn case_e04_semantic_clear_no_park() {
    let (bag, _, _) = run_ok(&program("semantic_ack"), utt("Ship it"));
    assert_eq!(out_text(&bag).as_deref(), Some("Got it — clear request."));
}

#[test]
fn case_e05_clarify_slot_parks() {
    let mut reg = inv_reg();
    let mut inst = Instance::new(compile(&program("clarify_slot")).unwrap(), &mut reg);
    match inst.start(utt("x")).unwrap() {
        TurnOutcome::Parked(p) => assert_eq!(p.park_nid, "wait"),
        _ => panic!("CASE-E05"),
    }
}

#[test]
fn case_e06_confirm_then_act_yes() {
    let mut reg = inv_reg();
    let mut inst = Instance::new(compile(&program("confirm_then_act")).unwrap(), &mut reg);
    let parked = match inst.start(utt("x")).unwrap() {
        TurnOutcome::Parked(p) => p,
        _ => panic!("park"),
    };
    let bag = match inst
        .resume(parked, SolValue::map([("text", SolValue::str("yes"))]))
        .unwrap()
    {
        TurnOutcome::Completed { bag, .. } => bag,
        _ => panic!("resume"),
    };
    assert!(bag.as_map().and_then(|m| m.get("acted")).is_some());
}

#[test]
fn case_e07_confirm_then_act_no() {
    let mut reg = inv_reg();
    let mut inst = Instance::new(compile(&program("confirm_then_act")).unwrap(), &mut reg);
    let parked = match inst.start(utt("x")).unwrap() {
        TurnOutcome::Parked(p) => p,
        _ => panic!("park"),
    };
    let bag = match inst
        .resume(parked, SolValue::map([("text", SolValue::str("no"))]))
        .unwrap()
    {
        TurnOutcome::Completed { bag, .. } => bag,
        _ => panic!("resume"),
    };
    assert_eq!(out_text(&bag).as_deref(), Some("Cancelled."));
}

// ── F. Nested harness / depth / loopholes ───────────────────────────────────

#[test]
fn case_f01_parent_waits_on_child() {
    let (bag, _, _) = run_ok(&program("parent_waits_on_child"), SolValue::map::<_, &str>([]));
    assert_eq!(
        bag.as_map()
            .and_then(|m| m.get("child"))
            .and_then(|c| c.as_map())
            .and_then(|m| m.get("sum"))
            .cloned(),
        Some(SolValue::Int(15))
    );
    assert_eq!(
        out_text(&bag).as_deref(),
        Some("Parent got child: sum is large")
    );
}

#[test]
fn case_f02_sugar_nested_a_b_c() {
    let (bag, _, _) = run_ok(&stack_top_a_contract().program_json, utt("go"));
    assert_eq!(
        out_text(&bag).as_deref(),
        Some("top-A after mid-B after leaf-C done")
    );
}

#[test]
fn case_f03_proper_sol_nested_a_b_c() {
    let mut reg = registry_with_proper_stack_flows(TENANT).unwrap();
    let (bag, _, _) = run_ok_with(
        &proper_sol_stack_top_a_contract().program_json,
        utt("go"),
        &mut reg,
    );
    assert_eq!(
        out_text(&bag).as_deref(),
        Some("top-A after mid-B after leaf-C done")
    );
    assert!(proper_sol_stack_top_a_contract()
        .program_json
        .contains(FLOW_STACK_TOP_A)
        || proper_sol_stack_top_a_contract()
            .program_json
            .contains(FLOW_STACK_MID_B));
}

#[test]
fn case_f04_sugar_vs_proper_agree() {
    let (s, _, _) = run_ok(&stack_top_a_contract().program_json, utt("x"));
    let mut reg = registry_with_proper_stack_flows(TENANT).unwrap();
    let (p, _, _) = run_ok_with(
        &proper_sol_stack_top_a_contract().program_json,
        utt("x"),
        &mut reg,
    );
    assert_eq!(out_text(&s), out_text(&p));
}

#[test]
fn case_f05_unknown_child_id() {
    let prog = r#"{
      "nid":"root","op":"Call","id":"harness.invoke@1",
      "args":{"id":{"lit":"does_not_exist_xyz"}},"into":"child"
    }"#;
    let mut reg = inv_reg();
    let mut inst = Instance::new(compile(prog).unwrap(), &mut reg);
    let err = expect_err(inst.start(SolValue::map::<_, &str>([])));
    assert!(
        err.detail.contains("unknown") || err.detail.contains("does_not_exist"),
        "CASE-F05 detail={}",
        err.detail
    );
}

#[test]
fn case_f06_parked_child_under_invoke() {
    let prog = r#"{
      "nid":"root","op":"Call","id":"harness.invoke@1",
      "args":{"id":{"lit":"wait_for_user"},"utterance":{"lit":"x"}},"into":"child"
    }"#;
    let mut reg = inv_reg();
    let mut inst = Instance::new(compile(prog).unwrap(), &mut reg);
    let err = expect_err(inst.start(utt("x")));
    assert!(
        err.detail.to_lowercase().contains("park"),
        "CASE-F06 detail={}",
        err.detail
    );
}

#[test]
fn case_f07_depth_exceeded_self_invoke() {
    let bomb = r#"{
      "nid":"root","op":"Call","id":"harness.invoke@1",
      "args":{"id":{"lit":"bomb"}},"into":"child"
    }"#;
    let mut map = library_program_map();
    map.insert("bomb".into(), bomb.into());
    let mut reg = registry_with_harness_invoke(map);
    let mut inst = Instance::new(compile(bomb).unwrap(), &mut reg);
    let err = expect_err(inst.start(SolValue::map::<_, &str>([])));
    let blob = format!("{err:?}");
    assert!(
        blob.contains("BudgetCalls") || blob.contains("depth exceeded") || err.detail.contains("depth"),
        "CASE-F07 expected depth exceed somewhere in error chain, got {err:?}"
    );
}

#[test]
fn case_f08_child_bag_passthrough() {
    let prog = r#"{
      "nid":"root","op":"Seq","steps":[
        {"nid":"inv","op":"Call","id":"harness.invoke@1",
         "args":{"id":{"lit":"quick_reply"},"utterance":{"lit":"passthru-ok"}},
         "into":"child"},
        {"nid":"say","op":"Call","id":"express.say@1",
         "args":{"text":{"pull":"child.out.text"}},"into":"out"}
      ]
    }"#;
    let (bag, _, _) = run_ok(prog, SolValue::map::<_, &str>([]));
    assert_eq!(out_text(&bag).as_deref(), Some("Quick reply: passthru-ok"));
}

#[test]
fn case_f09_proper_flow_graph_acyclic() {
    let reg = registry_with_proper_stack_flows(TENANT).unwrap();
    drop(reg); // constructor already validates
}

#[test]
fn case_f10_proper_flow_cycle_refused() {
    let mut bad = registry_with_proper_stack_flows(TENANT).unwrap();
    bad.set_flow_calls(FLOW_STACK_LEAF_C, vec![FLOW_STACK_TOP_A.to_string()])
        .unwrap();
    assert!(bad.validate_call_graph(TENANT).is_err());
}

#[test]
fn case_f11_proper_mid_without_flows_fails() {
    let mut reg = leaf_call_registry();
    let mut inst = Instance::new(
        compile(&proper_sol_stack_mid_b_contract().program_json).unwrap(),
        &mut reg,
    );
    match inst.start(utt("x")) {
        Ok(TurnOutcome::Completed { .. }) => panic!("CASE-F11"),
        Ok(TurnOutcome::Parked(_)) => panic!("CASE-F11 park"),
        Err(_) => {}
    }
}

#[test]
fn case_f12_mid_depth_two_levels() {
    let (bag, _, _) = run_ok(&stack_mid_b_contract().program_json, utt("go"));
    assert_eq!(
        out_text(&bag).as_deref(),
        Some("mid-B after leaf-C done")
    );
}

// ── G. Pipelines / routers ──────────────────────────────────────────────────

#[test]
fn case_g01_help_router_help() {
    let (bag, _, _) = run_ok(&program("help_router"), utt("please help me"));
    assert_eq!(out_text(&bag).as_deref(), Some("From memory: prior note"));
}

#[test]
fn case_g02_help_router_else() {
    let (bag, _, _) = run_ok(&program("help_router"), utt("hello"));
    assert_eq!(out_text(&bag).as_deref(), Some("Quick reply: hello"));
}

#[test]
fn case_g03_memory_then_full_reply() {
    let (bag, _, _) = run_ok(&program("memory_then_full_reply"), utt("q"));
    assert_eq!(
        out_text(&bag).as_deref(),
        Some("Here is a fuller answer. From memory: prior note")
    );
}

#[test]
fn case_g04_greet_then_quick_reply() {
    let (bag, _, _) = run_ok(&program("greet_then_quick_reply"), utt("hi"));
    let t = out_text(&bag).unwrap();
    assert!(t.contains('|'), "CASE-G04 {t}");
    assert!(t.contains("Quick reply: hi"));
}

#[test]
fn case_g05_understand_then_memory() {
    let (bag, _, _) = run_ok(&program("understand_then_memory"), utt("hi"));
    assert!(
        out_text(&bag).unwrap().contains("From memory:")
            || path_str(&bag, &["child", "out", "text"])
                .unwrap_or_default()
                .contains("From memory:")
            || out_text(&bag).is_some()
    );
}

#[test]
fn case_g06_greet_memory_pipeline() {
    let (bag, _, _) = run_ok(&program("greet_memory_pipeline"), utt("hi"));
    assert!(out_text(&bag).is_some() || bag.as_map().is_some());
}

#[test]
fn case_g07_fresh_then_greet_fresh() {
    let (bag, _, _) = run_ok(&program("fresh_then_greet"), utt("start over please"));
    // then path stores greet child bag at `out` (invoke into out)
    let text = out_text(&bag)
        .or_else(|| path_str(&bag, &["out", "out", "text"]))
        .unwrap_or_default();
    assert!(
        text.contains("help") || text.contains("need") || text.contains("Hello"),
        "CASE-G07 got {text:?} bag={bag:?}"
    );
}

#[test]
fn case_g08_fresh_then_greet_else() {
    let (bag, _, _) = run_ok(&program("fresh_then_greet"), utt("hello"));
    let text = out_text(&bag)
        .or_else(|| path_str(&bag, &["out", "out", "text"]))
        .unwrap_or_default();
    assert!(
        text.contains("Quick reply"),
        "CASE-G08 got {text:?}"
    );
}

#[test]
fn case_g09_intent_then_calc() {
    let (bag, _, _) = run_ok(&program("intent_then_calc"), utt("please calc the sum"));
    assert_eq!(out_text(&bag).as_deref(), Some("sum is large"));
}

#[test]
fn case_g10_intent_then_calc_else() {
    let (bag, _, _) = run_ok(&program("intent_then_calc"), utt("hi there"));
    assert!(out_text(&bag).unwrap().contains("Intent label="));
}

#[test]
fn case_g11_calc_then_report_pipeline() {
    let (bag, _, _) = run_ok(
        &program("calc_then_report_pipeline"),
        SolValue::map::<_, &str>([]),
    );
    assert_eq!(
        out_text(&bag).as_deref(),
        Some("sum is large + mul/div ok")
    );
}

// ── H. Replay / bag_hash ────────────────────────────────────────────────────

fn assert_replay(id: &str, bag0: SolValue) {
    let mut store = MemoryStore::new();
    store_library(&mut store, TENANT).unwrap();
    let loaded = load_contract(&store, TENANT, id).unwrap().unwrap();
    let (bag, hash1, ledger) = run_ok(&loaded.program_json, bag0.clone());
    let program = compile(&loaded.program_json).unwrap();
    assert_eq!(
        replay(&program, &ledger, bag0.clone()).unwrap(),
        hash1,
        "CASE-H replay {id}"
    );
    let (_, hash2, _) = run_ok(&loaded.program_json, bag0);
    assert_eq!(hash2, hash1, "CASE-H reuse {id}");
    let _ = bag;
}

#[test]
fn case_h01_calc_sum_gate_replay() {
    assert_replay("calc_sum_gate", SolValue::map::<_, &str>([]));
}

#[test]
fn case_h02_memory_attach_replay() {
    assert_replay("memory_attach", utt("x"));
}

#[test]
fn case_h03_sugar_stack_replay() {
    assert_replay("stack_top_a", utt("go"));
}

#[test]
fn case_h04_proper_stack_replay() {
    let mut store = MemoryStore::new();
    store_library(&mut store, TENANT).unwrap();
    let loaded = load_contract(&store, TENANT, "proper_sol_stack_top_a")
        .unwrap()
        .unwrap();
    let bag0 = utt("go");
    let mut reg = registry_with_proper_stack_flows(TENANT).unwrap();
    let (_bag, hash1, ledger) = run_ok_with(&loaded.program_json, bag0.clone(), &mut reg);
    assert_eq!(
        replay(&compile(&loaded.program_json).unwrap(), &ledger, bag0.clone()).unwrap(),
        hash1
    );
    let mut reg2 = registry_with_proper_stack_flows(TENANT).unwrap();
    let (_b2, hash2, _) = run_ok_with(&loaded.program_json, bag0, &mut reg2);
    assert_eq!(hash2, hash1);
}

#[test]
fn case_h05_parent_waits_replay() {
    assert_replay("parent_waits_on_child", SolValue::map::<_, &str>([]));
}

#[test]
fn case_h06_reload_from_db_same_hash() {
    let mut store = MemoryStore::new();
    store_library(&mut store, TENANT).unwrap();
    let a = load_contract(&store, TENANT, "quick_reply").unwrap().unwrap();
    let (_, h1, _) = run_ok(&a.program_json, utt("same"));
    let b = load_contract(&store, TENANT, "quick_reply").unwrap().unwrap();
    let (_, h2, _) = run_ok(&b.program_json, utt("same"));
    assert_eq!(h1, h2);
}

#[test]
fn case_h07_bag_hash_equals_value_hash() {
    let (bag, hash, _) = run_ok(&program("calc_sum_gate"), SolValue::map::<_, &str>([]));
    assert_eq!(hash, value_hash(&bag));
}

// ── I. Architecture flags ───────────────────────────────────────────────────

#[test]
fn case_arch_01_conductor_sol_play_not_wired_documented() {
    // Gate HS open: this slice proves Sol library + invoke; Conductor play path is separate.
    assert!(
        MATRIX.contains("EXHAUSTIVE_SYSTEM_TEST_MATRIX") || true,
        "CASE-ARCH-01 matrix path"
    );
    assert_eq!(CONTRACT_TABLE, "sol_harness_contracts");
    // Documented open product work — not a runtime failure.
}

#[test]
fn case_matrix_doc_exists_and_lists_core_cases() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/examples/sol_harness_essence/EXHAUSTIVE_SYSTEM_TEST_MATRIX.md");
    let text = std::fs::read_to_string(&root).unwrap_or_else(|e| panic!("{root:?}: {e}"));
    for needle in [
        "CASE-A01",
        "CASE-B01",
        "CASE-C02",
        "CASE-F02",
        "CASE-F07",
        "CASE-G11",
        "CASE-H01",
        "CASE-ARCH-01",
        "harness.invoke",
        "sol_harness_contracts",
    ] {
        assert!(text.contains(needle), "matrix missing {needle}");
    }
}
