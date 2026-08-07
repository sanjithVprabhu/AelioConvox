//! Run the baby Conductor Sol program end-to-end (decide → spawn/reply).
//!
//! Used by kernel tests and live Path B cutover (`/v1/turns`).
//! Opt out with `AELIO_CONDUCTOR_SOL=0`.

use crate::compile;
use crate::conductor_decide::{
    register_conductor_decide, register_conductor_decide_model, DecideArgs, ModelVerdict,
    DECIDE_CALL_ID,
};
use crate::driver::{Instance, TurnOutcome};
use crate::error::{ErrV1, ReasonCode};
use crate::sol_harness_lib::{registry_with_harness_invoke, SolHarnessContract, CONTRACT_TABLE};
use aelio_sol::SolValue;
use aelio_store::{PutIfAbsent, Store};
use std::collections::HashMap;
use std::sync::Arc;

/// How `conductor.decide@1` is backed for this turn.
#[derive(Clone)]
pub enum DecideMode {
    /// Built-in scripted heuristics (`source=scripted`).
    Scripted,
    /// Injected model classifier (`source=model`).
    Model(Arc<dyn Fn(&DecideArgs) -> Result<ModelVerdict, ErrV1> + Send + Sync>),
}

impl std::fmt::Debug for DecideMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecideMode::Scripted => write!(f, "Scripted"),
            DecideMode::Model(_) => write!(f, "Model(..)"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConductorSolTurnResult {
    pub reply_text: String,
    pub bag: SolValue,
    pub bag_hash: String,
    pub decision_kind: String,
    pub decision_source: String,
    pub spawned_harness_id: Option<String>,
}

fn bag_path_text(bag: &SolValue, root: &str) -> Option<String> {
    bag.as_map()
        .and_then(|m| m.get(root))
        .and_then(|r| r.as_map())
        .and_then(|m| m.get("text"))
        .and_then(|v| match v {
            SolValue::Str(s) => Some(s.clone()),
            _ => None,
        })
}

fn decision_field(bag: &SolValue, key: &str) -> Option<String> {
    bag.as_map()
        .and_then(|m| m.get("decision"))
        .and_then(|d| d.as_map())
        .and_then(|m| m.get(key))
        .and_then(|v| match v {
            SolValue::Str(s) => Some(s.clone()),
            _ => None,
        })
}

/// Embedded demo contracts (Rungs 1–5 teaching ladder).
pub fn demo_sum_ok_contract() -> SolHarnessContract {
    let raw = include_str!("../../../../docs/examples/sol_harness_essence/library/demo_sum_ok.json");
    parse_example_contract(raw)
}

pub fn demo_conductor_baby_contract() -> SolHarnessContract {
    let raw =
        include_str!("../../../../docs/examples/sol_harness_essence/library/demo_conductor_baby.json");
    parse_example_contract(raw)
}

fn parse_example_contract(raw: &str) -> SolHarnessContract {
    let v: serde_json::Value = serde_json::from_str(raw).expect("demo json");
    let id = v["id"].as_str().expect("id").to_string();
    let summary = v["summary"].as_str().unwrap_or("").to_string();
    let program_json = v.get("program").expect("program").to_string();
    compile(&program_json).unwrap_or_else(|e| panic!("{id} must compile: {e:?}"));
    SolHarnessContract::seed(id, summary, program_json)
}

/// Ensure demo.sum_ok + demo.conductor_baby are in the tenant contract table.
pub fn ensure_demo_conductor_library(
    store: &mut dyn Store,
    tenant: &str,
) -> Result<(), ErrV1> {
    for c in [demo_sum_ok_contract(), demo_conductor_baby_contract()] {
        match store
            .put_if_absent(tenant, CONTRACT_TABLE, &c.id, c.as_store_value())
            .map_err(|e| ErrV1::new(ReasonCode::Internal, "store", format!("{e:?}")))?
        {
            PutIfAbsent::Inserted { .. } | PutIfAbsent::Existing(_) => {}
        }
    }
    Ok(())
}

/// Catalog entries the baby Conductor Sol turn can spawn / describe.
pub fn demo_conductor_catalog() -> SolValue {
    let child = demo_sum_ok_contract();
    let cond = demo_conductor_baby_contract();
    SolValue::List(vec![
        SolValue::map([
            ("id", SolValue::str(&child.id)),
            ("summary", SolValue::str(&child.summary)),
        ]),
        SolValue::map([
            ("id", SolValue::str(&cond.id)),
            ("summary", SolValue::str(&cond.summary)),
        ]),
    ])
}

/// Run `demo.conductor_baby` with decide + harness.invoke for `demo.sum_ok`.
pub fn run_conductor_baby_turn(
    utterance: &str,
    mode: DecideMode,
) -> Result<ConductorSolTurnResult, ErrV1> {
    let child = demo_sum_ok_contract();
    let conductor = demo_conductor_baby_contract();
    let catalog = demo_conductor_catalog();

    let mut programs = HashMap::new();
    programs.insert(child.id.clone(), child.program_json.clone());

    let mut registry = registry_with_harness_invoke(programs);
    match mode {
        DecideMode::Scripted => register_conductor_decide(&mut registry),
        DecideMode::Model(classify) => register_conductor_decide_model(&mut registry, move |a| {
            classify(a)
        }),
    }

    let program = compile(&conductor.program_json)?;
    let mut inst = Instance::new(program, &mut registry);
    let bag_in = SolValue::map([
        ("utterance", SolValue::str(utterance)),
        ("catalog", catalog),
    ]);
    let (bag, bag_hash) = match inst.start(bag_in)? {
        TurnOutcome::Completed { bag, bag_hash } => (bag, bag_hash),
        TurnOutcome::Parked(_) => {
            return Err(ErrV1::new(
                ReasonCode::Internal,
                "conductor.sol",
                "demo.conductor_baby should not park",
            ))
        }
    };

    let reply_text = bag_path_text(&bag, "out").unwrap_or_default();
    let decision_kind = decision_field(&bag, "kind").unwrap_or_default();
    let decision_source = decision_field(&bag, "source").unwrap_or_default();
    let spawned_harness_id = decision_field(&bag, "harness_id");

    let _ = DECIDE_CALL_ID; // keep call id linked in docs/search
    Ok(ConductorSolTurnResult {
        reply_text,
        bag,
        bag_hash,
        decision_kind,
        decision_source,
        spawned_harness_id,
    })
}

/// Whether the live API should run Conductor Sol for this utterance.
///
/// Path B default: **always** (keyword Conductor is retired as authority).
/// Opt out only via legacy spine (`AELIO_AGENT_LEGACY=1` / parity AppState).
/// Set `AELIO_CONDUCTOR_SOL=0` to disable the Sol cutover while debugging.
pub fn should_cutover_conductor_sol(_utterance: &str) -> bool {
    match std::env::var("AELIO_CONDUCTOR_SOL") {
        Ok(v) if v == "0" || v.eq_ignore_ascii_case("false") => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conductor_decide::{KIND_SPAWN, ModelVerdict};

    #[test]
    fn scripted_turn_spawns_sum_ok() {
        let r = run_conductor_baby_turn(
            "please check if 2 and 3 make five",
            DecideMode::Scripted,
        )
        .unwrap();
        assert_eq!(r.decision_kind, KIND_SPAWN);
        assert_eq!(r.decision_source, "scripted");
        assert_eq!(r.spawned_harness_id.as_deref(), Some("demo.sum_ok"));
        assert_eq!(r.reply_text, "conductor spun demo.sum_ok → ok");
    }

    #[test]
    fn model_turn_sets_source_model() {
        let classify = Arc::new(|_a: &DecideArgs| {
            Ok(ModelVerdict {
                kind: KIND_SPAWN.into(),
                harness_id: Some("demo.sum_ok".into()),
                confidence: 0.88,
                x: Some(2),
                y: Some(3),
                reply_text: None,
            })
        });
        let r = run_conductor_baby_turn("anything", DecideMode::Model(classify)).unwrap();
        assert_eq!(r.decision_source, "model");
        assert_eq!(r.reply_text, "conductor spun demo.sum_ok → ok");
    }
}
