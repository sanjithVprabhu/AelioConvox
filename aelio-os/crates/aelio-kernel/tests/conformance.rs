//! Conformance vector runner (F11 / handoff Phase 2).
//! Reads `docs/vectors/*.json` and asserts expected bag_hash / err / plan-time reject.

use aelio_kernel::{
    compile, replay, EffectClass, ErrV1, Instance, ReasonCode, Registry, TurnOutcome,
};
use aelio_sol::{value_hash, SolValue};
use serde_json::Value as J;
use std::fs;
use std::path::PathBuf;

fn vectors_dir() -> PathBuf {
    // aelio-os/crates/aelio-kernel/tests → repo docs/vectors
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/vectors")
        .canonicalize()
        .expect("docs/vectors must exist")
}

fn load_bag(j: &J) -> SolValue {
    aelio_kernel::json_from(j).expect("initial_bag")
}

fn load_registry(vector: &J) -> Registry {
    let mut registry = Registry::default();
    for entry in vector
        .get("registry")
        .and_then(J::as_array)
        .into_iter()
        .flatten()
    {
        let id = entry["id"].as_str().expect("registry.id").to_string();
        let effect = match entry["effect"].as_str().expect("registry.effect") {
            "pure" => EffectClass::Pure,
            "read" => EffectClass::Read,
            "write" => EffectClass::Write,
            "external" => EffectClass::External,
            other => panic!("bad registry effect {other}"),
        };
        if let Some(result) = entry.get("result") {
            let result = load_bag(result);
            registry.register(id, effect, move |_| Ok(result.clone()));
        } else {
            let code = ReasonCode::from_code(
                entry["error"]["code"]
                    .as_str()
                    .expect("registry.error.code"),
            )
            .expect("known ReasonCode");
            registry.register(id, effect, move |_| {
                Err(ErrV1::new(
                    code.clone(),
                    "vector_target",
                    "injected failure",
                ))
            });
        }
    }
    registry
}

fn run_vector(path: &std::path::Path) {
    let text = fs::read_to_string(path).unwrap();
    let v: J = serde_json::from_str(&text).unwrap();
    let name = v["name"].as_str().unwrap_or("?");
    let plan = v["plan"].clone();
    let plan_text = serde_json::to_string(&plan).unwrap();
    let expected = &v["expected"];
    let kind = expected["kind"].as_str().unwrap();
    let plan_time_only = v
        .get("plan_time_only")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);

    match kind {
        "err" if plan_time_only || expected["code"].as_str() == Some("Policy") => {
            let err = compile(&plan_text);
            match err {
                Err(e) => {
                    let want = expected["code"].as_str().unwrap();
                    assert_eq!(e.code.code(), want, "{name}: plan-time code");
                }
                Ok(_) if plan_time_only => panic!("{name}: expected plan-time reject"),
                Ok(program) => {
                    // Runtime error path.
                    let bag = load_bag(
                        v.get("initial_bag")
                            .unwrap_or(&J::Object(Default::default())),
                    );
                    let mut reg = load_registry(&v);
                    let mut inst = Instance::new(program, &mut reg);
                    match inst.start(bag) {
                        Err(e) => {
                            assert_eq!(e.code.code(), expected["code"].as_str().unwrap(), "{name}")
                        }
                        Ok(_) => panic!("{name}: expected runtime err"),
                    }
                }
            }
        }
        "err" => {
            let program = match compile(&plan_text) {
                Ok(p) => p,
                Err(e) => {
                    assert_eq!(e.code.code(), expected["code"].as_str().unwrap(), "{name}");
                    return;
                }
            };
            let bag = load_bag(
                v.get("initial_bag")
                    .unwrap_or(&J::Object(Default::default())),
            );
            let mut reg = load_registry(&v);
            let mut inst = Instance::new(program, &mut reg);
            match inst.start(bag) {
                Err(e) => assert_eq!(e.code.code(), expected["code"].as_str().unwrap(), "{name}"),
                Ok(_) => panic!("{name} expected err"),
            }
        }
        "bag_hash" => {
            let program =
                compile(&plan_text).unwrap_or_else(|e| panic!("{name}: compile {}", e.detail));
            let bag = load_bag(
                v.get("initial_bag")
                    .unwrap_or(&J::Object(Default::default())),
            );
            let expected_bag = load_bag(&expected["bag"]);
            let want = value_hash(&expected_bag);
            let mut reg = load_registry(&v);
            let mut inst = Instance::new(program.clone(), &mut reg);
            let (got, ledger) = match inst.start(bag.clone()).unwrap() {
                TurnOutcome::Completed { bag_hash, .. } => (bag_hash, inst.ledger().clone()),
                TurnOutcome::Parked(p) => panic!("{name}: parked at {}", p.park_nid),
            };
            assert_eq!(got, want, "{name}: bag_hash");
            if v.get("replay_twice")
                .and_then(|x| x.as_bool())
                .unwrap_or(false)
            {
                let again = replay(&program, &ledger, bag).unwrap();
                assert_eq!(again, want, "{name}: replay");
            }
        }
        "park" => {
            let program = compile(&plan_text).expect("park vector compiles");
            let initial = load_bag(
                v.get("initial_bag")
                    .unwrap_or(&J::Object(Default::default())),
            );
            let mut registry = load_registry(&v);
            let mut instance = Instance::new(program, &mut registry);
            let parked = match instance.start(initial).expect("park vector starts") {
                TurnOutcome::Parked(parked) => parked,
                TurnOutcome::Completed { .. } => panic!("{name}: expected park"),
            };
            assert_eq!(
                parked.park_nid,
                expected["park_nid"].as_str().expect("park_nid"),
                "{name}"
            );
            if let Some(expected_bag) = expected.get("bag") {
                assert_eq!(
                    value_hash(&parked.bag),
                    value_hash(&load_bag(expected_bag)),
                    "{name}: parked bag"
                );
            }
            if let Some(resume) = expected.get("resume") {
                let wake = load_bag(resume.get("wake").unwrap_or(&J::Null));
                let expected_bag = load_bag(&resume["bag"]);
                match instance.resume(parked, wake).expect("park vector resumes") {
                    TurnOutcome::Completed { bag_hash, .. } => {
                        assert_eq!(bag_hash, value_hash(&expected_bag), "{name}: resume bag")
                    }
                    TurnOutcome::Parked(_) => panic!("{name}: expected completion after resume"),
                }
            }
        }
        other => panic!("{name}: unknown expected.kind {other}"),
    }
}

#[test]
fn all_conformance_vectors() {
    let dir = vectors_dir();
    let mut files: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no vectors in {}", dir.display());
    for f in files {
        run_vector(&f);
    }
}

#[test]
fn property_replay_determinism_const_seq() {
    // §27 property: execute → ledger → replay → bag_hash equality.
    let plan = r#"{"nid":"s","op":"Seq","steps":[
        {"nid":"c","op":"Const","v":{"k":1,"f":2.0}},
        {"nid":"i","op":"Identity"}
    ]}"#;
    let program = compile(plan).unwrap();
    let bag = SolValue::map::<_, &str>([]);
    let mut reg = Registry::default();
    let mut inst = Instance::new(program.clone(), &mut reg);
    let (h, ledger) = match inst.start(bag.clone()).unwrap() {
        TurnOutcome::Completed { bag_hash, .. } => (bag_hash, inst.ledger().clone()),
        _ => panic!("must complete"),
    };
    let h2 = replay(&program, &ledger, bag).unwrap();
    assert_eq!(h, h2);
    ledger.verify_chain().unwrap();
}

#[test]
fn property_canonical_hash_key_order_independent() {
    let a = SolValue::map([("z", SolValue::Int(1)), ("a", SolValue::Int(2))]);
    let b = SolValue::map([("a", SolValue::Int(2)), ("z", SolValue::Int(1))]);
    assert_eq!(value_hash(&a), value_hash(&b));
    assert_ne!(
        value_hash(&SolValue::Int(2)),
        value_hash(&SolValue::float(2.0).unwrap())
    );
}
