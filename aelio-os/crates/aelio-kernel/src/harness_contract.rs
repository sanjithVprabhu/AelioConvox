//! Phase 1 — first-class Sol harness contracts (identity, signature, admission).
//!
//! Authority: `docs/architecture/HARNESS_OS_EXECUTION_PLAN.md` §Phase 1.
//! Identity uses Mother §4.3 canonical Sol + BLAKE3 (`aelio_sol::value_hash`), never ad-hoc SHA-256.

use crate::error::{ErrV1, ReasonCode};
use crate::instr::parse_node;
use crate::plan;
use crate::registry::{EffectClass, Registry};
use aelio_sol::{value_hash, SolValue};
use serde_json::Value as Json;

/// Bag path slot in a harness IO signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotSpec {
    /// Bag path, e.g. `in.phone`, `out.value`.
    pub path: String,
    /// Coarse type tag: `str` | `int` | `bool` | `any` | `map` | `list`.
    pub ty: String,
    pub required: bool,
}

impl SlotSpec {
    pub fn req(path: impl Into<String>, ty: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            ty: ty.into(),
            required: true,
        }
    }

    pub fn opt(path: impl Into<String>, ty: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            ty: ty.into(),
            required: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HarnessSignature {
    pub inputs: Vec<SlotSpec>,
    pub outputs: Vec<SlotSpec>,
    pub errors: Vec<String>,
}

/// Budget ceilings declared on a harness (enforced at admission / runtime).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetSpec {
    pub max_calls: u64,
    pub max_tokens: u64,
    pub max_ms: u64,
    pub max_depth: u64,
}

impl Default for BudgetSpec {
    fn default() -> Self {
        Self {
            max_calls: 64,
            max_tokens: 32_768,
            max_ms: 30_000,
            max_depth: 16,
        }
    }
}

impl BudgetSpec {
    pub fn within_global_ceiling(&self) -> Result<(), String> {
        const MAX_CALLS: u64 = 10_000;
        const MAX_TOKENS: u64 = 1_000_000;
        const MAX_MS: u64 = 600_000;
        const MAX_DEPTH: u64 = 64;
        if self.max_calls == 0 || self.max_calls > MAX_CALLS {
            return Err(format!("budget.max_calls out of range 1..={MAX_CALLS}"));
        }
        if self.max_tokens > MAX_TOKENS {
            return Err(format!("budget.max_tokens exceeds {MAX_TOKENS}"));
        }
        if self.max_ms == 0 || self.max_ms > MAX_MS {
            return Err(format!("budget.max_ms out of range 1..={MAX_MS}"));
        }
        if self.max_depth == 0 || self.max_depth > MAX_DEPTH {
            return Err(format!("budget.max_depth out of range 1..={MAX_DEPTH}"));
        }
        Ok(())
    }
}

/// Infer library namespace from id (`calc.average` → `calc`, `quick_reply` → `conv`).
pub fn library_from_id(id: &str) -> String {
    if let Some((lib, _)) = id.split_once('.') {
        return lib.to_string();
    }
    match id {
        "quick_reply" | "full_reply" | "apologize_closed" | "semantic_ack" => "conv".into(),
        "understand_intent" | "clarify_slot" | "detect_fresh_utterance" | "help_router" => {
            "intent".into()
        }
        "wait_for_user" | "confirm_then_act" => "control".into(),
        "memory_attach" | "memory_search" => "memory".into(),
        id if id.starts_with("calc_") || id.starts_with("list_") || id.starts_with("string_") => {
            "calc".into()
        }
        id if id.starts_with("stack_") || id.starts_with("proper_sol_") => "proc".into(),
        id if id.starts_with("workflow.") => "workflow".into(),
        _ => "core".into(),
    }
}

/// Mother §4.3 identity: BLAKE3 over canonical Sol of `{id, version, program}`.
pub fn contract_identity(id: &str, version: u32, program_json: &str) -> String {
    let program_sol = match serde_json::from_str::<Json>(program_json) {
        Ok(j) => crate::json_from(&j).unwrap_or_else(|_| SolValue::str(program_json)),
        Err(_) => SolValue::str(program_json),
    };
    let envelope = SolValue::map([
        ("id", SolValue::str(id)),
        ("version", SolValue::Int(version as i64)),
        ("program", program_sol),
    ]);
    value_hash(&envelope)
}

/// Collect `Call.id` strings from a Sol program JSON tree.
pub fn collect_call_ids(program_json: &str) -> Result<Vec<String>, String> {
    let v: Json = serde_json::from_str(program_json).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    walk_calls(&v, &mut out);
    out.sort();
    out.dedup();
    Ok(out)
}

fn walk_calls(v: &Json, out: &mut Vec<String>) {
    match v {
        Json::Object(map) => {
            if map.get("op").and_then(|o| o.as_str()) == Some("Call") {
                if let Some(id) = map.get("id").and_then(|i| i.as_str()) {
                    out.push(id.to_string());
                }
            }
            for val in map.values() {
                walk_calls(val, out);
            }
        }
        Json::Array(items) => {
            for item in items {
                walk_calls(item, out);
            }
        }
        _ => {}
    }
}

/// Infer max effect from registered Call targets (pure < read < write < external).
pub fn infer_effect_from_calls(call_ids: &[String], registry: &Registry) -> EffectClass {
    let mut rank = 0u8; // 0 pure, 1 read, 2 write, 3 external
    for id in call_ids {
        let r = match registry.effect_of(id) {
            Some(EffectClass::Pure) => 0,
            Some(EffectClass::Read) => 1,
            Some(EffectClass::Write) => 2,
            Some(EffectClass::External) => 3,
            None => continue,
        };
        rank = rank.max(r);
    }
    match rank {
        0 => EffectClass::Pure,
        1 => EffectClass::Read,
        2 => EffectClass::Write,
        _ => EffectClass::External,
    }
}

fn effect_rank(e: EffectClass) -> u8 {
    match e {
        EffectClass::Pure => 0,
        EffectClass::Read => 1,
        EffectClass::Write => 2,
        EffectClass::External => 3,
    }
}

/// Serialize [`EffectClass`] for durable store rows.
pub fn effect_to_str(e: EffectClass) -> &'static str {
    match e {
        EffectClass::Pure => "pure",
        EffectClass::Read => "read",
        EffectClass::Write => "write",
        EffectClass::External => "external",
    }
}

/// Parse effect from store / authoring text.
pub fn effect_from_str(s: &str) -> Option<EffectClass> {
    match s {
        "pure" => Some(EffectClass::Pure),
        "read" => Some(EffectClass::Read),
        "write" => Some(EffectClass::Write),
        "external" => Some(EffectClass::External),
        _ => None,
    }
}

/// Admission validation (Phase 1.3).
///
/// When `registry` is `Some`, every `Call` id must be registered, the call graph must be acyclic
/// for the given tenant, and `declared_effect` must not be weaker than the max effect of those
/// calls. When `None`, only compile (plan) + budget + identity sealing run.
pub fn admit_contract(
    id: &str,
    version: u32,
    program_json: &str,
    declared_effect: EffectClass,
    budget: &BudgetSpec,
    registry: Option<&Registry>,
    tenant: &str,
) -> Result<String, ErrV1> {
    if id.trim().is_empty() {
        return Err(ErrV1::new(ReasonCode::Shape, "admit", "id must be non-empty"));
    }
    if version == 0 {
        return Err(ErrV1::new(ReasonCode::Shape, "admit", "version must be >= 1"));
    }
    budget
        .within_global_ceiling()
        .map_err(|e| ErrV1::new(ReasonCode::BudgetCalls, "admit", e))?;

    let node = parse_node(
        &serde_json::from_str(program_json)
            .map_err(|e| ErrV1::new(ReasonCode::Shape, "admit", format!("invalid JSON: {e}")))?,
    )?;
    // Syntax / structural plan always required.
    plan::plan(&node)?;

    if let Some(reg) = registry {
        let t = if tenant.trim().is_empty() {
            "vendor"
        } else {
            tenant
        };
        // Flow-edge acyclicity when the registry declares edges (no-op if empty).
        reg.validate_call_graph(t)
            .map_err(|e| ErrV1::new(ReasonCode::Policy, "admit", e))?;

        let calls = collect_call_ids(program_json)
            .map_err(|e| ErrV1::new(ReasonCode::Shape, "admit", e))?;
        for call_id in &calls {
            // `effect_of` is true for both demo `register` stubs and production `register_declared`.
            if reg.effect_of(call_id).is_none() {
                return Err(ErrV1::new(
                    ReasonCode::Policy,
                    "admit",
                    format!("unregistered Call target `{call_id}`"),
                ));
            }
        }
        let inferred = infer_effect_from_calls(&calls, reg);
        if effect_rank(declared_effect) < effect_rank(inferred) {
            return Err(ErrV1::new(
                ReasonCode::Policy,
                "admit",
                format!(
                    "declared effect {declared_effect:?} weaker than calls require {inferred:?}"
                ),
            ));
        }
    }

    Ok(contract_identity(id, version, program_json))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Boundedness;
    use crate::registry::{Declaration, Origin, TargetClass};

    #[test]
    fn identity_stable_and_blake3_hex() {
        let prog = r#"{"nid":"root","op":"Const","v":{"x":1}}"#;
        let a = contract_identity("calc.demo", 1, prog);
        let b = contract_identity("calc.demo", 1, prog);
        assert_eq!(a, b);
        assert_eq!(a.len(), 64); // blake3 hex
        let c = contract_identity("calc.demo", 2, prog);
        assert_ne!(a, c);
    }

    #[test]
    fn pure_cannot_declare_weaker_than_external_call() {
        let mut reg = Registry::default();
        let decl = Declaration {
            id: "tool.x@1".into(),
            class: TargetClass::Tool,
            input_imprint: "aelio.unit@1".into(),
            output_imprint: "aelio.unit@1".into(),
            boundedness: Boundedness::DeadlineCompliant { max_ms: 1000 },
            effect_class: EffectClass::External,
            policy_tags: vec!["t".into()],
            tenant: "vendor".into(),
            origin: Origin::Vendor,
        };
        reg.register_declared(decl, |_| Ok(SolValue::Null)).unwrap();
        let prog = r#"{"nid":"root","op":"Call","id":"tool.x@1","args":{},"into":"o"}"#;
        let err = admit_contract(
            "t.x",
            1,
            prog,
            EffectClass::Pure,
            &BudgetSpec::default(),
            Some(&reg),
            "vendor",
        )
        .unwrap_err();
        assert!(err.detail.contains("weaker") || err.detail.contains("External"));
    }

    #[test]
    fn admit_without_registry_still_compiles_and_seals() {
        let prog = r#"{"nid":"root","op":"Const","v":{"ok":true}}"#;
        let id = admit_contract(
            "pure.const",
            1,
            prog,
            EffectClass::Pure,
            &BudgetSpec::default(),
            None,
            "vendor",
        )
        .unwrap();
        assert_eq!(id, contract_identity("pure.const", 1, prog));
    }

    #[test]
    fn collect_call_ids_nested() {
        let prog = r#"{
          "nid":"root","op":"Seq","steps":[
            {"nid":"a","op":"Call","id":"express.say@1","args":{},"into":"x"},
            {"nid":"b","op":"Branch","pred":{"fn":"eq","args":[{"lit":1},{"lit":1}]},
              "then":{"nid":"t","op":"Call","id":"memory.search@1","args":{},"into":"y"},
              "else":{"nid":"e","op":"Const","v":{}}}
          ]}"#;
        let ids = collect_call_ids(prog).unwrap();
        assert!(ids.contains(&"express.say@1".into()));
        assert!(ids.contains(&"memory.search@1".into()));
    }
}
