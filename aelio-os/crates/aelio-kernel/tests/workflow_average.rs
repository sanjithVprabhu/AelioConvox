//! Conformance: `workflow.average` pure Sol composition + bag_hash replay.
//!
//! Proves production-plan backlog items 6/9 for the sequential slice:
//! namespaced stdlib Calls + stored Sol program + deterministic replay.
//! True SpawnMany/join remains deferred (documented, not claimed).

use aelio_kernel::{
    compile, replay, registry_with_p0_stdlib, workflow_average_contract,
    workflow_average_program_json, ArtifactOriginV1, BudgetContractV1, DeterminismV1,
    EffectClassV1, HarnessContractV1, Instance, SuspendabilityV1, TurnOutcome,
};
use aelio_sol::{value_hash, SolValue};
use std::path::PathBuf;

fn bag_values(nums: &[i64]) -> SolValue {
    SolValue::map([(
        "values",
        SolValue::List(nums.iter().copied().map(SolValue::Int).collect()),
    )])
}

fn run_average(values: &[i64]) -> (SolValue, String, aelio_kernel::Ledger) {
    let program_json = workflow_average_program_json().to_string();
    let program = compile(&program_json).expect("workflow.average must compile + plan");
    let mut registry = registry_with_p0_stdlib();
    let mut inst = Instance::new(program, &mut registry);
    match inst.start(bag_values(values)).expect("start") {
        TurnOutcome::Completed { bag, bag_hash } => (bag, bag_hash, inst.ledger().clone()),
        TurnOutcome::Parked(_) => panic!("workflow.average must not park"),
    }
}

#[test]
fn workflow_average_success_and_replay() {
    let (bag, bag_hash, ledger) = run_average(&[2, 4, 6]);
    let average = bag
        .as_map()
        .and_then(|m| m.get("average"))
        .cloned()
        .expect("average field");
    assert_eq!(average, SolValue::Int(4));
    assert_eq!(bag_hash, value_hash(&bag));

    let program = compile(&workflow_average_program_json().to_string()).unwrap();
    let replayed = replay(&program, &ledger, bag_values(&[2, 4, 6]))
        .expect("replay must succeed");
    assert_eq!(replayed, bag_hash, "bag_hash identity on replay");
}

#[test]
fn workflow_average_empty_list_routes_to_error() {
    let (bag, _, _) = run_average(&[]);
    let err = bag
        .as_map()
        .and_then(|m| m.get("error"))
        .and_then(|v| v.as_map())
        .and_then(|m| m.get("text"))
        .cloned();
    assert_eq!(err, Some(SolValue::str("Math.EmptyInput")));
    assert!(
        bag.as_map().and_then(|m| m.get("average")).is_none(),
        "empty list must not produce average"
    );
}

#[test]
fn workflow_average_contract_seed_matches_program() {
    let c = workflow_average_contract();
    assert_eq!(c.id, "workflow.average");
    let seeded: serde_json::Value = serde_json::from_str(&c.program_json).unwrap();
    assert_eq!(seeded, workflow_average_program_json());
    compile(&c.program_json).expect("seed contract program must compile");
}

#[test]
fn on_disk_library_program_matches_seed_and_compiles() {
    let path = library_program_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let on_disk: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        on_disk,
        workflow_average_program_json(),
        "on-disk program.sol.json must match seed"
    );
    compile(&text).expect("on-disk program must compile");

    let contract_path = path
        .parent()
        .unwrap()
        .join("contract.json");
    let contract_text = std::fs::read_to_string(&contract_path).unwrap();
    let contract: HarnessContractV1 = serde_json::from_str(&contract_text).unwrap();
    contract.validate().expect("on-disk HarnessContractV1");
    assert_eq!(contract.id, "workflow.average");
    assert_eq!(contract.origin, ArtifactOriginV1::Vendor);
    assert_eq!(contract.effect, EffectClassV1::Pure);
    assert_eq!(contract.determinism, DeterminismV1::Deterministic);
    assert_eq!(contract.suspendability, SuspendabilityV1::Never);
    assert!(contract.budgets.calls.unwrap_or(0) >= 3);
    // program field on contract.json should match file
    assert_eq!(contract.program, on_disk);
    let hash = contract.content_hash();
    assert!(hash.starts_with("blake3:"));
    // re-hash stable
    assert_eq!(hash, contract.content_hash());
    let _ = BudgetContractV1::default();
}

#[test]
fn library_manifest_lists_average() {
    let manifest = library_root().join("manifest.json");
    let text = std::fs::read_to_string(&manifest).expect("manifest.json");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let harnesses = v["harnesses"].as_array().expect("harnesses array");
    assert!(
        harnesses.iter().any(|h| h["id"] == "workflow.average"),
        "manifest must list workflow.average"
    );
    assert!(
        harnesses.iter().any(|h| h["id"] == "conductor.root"),
        "manifest must list conductor.root"
    );
}

#[test]
fn average_via_process_tree_join_all() {
    use aelio_kernel::{workflow_average_via_join, ProcessTree};
    let mut tree = ProcessTree::new("tenant-avg", "user-1");
    let (_root, bag, hash) = workflow_average_via_join(&mut tree, &[10, 20, 30]).unwrap();
    assert_eq!(
        bag.as_map().and_then(|m| m.get("average")).cloned(),
        Some(SolValue::Int(20))
    );
    assert_eq!(hash, value_hash(&bag));
    assert_eq!(tree.instance_count(), 3);
}

#[test]
fn conductor_root_on_disk_matches_seed() {
    use aelio_kernel::conductor_root_program_json;
    let path = library_root().join("harnesses/conductor/root/1.0.0/program.sol.json");
    let text = std::fs::read_to_string(&path).expect("conductor program");
    let on_disk: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(on_disk, conductor_root_program_json());
    compile(&text).expect("conductor.root must compile");
}

fn library_root() -> PathBuf {
    // tests run with CWD = crate or workspace; resolve relative to this file's crate.
    let candidates = [
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../library"),
        PathBuf::from("library"),
        PathBuf::from("aelio-os/library"),
    ];
    for c in candidates {
        if c.join("manifest.json").exists() {
            return c;
        }
    }
    panic!("could not locate aelio-os/library");
}

fn library_program_path() -> PathBuf {
    library_root().join("harnesses/workflow/average/1.0.0/program.sol.json")
}
