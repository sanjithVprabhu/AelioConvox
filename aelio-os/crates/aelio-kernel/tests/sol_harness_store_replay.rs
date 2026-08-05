//! Sol harness essence demo (HS stress slice):
//!
//!   prompt → (mock) LLM devises Sol steps → store contract in DB → retrieve → run → replay
//!
//! Full seed library lives in [`aelio_kernel::sol_harness_library`] (6 contracts).
//! This file drills calc + semantic devise/replay and stores the whole library.

use aelio_kernel::{
    compile, list_contract_ids, load_contract, replay, sol_harness_library, store_library,
    EffectClass, Instance, Registry, TurnOutcome, CONTRACT_TABLE,
};
use aelio_sol::{value_hash, SolValue};
use aelio_store::MemoryStore;

const TENANT: &str = "demo";

/// Mock “LLM devises steps” — same role as `MockDrafter` in aelio-runtime forge.
fn devise_sol_from_prompt(prompt: &str) -> aelio_kernel::SolHarnessContract {
    let lower = prompt.to_lowercase();
    let lib = sol_harness_library();
    if lower.contains("sum") || lower.contains("add") || lower.contains("calc") {
        return lib
            .into_iter()
            .find(|c| c.id == "calc_sum_gate")
            .expect("calc in library");
    }
    if lower.contains("sentiment")
        || lower.contains("semantic")
        || lower.contains("sentence")
        || lower.contains("classify")
    {
        return lib
            .into_iter()
            .find(|c| c.id == "semantic_ack")
            .expect("semantic in library");
    }
    panic!("mock drafter: unsupported prompt — use calc/sum/add or sentiment/semantic/sentence");
}

fn demo_registry() -> Registry {
    let mut r = Registry::default();
    r.register("compute.hold@1", EffectClass::Pure, |args| {
        Ok(args
            .as_map()
            .and_then(|m| m.get("v"))
            .cloned()
            .unwrap_or(SolValue::Null))
    });
    r.register("express.say@1", EffectClass::Read, |args| {
        let text = args
            .as_map()
            .and_then(|m| m.get("text"))
            .cloned()
            .unwrap_or_else(|| SolValue::str(""));
        Ok(SolValue::map([("text", text)]))
    });
    r.register("understand.classify@1", EffectClass::Read, |args| {
        let text = args
            .as_map()
            .and_then(|m| m.get("text"))
            .and_then(|v| match v {
                SolValue::Str(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");
        let label = if text.split_whitespace().count() <= 6 && !text.contains('?') {
            "clear"
        } else {
            "unclear"
        };
        Ok(SolValue::map([("label", SolValue::str(label))]))
    });
    r.register("memory.search@1", EffectClass::Read, |_| {
        Ok(SolValue::map([("snippet", SolValue::str("prior note"))]))
    });
    r.register("context.attach@1", EffectClass::Write, |args| {
        let snippet = args
            .as_map()
            .and_then(|m| m.get("snippet"))
            .cloned()
            .unwrap_or_else(|| SolValue::str(""));
        Ok(SolValue::map([("note", snippet)]))
    });
    r
}

fn run_to_completion(
    program_json: &str,
    bag: SolValue,
    registry: &mut Registry,
) -> (SolValue, String, aelio_kernel::Ledger) {
    let program = compile(program_json).expect("Sol harness must compile + plan");
    let mut inst = Instance::new(program.clone(), registry);
    match inst.start(bag).expect("start") {
        TurnOutcome::Completed { bag, bag_hash } => (bag, bag_hash, inst.ledger().clone()),
        TurnOutcome::Parked(_) => panic!("expected completion (no park)"),
    }
}

#[test]
fn store_full_sol_harness_library() {
    let mut store = MemoryStore::new();
    store_library(&mut store, TENANT).unwrap();
    let ids = list_contract_ids(&store, TENANT).unwrap();
    assert!(ids.len() >= 36, "expected combination-expanded library, got {}", ids.len());
    assert_eq!(CONTRACT_TABLE, "sol_harness_contracts");
    for id in [
        "quick_reply",
        "understand_intent",
        "wait_for_user",
        "memory_attach",
        "calc_sum_gate",
        "semantic_ack",
        "calc_repeat_add",
        "list_filter_keep",
        "fallback_say",
        "budgeted_express",
        "greet_then_offer_help",
        "parent_waits_on_child",
        "help_router",
        "memory_then_full_reply",
        "clarify_slot",
        "memory_search",
        "stack_top_a",
        "proper_sol_stack_top_a",
    ] {
        let c = load_contract(&store, TENANT, id).unwrap().unwrap();
        compile(&c.program_json).unwrap_or_else(|e| panic!("{id}: {e:?}"));
    }
}

#[test]
fn calc_harness_devise_store_retrieve_replay_reuse() {
    let prompt = "Create a calc harness that adds numbers and gates on the sum";
    let devised = devise_sol_from_prompt(prompt);

    let mut store = MemoryStore::new();
    store_library(&mut store, TENANT).unwrap();
    let loaded = load_contract(&store, TENANT, &devised.id)
        .unwrap()
        .expect("calc stored with library");
    assert_eq!(loaded.program_json, devised.program_json);

    let mut registry = demo_registry();
    let bag0 = SolValue::map::<_, &str>([]);
    let (bag, hash1, ledger) = run_to_completion(&loaded.program_json, bag0.clone(), &mut registry);

    assert_eq!(
        bag.as_map().and_then(|m| m.get("sum")),
        Some(&SolValue::Int(15))
    );
    let msg = bag
        .as_map()
        .and_then(|m| m.get("msg"))
        .and_then(|m| m.as_map())
        .and_then(|m| m.get("text"));
    assert_eq!(msg, Some(&SolValue::str("sum is large")));

    let program = compile(&loaded.program_json).unwrap();
    let replay_hash = replay(&program, &ledger, bag0.clone()).expect("replay");
    assert_eq!(replay_hash, hash1);

    let loaded_again = load_contract(&store, TENANT, "calc_sum_gate")
        .unwrap()
        .unwrap();
    let mut registry2 = demo_registry();
    let (_bag2, hash2, _) = run_to_completion(&loaded_again.program_json, bag0, &mut registry2);
    assert_eq!(hash2, hash1);
    assert_eq!(hash1, value_hash(&bag));
}

#[test]
fn semantic_harness_devise_store_park_resume_replay() {
    let devised = devise_sol_from_prompt("semantic sentence classify with clarify park");
    let mut store = MemoryStore::new();
    store_library(&mut store, TENANT).unwrap();
    let loaded = load_contract(&store, TENANT, &devised.id).unwrap().unwrap();

    let program = compile(&loaded.program_json).expect("semantic Sol compiles");
    let mut registry = demo_registry();
    let mut inst = Instance::new(program.clone(), &mut registry);

    let start_bag = SolValue::map([(
        "utterance",
        SolValue::str(
            "I was wondering maybe if you could somehow help with several confusing things?",
        ),
    )]);
    let parked = match inst.start(start_bag.clone()).unwrap() {
        TurnOutcome::Parked(p) => {
            assert_eq!(p.park_nid, "wait");
            p
        }
        TurnOutcome::Completed { .. } => panic!("unclear path should Park"),
    };

    let wake = SolValue::map([("text", SolValue::str("Need hiring help"))]);
    let (bag, bag_hash) = match inst.resume(parked, wake).unwrap() {
        TurnOutcome::Completed { bag, bag_hash } => (bag, bag_hash),
        TurnOutcome::Parked(_) => panic!("should complete after resume"),
    };
    let out = bag
        .as_map()
        .and_then(|m| m.get("out"))
        .and_then(|m| m.as_map())
        .and_then(|m| m.get("text"));
    assert_eq!(out, Some(&SolValue::str("Thanks — noted.")));

    let replay_hash = replay(&program, inst.ledger(), start_bag).expect("replay");
    assert_eq!(replay_hash, bag_hash);

    let mut registry2 = demo_registry();
    let clear_bag = SolValue::map([("utterance", SolValue::str("Book a demo"))]);
    let (bag_clear, hash_clear, ledger_clear) =
        run_to_completion(&loaded.program_json, clear_bag.clone(), &mut registry2);
    let out_clear = bag_clear
        .as_map()
        .and_then(|m| m.get("out"))
        .and_then(|m| m.as_map())
        .and_then(|m| m.get("text"));
    assert_eq!(out_clear, Some(&SolValue::str("Got it — clear request.")));
    assert_eq!(
        replay(
            &compile(&loaded.program_json).unwrap(),
            &ledger_clear,
            clear_bag
        )
        .unwrap(),
        hash_clear
    );
}

#[test]
fn quick_reply_and_memory_attach_from_stored_library() {
    let mut store = MemoryStore::new();
    store_library(&mut store, TENANT).unwrap();
    let mut registry = demo_registry();

    let qr = load_contract(&store, TENANT, "quick_reply").unwrap().unwrap();
    let bag = SolValue::map([("utterance", SolValue::str("hello"))]);
    let (out_bag, _, _) = run_to_completion(&qr.program_json, bag, &mut registry);
    let text = out_bag
        .as_map()
        .and_then(|m| m.get("out"))
        .and_then(|m| m.as_map())
        .and_then(|m| m.get("text"));
    assert_eq!(text, Some(&SolValue::str("Quick reply: hello")));

    let mem = load_contract(&store, TENANT, "memory_attach")
        .unwrap()
        .unwrap();
    let mut registry2 = demo_registry();
    let bag2 = SolValue::map([("utterance", SolValue::str("what did I say"))]);
    let (out2, _, _) = run_to_completion(&mem.program_json, bag2, &mut registry2);
    let text2 = out2
        .as_map()
        .and_then(|m| m.get("out"))
        .and_then(|m| m.as_map())
        .and_then(|m| m.get("text"));
    assert_eq!(text2, Some(&SolValue::str("From memory: prior note")));
}

/// Build a harness from sugar steps → Sol → store → prove the saved body is Sol.
#[test]
fn build_greet_harness_steps_saved_as_sol() {
    use aelio_kernel::greet_then_offer_help_contract;
    use aelio_store::{PutIfAbsent, Store};

    // --- 1) Sugar steps (human plan) ---
    // greet → gate(contains "help") → offer_help | ask_need

    // --- 2) Build Sol contract from those steps ---
    let built = greet_then_offer_help_contract();
    assert_eq!(built.id, "greet_then_offer_help");

    let program: serde_json::Value =
        serde_json::from_str(&built.program_json).expect("program_json is JSON");
    assert_eq!(program["nid"], "root");
    assert_eq!(program["op"], "Seq", "top op must be Sol Seq");
    assert!(program["steps"].is_array());
    assert_eq!(program["steps"][0]["nid"], "greet");
    assert_eq!(program["steps"][0]["op"], "Call");
    assert_eq!(program["steps"][1]["op"], "Branch");

    // --- 3) Planner accepts Sol ---
    compile(&built.program_json).expect("built harness must be valid Sol");

    // --- 4) Save as Sol contract ---
    let mut store = MemoryStore::new();
    let put = store
        .put_if_absent(TENANT, CONTRACT_TABLE, &built.id, built.as_store_value())
        .unwrap();
    assert!(matches!(put, PutIfAbsent::Inserted { .. }));

    // --- 5) Retrieve — prove stored kind is sol_harness + program is Sol ---
    let loaded = load_contract(&store, TENANT, "greet_then_offer_help")
        .unwrap()
        .expect("must be saved");
    let row = store
        .get(TENANT, CONTRACT_TABLE, "greet_then_offer_help")
        .unwrap()
        .unwrap();
    let kind = row.value.as_map().and_then(|m| m.get("kind")).cloned();
    assert_eq!(kind, Some(SolValue::str("sol_harness")));
    assert_eq!(loaded.program_json, built.program_json);

    // --- 6) Use it ---
    let mut registry = demo_registry();
    let bag = SolValue::map([("utterance", SolValue::str("please help me"))]);
    let (out, hash1, ledger) = run_to_completion(&loaded.program_json, bag.clone(), &mut registry);
    let text = out
        .as_map()
        .and_then(|m| m.get("out"))
        .and_then(|m| m.as_map())
        .and_then(|m| m.get("text"));
    assert_eq!(
        text,
        Some(&SolValue::str("I can help. What should we do first?"))
    );

    // --- 7) Replay ledger → same bag_hash ---
    let program = compile(&loaded.program_json).unwrap();
    let replay_hash = replay(&program, &ledger, bag.clone()).expect("replay");
    assert_eq!(replay_hash, hash1, "replay must match live bag_hash");

    // --- 8) Retrieve again + reuse (else branch) ---
    let loaded2 = load_contract(&store, TENANT, "greet_then_offer_help")
        .unwrap()
        .unwrap();
    let mut registry2 = demo_registry();
    let bag_else = SolValue::map([("utterance", SolValue::str("just saying hi"))]);
    let (out2, hash2, ledger2) =
        run_to_completion(&loaded2.program_json, bag_else.clone(), &mut registry2);
    let text2 = out2
        .as_map()
        .and_then(|m| m.get("out"))
        .and_then(|m| m.as_map())
        .and_then(|m| m.get("text"));
    assert_eq!(text2, Some(&SolValue::str("What do you need today?")));
    assert_eq!(
        replay(&compile(&loaded2.program_json).unwrap(), &ledger2, bag_else).unwrap(),
        hash2
    );

    // help-path reuse: same utterance → same hash
    let mut registry3 = demo_registry();
    let (_out3, hash3, _) = run_to_completion(&loaded2.program_json, bag, &mut registry3);
    assert_eq!(hash3, hash1, "reuse of stored Sol must be bit-identical");
}

#[test]
fn parent_harness_calls_child_waits_for_output() {
    use aelio_kernel::{
        library_program_map, parent_waits_on_child_contract, registry_with_harness_invoke,
    };
    use aelio_store::Store;

    // Sugar: delegate(child) → wait (Call blocks) → report(child.msg)
    let parent = parent_waits_on_child_contract();
    compile(&parent.program_json).expect("parent Sol valid");

    let mut store = MemoryStore::new();
    store_library(&mut store, TENANT).unwrap();
    // Also ensure parent is stored
    let _ = store.put_if_absent(
        TENANT,
        CONTRACT_TABLE,
        &parent.id,
        parent.as_store_value(),
    );

    let loaded = load_contract(&store, TENANT, "parent_waits_on_child")
        .unwrap()
        .expect("parent saved as Sol");
    assert_eq!(
        loaded
            .as_store_value()
            .as_map()
            .and_then(|m| m.get("kind"))
            .cloned(),
        Some(SolValue::str("sol_harness"))
    );

    let mut registry = registry_with_harness_invoke(library_program_map());
    let bag0 = SolValue::map::<_, &str>([]);
    let (bag, hash1, ledger) = run_to_completion(&loaded.program_json, bag0.clone(), &mut registry);

    assert_eq!(
        bag.as_map()
            .and_then(|m| m.get("child"))
            .and_then(|c| c.as_map())
            .and_then(|m| m.get("sum")),
        Some(&SolValue::Int(15)),
        "parent must receive child bag after wait"
    );
    let out = bag
        .as_map()
        .and_then(|m| m.get("out"))
        .and_then(|m| m.as_map())
        .and_then(|m| m.get("text"));
    assert_eq!(
        out,
        Some(&SolValue::str("Parent got child: sum is large"))
    );

    let program = compile(&loaded.program_json).unwrap();
    assert_eq!(replay(&program, &ledger, bag0.clone()).unwrap(), hash1);

    let mut registry2 = registry_with_harness_invoke(library_program_map());
    let (_b2, hash2, _) = run_to_completion(&loaded.program_json, bag0, &mut registry2);
    assert_eq!(hash2, hash1);
}

#[test]
fn nested_harness_stack_a_waits_on_b_waits_on_c() {
    use aelio_kernel::{
        library_program_map, registry_with_harness_invoke, stack_top_a_contract,
    };

    // Multiple harnesses: A calls B, B calls C. Each waits (sync Call) until child Completes.
    let top = stack_top_a_contract();
    compile(&top.program_json).expect("stack_top_a Sol valid");

    let mut store = MemoryStore::new();
    store_library(&mut store, TENANT).unwrap();
    for id in ["stack_leaf_c", "stack_mid_b", "stack_top_a"] {
        assert!(
            load_contract(&store, TENANT, id).unwrap().is_some(),
            "expected {id} in library"
        );
    }

    let loaded = load_contract(&store, TENANT, "stack_top_a")
        .unwrap()
        .expect("top of stack stored");

    let mut registry = registry_with_harness_invoke(library_program_map());
    let bag0 = SolValue::map([("utterance", SolValue::str("stack-demo"))]);
    let (bag, hash1, ledger) = run_to_completion(&loaded.program_json, bag0.clone(), &mut registry);

    let text = bag
        .as_map()
        .and_then(|m| m.get("out"))
        .and_then(|m| m.as_map())
        .and_then(|m| m.get("text"));
    assert_eq!(
        text,
        Some(&SolValue::str("top-A after mid-B after leaf-C done")),
        "parent must wait for mid, mid must wait for leaf"
    );

    let program = compile(&loaded.program_json).unwrap();
    assert_eq!(replay(&program, &ledger, bag0.clone()).unwrap(), hash1);

    let mut registry2 = registry_with_harness_invoke(library_program_map());
    let (_b2, hash2, _) = run_to_completion(&loaded.program_json, bag0, &mut registry2);
    assert_eq!(hash2, hash1, "nested stack replay must be bit-identical");
}
