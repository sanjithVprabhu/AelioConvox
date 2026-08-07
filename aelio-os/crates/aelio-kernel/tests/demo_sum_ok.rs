//! Baby step: `demo.sum_ok` Rung 1→5.
//!
//! Rung 5: Conductor-style Sol (`demo.conductor_baby`) calls `conductor.decide@1`
//! (test stub — not keyword matching in the Sol body), then
//! `harness.invoke@1` by catalog id `demo.sum_ok`.

use aelio_kernel::{
    compile, load_contract, registry_with_harness_invoke, EffectClass, Instance, Registry,
    SolHarnessContract, TurnOutcome, CONTRACT_TABLE,
};
use aelio_sol::SolValue;
use aelio_store::{MemoryStore, PutIfAbsent, Store};
use std::collections::HashMap;

fn registry() -> Registry {
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
    r
}

fn program_json() -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../docs/examples/sol_harness_essence/library/demo_sum_ok.json"
    );
    let raw = std::fs::read_to_string(path).expect("read demo_sum_ok.json");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("parse wrapper");
    v.get("program")
        .expect("program field")
        .to_string()
}

fn reply_text(bag: &SolValue) -> String {
    bag.as_map()
        .and_then(|m| m.get("reply"))
        .and_then(|r| r.as_map())
        .and_then(|m| m.get("text"))
        .and_then(|v| match v {
            SolValue::Str(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn run(x: i64, y: i64) -> (SolValue, String) {
    let program = compile(&program_json()).expect("demo.sum_ok must compile");
    let mut reg = registry();
    let mut inst = Instance::new(program, &mut reg);
    let bag = SolValue::map([("x", SolValue::Int(x)), ("y", SolValue::Int(y))]);
    match inst.start(bag).expect("start") {
        TurnOutcome::Completed { bag, bag_hash } => (bag, bag_hash),
        TurnOutcome::Parked(_) => panic!("demo.sum_ok should not park"),
    }
}

#[test]
fn demo_sum_ok_compiles_and_says_ok_for_2_3() {
    let (bag, hash) = run(2, 3);
    assert_eq!(reply_text(&bag), "ok");
    assert_eq!(
        bag.as_map().and_then(|m| m.get("sum")).cloned(),
        Some(SolValue::Int(5))
    );
    assert!(!hash.is_empty());
}

#[test]
fn demo_sum_ok_says_nope_for_2_2() {
    let (bag, _) = run(2, 2);
    assert_eq!(reply_text(&bag), "nope");
    assert_eq!(
        bag.as_map().and_then(|m| m.get("sum")).cloned(),
        Some(SolValue::Int(4))
    );
}

fn demo_sum_ok_contract() -> SolHarnessContract {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../docs/examples/sol_harness_essence/library/demo_sum_ok.json"
    );
    let raw = std::fs::read_to_string(path).expect("read demo_sum_ok.json");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("parse wrapper");
    let id = v["id"].as_str().expect("id").to_string();
    let summary = v["summary"].as_str().unwrap_or("").to_string();
    let program_json = v.get("program").expect("program").to_string();
    compile(&program_json).expect("seed body must compile");
    SolHarnessContract::seed(id, summary, program_json)
}

fn run_loaded(program_json: &str, x: i64, y: i64) -> (SolValue, String) {
    let program = compile(program_json).expect("loaded program must compile");
    let mut reg = registry();
    let mut inst = Instance::new(program, &mut reg);
    let bag = SolValue::map([("x", SolValue::Int(x)), ("y", SolValue::Int(y))]);
    match inst.start(bag).expect("start") {
        TurnOutcome::Completed { bag, bag_hash } => (bag, bag_hash),
        TurnOutcome::Parked(_) => panic!("demo.sum_ok should not park"),
    }
}

/// Rung 3: persist to `sol_harness_contracts`, retrieve by id, run — same outcomes.
#[test]
fn demo_sum_ok_store_load_run() {
    const TENANT: &str = "demo-baby";
    let contract = demo_sum_ok_contract();
    assert_eq!(contract.id, "demo.sum_ok");
    assert!(!contract.identity.is_empty());

    let mut store = MemoryStore::new();
    let put = store
        .put_if_absent(TENANT, CONTRACT_TABLE, &contract.id, contract.as_store_value())
        .expect("store put");
    assert!(matches!(put, PutIfAbsent::Inserted { .. }));

    let loaded = load_contract(&store, TENANT, "demo.sum_ok")
        .expect("load ok")
        .expect("demo.sum_ok must be present");
    assert_eq!(loaded.id, "demo.sum_ok");
    assert_eq!(loaded.program_json, contract.program_json);
    assert_eq!(loaded.identity, contract.identity);

    let (bag_ok, hash_ok) = run_loaded(&loaded.program_json, 2, 3);
    assert_eq!(reply_text(&bag_ok), "ok");
    assert_eq!(
        bag_ok.as_map().and_then(|m| m.get("sum")).cloned(),
        Some(SolValue::Int(5))
    );

    let (bag_nope, _) = run_loaded(&loaded.program_json, 2, 2);
    assert_eq!(reply_text(&bag_nope), "nope");

    // Reload again — same program, same happy-path hash.
    let loaded_again = load_contract(&store, TENANT, "demo.sum_ok")
        .unwrap()
        .unwrap();
    let (_, hash_ok2) = run_loaded(&loaded_again.program_json, 2, 3);
    assert_eq!(hash_ok2, hash_ok);
}

fn load_example_contract(filename: &str) -> SolHarnessContract {
    let path = format!(
        "{}/../../../docs/examples/sol_harness_essence/library/{filename}",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let v: serde_json::Value = serde_json::from_str(&raw).expect("parse wrapper");
    let id = v["id"].as_str().expect("id").to_string();
    let summary = v["summary"].as_str().unwrap_or("").to_string();
    let program_json = v.get("program").expect("program").to_string();
    compile(&program_json).unwrap_or_else(|e| panic!("{id} compile: {e:?}"));
    SolHarnessContract::seed(id, summary, program_json)
}

fn out_text(bag: &SolValue) -> String {
    bag.as_map()
        .and_then(|m| m.get("out"))
        .and_then(|r| r.as_map())
        .and_then(|m| m.get("text"))
        .and_then(|v| match v {
            SolValue::Str(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Rung 4: parent harness invokes stored child by id; uses returned values.
#[test]
fn demo_two_checks_invokes_sum_ok() {
    const TENANT: &str = "demo-baby-r4";
    let child = load_example_contract("demo_sum_ok.json");
    let parent = load_example_contract("demo_two_checks.json");
    assert_eq!(child.id, "demo.sum_ok");
    assert_eq!(parent.id, "demo.two_checks");

    let mut store = MemoryStore::new();
    for c in [&child, &parent] {
        let put = store
            .put_if_absent(TENANT, CONTRACT_TABLE, &c.id, c.as_store_value())
            .expect("store");
        assert!(matches!(put, PutIfAbsent::Inserted { .. }));
    }

    let loaded_child = load_contract(&store, TENANT, "demo.sum_ok")
        .unwrap()
        .expect("child in store");
    let loaded_parent = load_contract(&store, TENANT, "demo.two_checks")
        .unwrap()
        .expect("parent in store");

    // Invoke registry resolves child id → program_json (composition building block).
    let mut programs = HashMap::new();
    programs.insert(loaded_child.id.clone(), loaded_child.program_json.clone());

    let mut registry = registry_with_harness_invoke(programs);
    let program = compile(&loaded_parent.program_json).expect("parent compiles");
    let mut inst = Instance::new(program, &mut registry);
    let bag0 = SolValue::map::<_, &str>([]);
    let (bag, hash) = match inst.start(bag0).expect("parent start") {
        TurnOutcome::Completed { bag, bag_hash } => (bag, bag_hash),
        TurnOutcome::Parked(_) => panic!("parent should complete"),
    };

    assert_eq!(out_text(&bag), "first=ok second=nope");
    assert_eq!(
        bag.as_map()
            .and_then(|m| m.get("first"))
            .and_then(|b| b.as_map())
            .and_then(|m| m.get("sum"))
            .cloned(),
        Some(SolValue::Int(5))
    );
    assert_eq!(
        bag.as_map()
            .and_then(|m| m.get("second"))
            .and_then(|b| b.as_map())
            .and_then(|m| m.get("sum"))
            .cloned(),
        Some(SolValue::Int(4))
    );
    assert!(!hash.is_empty());
}

/// Rung 5: Conductor Sol uses shared `conductor.decide@1` (scripted backend).
///
/// Sol does not keyword-match. Selection is the Call's structured out-map;
/// spawn harness_id must be in the catalog.
#[test]
fn demo_conductor_baby_spins_sum_ok_from_catalog() {
    const TENANT: &str = "demo-baby-r5";
    let child = load_example_contract("demo_sum_ok.json");
    let conductor = load_example_contract("demo_conductor_baby.json");
    assert_eq!(conductor.id, "demo.conductor_baby");

    let mut store = MemoryStore::new();
    for c in [&child, &conductor] {
        let put = store
            .put_if_absent(TENANT, CONTRACT_TABLE, &c.id, c.as_store_value())
            .expect("store");
        assert!(matches!(put, PutIfAbsent::Inserted { .. }));
    }

    let loaded_child = load_contract(&store, TENANT, "demo.sum_ok")
        .unwrap()
        .expect("child");
    let loaded_conductor = load_contract(&store, TENANT, "demo.conductor_baby")
        .unwrap()
        .expect("conductor");

    let catalog = SolValue::List(vec![
        SolValue::map([
            ("id", SolValue::str("demo.sum_ok")),
            ("summary", SolValue::str(&loaded_child.summary)),
        ]),
        SolValue::map([
            ("id", SolValue::str("demo.conductor_baby")),
            ("summary", SolValue::str(&loaded_conductor.summary)),
        ]),
    ]);

    let mut programs = HashMap::new();
    programs.insert(loaded_child.id.clone(), loaded_child.program_json.clone());

    let mut registry = registry_with_harness_invoke(programs);
    aelio_kernel::register_conductor_decide(&mut registry);

    let program = compile(&loaded_conductor.program_json).expect("conductor compiles");
    let mut inst = Instance::new(program, &mut registry);
    let bag_in = SolValue::map([
        ("utterance", SolValue::str("please check if 2 and 3 make five")),
        ("catalog", catalog),
    ]);
    let (bag, hash) = match inst.start(bag_in).expect("conductor start") {
        TurnOutcome::Completed { bag, bag_hash } => (bag, bag_hash),
        TurnOutcome::Parked(_) => panic!("conductor baby should complete"),
    };

    assert_eq!(
        bag.as_map()
            .and_then(|m| m.get("decision"))
            .and_then(|d| d.as_map())
            .and_then(|m| m.get("harness_id"))
            .cloned(),
        Some(SolValue::str("demo.sum_ok"))
    );
    assert_eq!(
        bag.as_map()
            .and_then(|m| m.get("decision"))
            .and_then(|d| d.as_map())
            .and_then(|m| m.get("source"))
            .cloned(),
        Some(SolValue::str("scripted"))
    );
    assert_eq!(out_text(&bag), "conductor spun demo.sum_ok → ok");
    assert_eq!(
        bag.as_map()
            .and_then(|m| m.get("child"))
            .and_then(|b| b.as_map())
            .and_then(|m| m.get("sum"))
            .cloned(),
        Some(SolValue::Int(5))
    );
    assert!(!hash.is_empty());

    // No-spawn path when catalog has no demo.sum_ok / utterance does not ask to check.
    let mut programs2 = HashMap::new();
    programs2.insert(loaded_child.id.clone(), loaded_child.program_json.clone());
    let mut registry2 = registry_with_harness_invoke(programs2);
    aelio_kernel::register_conductor_decide(&mut registry2);
    let program2 = compile(&loaded_conductor.program_json).unwrap();
    let mut inst2 = Instance::new(program2, &mut registry2);
    let bag_chat = SolValue::map([
        ("utterance", SolValue::str("tell me a long story about space travel")),
        ("catalog", SolValue::List(vec![])),
    ]);
    let (bag2, _) = match inst2.start(bag_chat).unwrap() {
        TurnOutcome::Completed { bag, bag_hash } => (bag, bag_hash),
        TurnOutcome::Parked(_) => panic!("expected complete"),
    };
    assert_eq!(out_text(&bag2), "Tell me more about what you need.");
}
