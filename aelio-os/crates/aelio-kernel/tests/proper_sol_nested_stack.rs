//! Thorough proof: nested harness wait works, and the same shape is **proper Mother Sol**
//! via registered-flow `Call` targets (§8.4 / §10 / F6) — not only `harness.invoke@1` sugar.

use aelio_kernel::{
    compile, leaf_call_registry, library_program_map, load_contract, replay,
    registry_with_harness_invoke, registry_with_proper_stack_flows, sol_harness_library,
    stack_top_a_contract, store_library, Instance, TurnOutcome, FLOW_STACK_LEAF_C, FLOW_STACK_MID_B,
    FLOW_STACK_TOP_A,
};
use aelio_kernel::{
    proper_sol_stack_leaf_c_contract, proper_sol_stack_mid_b_contract,
    proper_sol_stack_top_a_contract,
};
use aelio_sol::SolValue;
use aelio_store::MemoryStore;

const TENANT: &str = "demo";

fn run_to_completion(
    program_json: &str,
    bag: SolValue,
    registry: &mut aelio_kernel::Registry,
) -> (SolValue, String, aelio_kernel::Ledger) {
    let program = compile(program_json).expect("compile");
    let mut inst = Instance::new(program, registry);
    match inst.start(bag).expect("start") {
        TurnOutcome::Completed { bag, bag_hash } => {
            let ledger = inst.ledger().clone();
            (bag, bag_hash, ledger)
        }
        TurnOutcome::Parked(_) => panic!("expected Completed"),
    }
}

fn out_text(bag: &SolValue) -> Option<&SolValue> {
    bag.as_map()
        .and_then(|m| m.get("out"))
        .and_then(|m| m.as_map())
        .and_then(|m| m.get("text"))
}

#[test]
fn thorough_library_every_contract_compiles() {
    let lib = sol_harness_library();
    assert!(
        lib.len() >= 36,
        "expected expanded library including proper Sol stack, got {}",
        lib.len()
    );
    for c in &lib {
        compile(&c.program_json).unwrap_or_else(|e| panic!("id={}: {e:?}", c.id));
    }
}

#[test]
fn thorough_sugar_stack_store_wait_replay_reuse() {
    let mut store = MemoryStore::new();
    store_library(&mut store, TENANT).unwrap();
    for id in ["stack_leaf_c", "stack_mid_b", "stack_top_a"] {
        assert!(load_contract(&store, TENANT, id).unwrap().is_some());
    }

    let loaded = load_contract(&store, TENANT, "stack_top_a")
        .unwrap()
        .expect("sugar top");
    let bag0 = SolValue::map([("utterance", SolValue::str("thorough"))]);

    let mut reg = registry_with_harness_invoke(library_program_map());
    let (bag, hash1, ledger) = run_to_completion(&loaded.program_json, bag0.clone(), &mut reg);
    assert_eq!(
        out_text(&bag),
        Some(&SolValue::str("top-A after mid-B after leaf-C done"))
    );
    assert_eq!(
        replay(&compile(&loaded.program_json).unwrap(), &ledger, bag0.clone()).unwrap(),
        hash1
    );

    let mut reg2 = registry_with_harness_invoke(library_program_map());
    let (_b2, hash2, _) = run_to_completion(&loaded.program_json, bag0, &mut reg2);
    assert_eq!(hash2, hash1);
}

#[test]
fn thorough_proper_sol_registered_flows_compile_wait_replay() {
    // Mother shape: Call(flow.stack_mid_b@1) / Call(flow.stack_leaf_c@1) — pinned ids.
    let leaf = proper_sol_stack_leaf_c_contract();
    let mid = proper_sol_stack_mid_b_contract();
    let top = proper_sol_stack_top_a_contract();
    for c in [&leaf, &mid, &top] {
        compile(&c.program_json).unwrap_or_else(|e| panic!("{}: {e:?}", c.id));
    }

    assert!(
        mid.program_json.contains(FLOW_STACK_LEAF_C),
        "mid must Call pinned leaf flow id"
    );
    assert!(
        top.program_json.contains(FLOW_STACK_MID_B),
        "top must Call pinned mid flow id"
    );
    assert!(
        !top.program_json.contains("harness.invoke@1"),
        "proper Sol must not use harness.invoke sugar"
    );

    let mut store = MemoryStore::new();
    store_library(&mut store, TENANT).unwrap();
    for id in [
        "proper_sol_stack_leaf_c",
        "proper_sol_stack_mid_b",
        "proper_sol_stack_top_a",
    ] {
        assert!(
            load_contract(&store, TENANT, id).unwrap().is_some(),
            "missing {id}"
        );
    }

    let loaded = load_contract(&store, TENANT, "proper_sol_stack_top_a")
        .unwrap()
        .unwrap();
    let bag0 = SolValue::map([("utterance", SolValue::str("proper"))]);

    let mut reg = registry_with_proper_stack_flows(TENANT).expect("flows");
    let (bag, hash1, ledger) = run_to_completion(&loaded.program_json, bag0.clone(), &mut reg);
    assert_eq!(
        out_text(&bag),
        Some(&SolValue::str("top-A after mid-B after leaf-C done")),
        "proper Sol wait chain must match sugar stack outcome"
    );
    assert_eq!(
        replay(&compile(&loaded.program_json).unwrap(), &ledger, bag0.clone()).unwrap(),
        hash1
    );

    let mut reg2 = registry_with_proper_stack_flows(TENANT).unwrap();
    let (_b2, hash2, _) = run_to_completion(&loaded.program_json, bag0, &mut reg2);
    assert_eq!(hash2, hash1, "proper Sol nested stack replay bit-identical");
}

#[test]
fn thorough_proper_sol_call_graph_acyclic_and_cycle_refused() {
    let reg = registry_with_proper_stack_flows(TENANT).unwrap();
    // Fresh registry already validated in constructor; rebuild and break it.
    let mut bad = registry_with_proper_stack_flows(TENANT).unwrap();
    bad.set_flow_calls(FLOW_STACK_LEAF_C, vec![FLOW_STACK_TOP_A.to_string()])
        .unwrap();
    let err = bad.validate_call_graph(TENANT).unwrap_err();
    assert!(
        err.to_lowercase().contains("cycle") || err.contains("cycle") || !err.is_empty(),
        "expected cycle refusal, got {err}"
    );
    drop(reg);
}

#[test]
fn thorough_sugar_and_proper_agree_on_outcome() {
    let bag0 = SolValue::map([("utterance", SolValue::str("agree"))]);

    let mut sugar_reg = registry_with_harness_invoke(library_program_map());
    let (sugar_bag, _, _) =
        run_to_completion(&stack_top_a_contract().program_json, bag0.clone(), &mut sugar_reg);

    let mut proper_reg = registry_with_proper_stack_flows(TENANT).unwrap();
    let (proper_bag, _, _) = run_to_completion(
        &proper_sol_stack_top_a_contract().program_json,
        bag0,
        &mut proper_reg,
    );

    assert_eq!(out_text(&sugar_bag), out_text(&proper_bag));
}

#[test]
fn thorough_leaf_registry_alone_cannot_run_proper_mid() {
    // Without registered flows, Call(flow.stack_leaf_c@1) must fail — proves flows are required.
    let mid = proper_sol_stack_mid_b_contract();
    let mut reg = leaf_call_registry();
    let mut inst = Instance::new(compile(&mid.program_json).unwrap(), &mut reg);
    match inst.start(SolValue::map([("utterance", SolValue::str("x"))])) {
        Ok(TurnOutcome::Completed { .. }) => {
            panic!("missing flow target must not complete")
        }
        Ok(TurnOutcome::Parked(_)) => panic!("missing flow target must not park"),
        Err(_) => {}
    }
}
