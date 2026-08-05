//! Sol harness contract library (Path B — Sol-only bodies).
//!
//! Each entry is a named harness body stored as App E Sol JSON — sugar names map to Sol ops.
//! Persist via [`store_library`] into table [`CONTRACT_TABLE`]; retrieve with [`load_contract`].
//!
//! Phase 1 contract model: version, library, signature, effect, budget, BLAKE3 identity
//! (`docs/architecture/HARNESS_OS_EXECUTION_PLAN.md` §Phase 1).

use crate::harness_contract::{
    admit_contract, collect_call_ids, contract_identity, effect_from_str, effect_to_str,
    infer_effect_from_calls, library_from_id, BudgetSpec, HarnessSignature, SlotSpec,
};
use crate::registry::EffectClass;
use aelio_sol::SolValue;
use aelio_store::{PutIfAbsent, Store, StoreError};
use serde_json::json;

/// Store table for Sol harness contracts (tenant-scoped).
pub const CONTRACT_TABLE: &str = "sol_harness_contracts";

/// One durable Sol harness contract (Phase 1 first-class artifact).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolHarnessContract {
    /// Namespaced id, e.g. `calc.average` or legacy `quick_reply`.
    pub id: String,
    pub version: u32,
    /// Selection-facing description.
    pub summary: String,
    /// Library namespace: `calc` | `control` | `memory` | `conv` | …
    pub library: String,
    /// App E instruction tree as JSON text (compile with [`crate::compile`]).
    pub program_json: String,
    pub signature: HarnessSignature,
    pub effect: EffectClass,
    pub budget: BudgetSpec,
    /// Mother §4.3 canonical Sol + BLAKE3 identity (hex).
    pub identity: String,
}

impl SolHarnessContract {
    /// Build a seed contract: version 1, library from id, default budget/signature,
    /// effect inferred from Call targets when possible (else `External`), sealed identity.
    pub fn seed(
        id: impl Into<String>,
        summary: impl Into<String>,
        program_json: impl Into<String>,
    ) -> Self {
        let id = id.into();
        let summary = summary.into();
        let program_json = program_json.into();
        let version = 1u32;
        let library = library_from_id(&id);
        let budget = BudgetSpec::default();
        let identity = contract_identity(&id, version, &program_json);
        let effect = match collect_call_ids(&program_json) {
            Ok(calls) if calls.is_empty() => EffectClass::Pure,
            Ok(calls) => {
                // Lightweight inference without a live Registry (seed time).
                rough_effect_from_call_ids(&calls)
            }
            Err(_) => EffectClass::External,
        };
        Self {
            id,
            version,
            summary,
            library,
            program_json,
            signature: HarnessSignature::default(),
            effect,
            budget,
            identity,
        }
    }

    /// Seed with explicit IO signature slots.
    pub fn seed_signed(
        id: impl Into<String>,
        summary: impl Into<String>,
        program_json: impl Into<String>,
        signature: HarnessSignature,
    ) -> Self {
        let mut c = Self::seed(id, summary, program_json);
        c.signature = signature;
        c
    }

    /// Re-seal identity after any field that participates in the hash changes.
    pub fn reseal_identity(&mut self) {
        self.identity = contract_identity(&self.id, self.version, &self.program_json);
    }

    /// Admit (compile + optional registry checks) and return sealed identity.
    pub fn admit(
        &self,
        registry: Option<&crate::registry::Registry>,
        tenant: &str,
    ) -> Result<String, crate::error::ErrV1> {
        admit_contract(
            &self.id,
            self.version,
            &self.program_json,
            self.effect,
            &self.budget,
            registry,
            tenant,
        )
    }

    pub fn as_store_value(&self) -> SolValue {
        SolValue::map([
            ("id", SolValue::str(&self.id)),
            ("kind", SolValue::str("sol_harness")),
            ("version", SolValue::Int(self.version as i64)),
            ("summary", SolValue::str(&self.summary)),
            ("library", SolValue::str(&self.library)),
            ("program_json", SolValue::str(&self.program_json)),
            ("effect", SolValue::str(effect_to_str(self.effect))),
            ("identity", SolValue::str(&self.identity)),
            ("budget", budget_to_sol(&self.budget)),
            ("signature", signature_to_sol(&self.signature)),
        ])
    }

    pub fn from_store_value(value: &SolValue) -> Result<Self, String> {
        let map = value.as_map().ok_or("contract must be a map")?;
        let id = map
            .get("id")
            .and_then(|v| match v {
                SolValue::Str(s) => Some(s.clone()),
                _ => None,
            })
            .ok_or("missing id")?;
        let summary = map
            .get("summary")
            .and_then(|v| match v {
                SolValue::Str(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let program_json = map
            .get("program_json")
            .and_then(|v| match v {
                SolValue::Str(s) => Some(s.clone()),
                _ => None,
            })
            .ok_or("missing program_json")?;
        let version = map
            .get("version")
            .and_then(|v| match v {
                SolValue::Int(n) if *n > 0 => Some(*n as u32),
                _ => None,
            })
            .unwrap_or(1);
        let library = map
            .get("library")
            .and_then(|v| match v {
                SolValue::Str(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_else(|| library_from_id(&id));
        let effect = map
            .get("effect")
            .and_then(|v| match v {
                SolValue::Str(s) => effect_from_str(s),
                _ => None,
            })
            .unwrap_or(EffectClass::External);
        let budget = map
            .get("budget")
            .map(budget_from_sol)
            .unwrap_or_else(BudgetSpec::default);
        let signature = map
            .get("signature")
            .map(signature_from_sol)
            .unwrap_or_default();
        let identity = map
            .get("identity")
            .and_then(|v| match v {
                SolValue::Str(s) if !s.is_empty() => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_else(|| contract_identity(&id, version, &program_json));
        Ok(Self {
            id,
            version,
            summary,
            library,
            program_json,
            signature,
            effect,
            budget,
            identity,
        })
    }
}

/// Coarse effect inference used at seed construction (no live Registry).
fn rough_effect_from_call_ids(calls: &[String]) -> EffectClass {
    let mut rank = 0u8;
    for id in calls {
        let r = if id.starts_with("tool.") || id.contains("act_stub") {
            3
        } else if id.contains("attach") || id.starts_with("page.") {
            2
        } else if id.starts_with("express.")
            || id.starts_with("memory.")
            || id.starts_with("understand.")
            || id.starts_with("harness.")
            || id.starts_with("flow.")
            || id.starts_with("context.")
        {
            1
        } else {
            // math.*, collection.*, compute.hold@1, …
            0
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

fn budget_to_sol(b: &BudgetSpec) -> SolValue {
    SolValue::map([
        ("max_calls", SolValue::Int(b.max_calls as i64)),
        ("max_tokens", SolValue::Int(b.max_tokens as i64)),
        ("max_ms", SolValue::Int(b.max_ms as i64)),
        ("max_depth", SolValue::Int(b.max_depth as i64)),
    ])
}

fn budget_from_sol(v: &SolValue) -> BudgetSpec {
    let d = BudgetSpec::default();
    let Some(m) = v.as_map() else {
        return d;
    };
    let get = |k: &str, fallback: u64| {
        m.get(k)
            .and_then(|x| match x {
                SolValue::Int(n) if *n > 0 => Some(*n as u64),
                _ => None,
            })
            .unwrap_or(fallback)
    };
    BudgetSpec {
        max_calls: get("max_calls", d.max_calls),
        max_tokens: get("max_tokens", d.max_tokens),
        max_ms: get("max_ms", d.max_ms),
        max_depth: get("max_depth", d.max_depth),
    }
}

fn signature_to_sol(s: &HarnessSignature) -> SolValue {
    SolValue::map([
        (
            "inputs",
            SolValue::List(s.inputs.iter().map(slot_to_sol).collect()),
        ),
        (
            "outputs",
            SolValue::List(s.outputs.iter().map(slot_to_sol).collect()),
        ),
        (
            "errors",
            SolValue::List(s.errors.iter().map(|e| SolValue::str(e)).collect()),
        ),
    ])
}

fn slot_to_sol(s: &SlotSpec) -> SolValue {
    SolValue::map([
        ("path", SolValue::str(&s.path)),
        ("ty", SolValue::str(&s.ty)),
        ("required", SolValue::Bool(s.required)),
    ])
}

fn signature_from_sol(v: &SolValue) -> HarnessSignature {
    let Some(m) = v.as_map() else {
        return HarnessSignature::default();
    };
    let slots = |key: &str| -> Vec<SlotSpec> {
        m.get(key)
            .and_then(|x| x.as_list())
            .map(|list| {
                list.iter()
                    .filter_map(|item| {
                        let sm = item.as_map()?;
                        let path = sm.get("path").and_then(|p| match p {
                            SolValue::Str(s) => Some(s.clone()),
                            _ => None,
                        })?;
                        let ty = sm
                            .get("ty")
                            .and_then(|p| match p {
                                SolValue::Str(s) => Some(s.clone()),
                                _ => None,
                            })
                            .unwrap_or_else(|| "any".into());
                        let required = sm
                            .get("required")
                            .and_then(|p| match p {
                                SolValue::Bool(b) => Some(*b),
                                _ => None,
                            })
                            .unwrap_or(true);
                        Some(SlotSpec { path, ty, required })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let errors = m
        .get("errors")
        .and_then(|x| x.as_list())
        .map(|list| {
            list.iter()
                .filter_map(|e| match e {
                    SolValue::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    HarnessSignature {
        inputs: slots("inputs"),
        outputs: slots("outputs"),
        errors,
    }
}

/// Full OS seed library: starters + calc/semantic + expanded control demos.
pub fn sol_harness_library() -> Vec<SolHarnessContract> {
    vec![
        // Starter conversation set (Conductor-facing)
        quick_reply_contract(),
        understand_intent_contract(),
        wait_for_user_contract(),
        memory_attach_contract(),
        full_reply_contract(),
        apologize_closed_contract(),
        // Calc / list / string
        calc_sum_gate_contract(),
        calc_repeat_add_contract(),
        calc_mul_div_contract(),
        list_filter_keep_contract(),
        string_contains_gate_contract(),
        // Semantic / control
        semantic_ack_contract(),
        confirm_then_act_contract(),
        detect_fresh_utterance_contract(),
        greet_then_offer_help_contract(),
        parent_waits_on_child_contract(),
        // Combinations (compose existing harnesses / patterns)
        memory_then_full_reply_contract(),
        greet_then_quick_reply_contract(),
        understand_then_memory_contract(),
        help_router_contract(),
        intent_then_calc_contract(),
        greet_memory_pipeline_contract(),
        fresh_then_greet_contract(),
        calc_then_report_pipeline_contract(),
        clarify_slot_contract(),
        memory_search_only_contract(),
        // Nested stack: A waits on B waits on C
        stack_leaf_c_contract(),
        stack_mid_b_contract(),
        stack_top_a_contract(),
        // Proper Mother Sol (registered-flow Call ids, not harness.invoke)
        proper_sol_stack_leaf_c_contract(),
        proper_sol_stack_mid_b_contract(),
        proper_sol_stack_top_a_contract(),
        // Sol control coverage demos
        fallback_say_contract(),
        guard_required_field_contract(),
        once_external_stub_contract(),
        budgeted_express_contract(),
        // Production plan P0 reference compound (sequential pure composition)
        workflow_average_contract(),
    ]
}

/// Insert every library contract with `put_if_absent` (existing ids win — safe re-seed).
///
/// Each contract is admitted (compile + budget + identity seal) before write. Full Call-registry
/// checks use [`store_library_admitted`] when a registry is available.
pub fn store_library(
    store: &mut dyn Store,
    tenant: &str,
) -> Result<Vec<(String, PutIfAbsent)>, StoreError> {
    store_library_admitted(store, tenant, None)
}

/// Like [`store_library`], but when `registry` is provided every Call id must be registered and
/// declared effects must not be weaker than the call graph requires.
pub fn store_library_admitted(
    store: &mut dyn Store,
    tenant: &str,
    registry: Option<&crate::registry::Registry>,
) -> Result<Vec<(String, PutIfAbsent)>, StoreError> {
    let mut out = Vec::new();
    for mut c in sol_harness_library() {
        let identity = c
            .admit(registry, tenant)
            .map_err(|e| StoreError::Internal(format!("admit {}: {}", c.id, e.detail)))?;
        c.identity = identity;
        if let Some(reg) = registry {
            if let Ok(calls) = collect_call_ids(&c.program_json) {
                c.effect = infer_effect_from_calls(&calls, reg);
            }
        }
        let result = store.put_if_absent(tenant, CONTRACT_TABLE, &c.id, c.as_store_value())?;
        out.push((c.id, result));
    }
    Ok(out)
}

pub fn load_contract(
    store: &dyn Store,
    tenant: &str,
    id: &str,
) -> Result<Option<SolHarnessContract>, StoreError> {
    match store.get(tenant, CONTRACT_TABLE, id)? {
        None => Ok(None),
        Some(row) => Ok(Some(
            SolHarnessContract::from_store_value(&row.value)
                .map_err(StoreError::Internal)?,
        )),
    }
}

/// Upper bound on installed harness contracts per tenant.
///
/// A silent scan truncation is indistinguishable from "not installed", so the listing refuses
/// rather than returning a short list. Raise this constant together with the admission cap.
pub const MAX_INSTALLED_CONTRACTS: usize = 4_096;

pub fn list_contract_ids(
    store: &dyn Store,
    tenant: &str,
) -> Result<Vec<String>, StoreError> {
    // Request one past the ceiling so saturation is detectable rather than silent.
    let rows = store.scan_prefix(tenant, CONTRACT_TABLE, "", MAX_INSTALLED_CONTRACTS + 1)?;
    if rows.len() > MAX_INSTALLED_CONTRACTS {
        return Err(StoreError::Internal(format!(
            "installed harness contracts exceed MAX_INSTALLED_CONTRACTS ({MAX_INSTALLED_CONTRACTS}); \
             listing would truncate silently"
        )));
    }
    Ok(rows.into_iter().map(|(k, _)| k).collect())
}

// ── contracts ───────────────────────────────────────────────────────────────

pub fn quick_reply_contract() -> SolHarnessContract {
    // sugar: prompt.quick_reply → return.finish
    SolHarnessContract::seed(
        "quick_reply",
        "Answer briefly in one or two sentences from current context",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "reply",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {
                        "text": {
                            "fn": "concat",
                            "args": [
                                {"lit": "Quick reply: "},
                                {"pull": "utterance"}
                            ]
                        }
                    },
                    "into": "out"
                }
            ]
        })
        .to_string(),
    )
}

pub fn understand_intent_contract() -> SolHarnessContract {
    // sugar: prompt.understand_intent → return.finish
    SolHarnessContract::seed(
        "understand_intent",
        "Clarify what the user wants; suggest the next harness; may ask one question",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "classify",
                    "op": "Call",
                    "id": "understand.classify@1",
                    "args": {"text": {"pull": "utterance"}},
                    "into": "intent"
                },
                {
                    "nid": "say",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {
                        "text": {
                            "fn": "concat",
                            "args": [
                                {"lit": "Intent label="},
                                {"pull": "intent.label"}
                            ]
                        }
                    },
                    "into": "out"
                }
            ]
        })
        .to_string(),
    )
}

pub fn wait_for_user_contract() -> SolHarnessContract {
    // sugar: ask_and_wait (terminal Park)
    SolHarnessContract::seed(
        "wait_for_user",
        "Ask one clarifying question and wait for the next message",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "ask",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {
                        "text": {"lit": "Before I continue, what should I clarify first?"}
                    },
                    "into": "ask1"
                },
                {
                    "nid": "wait",
                    "op": "Park",
                    "until": {"kind": "event"},
                    "into": "reply1"
                },
                {
                    "nid": "ack",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {"text": {"lit": "Thanks — continuing."}},
                    "into": "out"
                }
            ]
        })
        .to_string(),
    )
}

pub fn memory_attach_contract() -> SolHarnessContract {
    // sugar: memory.search → context.attach → prompt.quick_reply → return.finish
    SolHarnessContract::seed(
        "memory_attach",
        "Search memory and attach a short snippet to the context page",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "search",
                    "op": "Call",
                    "id": "memory.search@1",
                    "args": {"query": {"pull": "utterance"}},
                    "into": "hits"
                },
                {
                    "nid": "attach",
                    "op": "Call",
                    "id": "context.attach@1",
                    "args": {"snippet": {"pull": "hits.snippet"}},
                    "into": "page"
                },
                {
                    "nid": "reply",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {
                        "text": {
                            "fn": "concat",
                            "args": [
                                {"lit": "From memory: "},
                                {"pull": "page.note"}
                            ]
                        }
                    },
                    "into": "out"
                }
            ]
        })
        .to_string(),
    )
}

pub fn calc_sum_gate_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "calc_sum_gate",
        "Add three numbers, branch on whether sum > 10, emit a short status.",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {"nid": "seed", "op": "Const", "v": {"a": 7, "b": 5, "c": 3}},
                {
                    "nid": "add_ab",
                    "op": "Call",
                    "id": "compute.hold@1",
                    "args": {
                        "v": {"fn": "add", "args": [{"pull": "a"}, {"pull": "b"}]}
                    },
                    "into": "partial"
                },
                {
                    "nid": "add_c",
                    "op": "Call",
                    "id": "compute.hold@1",
                    "args": {
                        "v": {"fn": "add", "args": [{"pull": "partial"}, {"pull": "c"}]}
                    },
                    "into": "sum"
                },
                {
                    "nid": "gate",
                    "op": "Branch",
                    "pred": {"fn": "gt", "args": [{"pull": "sum"}, {"lit": 10}]},
                    "then": {
                        "nid": "say_large",
                        "op": "Call",
                        "id": "express.say@1",
                        "args": {"text": {"lit": "sum is large"}},
                        "into": "msg"
                    },
                    "else": {
                        "nid": "say_small",
                        "op": "Call",
                        "id": "express.say@1",
                        "args": {"text": {"lit": "sum is small"}},
                        "into": "msg"
                    }
                }
            ]
        })
        .to_string(),
    )
}

pub fn semantic_ack_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "semantic_ack",
        "Classify a sentence, branch, ask if unclear, park for user, then acknowledge.",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "classify",
                    "op": "Call",
                    "id": "understand.classify@1",
                    "args": {"text": {"pull": "utterance"}},
                    "into": "intent"
                },
                {
                    "nid": "route",
                    "op": "Branch",
                    "pred": {
                        "fn": "eq",
                        "args": [{"pull": "intent.label"}, {"lit": "clear"}]
                    },
                    "then": {
                        "nid": "ack_clear",
                        "op": "Call",
                        "id": "express.say@1",
                        "args": {"text": {"lit": "Got it — clear request."}},
                        "into": "out"
                    },
                    "else": {
                        "nid": "clarify",
                        "op": "Seq",
                        "steps": [
                            {
                                "nid": "ask",
                                "op": "Call",
                                "id": "express.say@1",
                                "args": {
                                    "text": {
                                        "lit": "Can you say that in one short sentence?"
                                    }
                                },
                                "into": "ask1"
                            },
                            {
                                "nid": "wait",
                                "op": "Park",
                                "until": {"kind": "event"},
                                "into": "reply1"
                            },
                            {
                                "nid": "ack",
                                "op": "Call",
                                "id": "express.say@1",
                                "args": {"text": {"lit": "Thanks — noted."}},
                                "into": "out"
                            }
                        ]
                    }
                }
            ]
        })
        .to_string(),
    )
}

pub fn full_reply_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "full_reply",
        "Longer grounded answer from utterance + optional page note",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [{
                "nid": "reply",
                "op": "Call",
                "id": "express.say@1",
                "args": {
                    "text": {
                        "fn": "concat",
                        "args": [
                            {"lit": "Here is a fuller answer about: "},
                            {"pull": "utterance"}
                        ]
                    }
                },
                "into": "out"
            }]
        })
        .to_string(),
    )
}

pub fn apologize_closed_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "apologize_closed",
        "Fail-closed user message when the OS cannot proceed safely",
        json!({
            "nid": "root",
            "op": "Call",
            "id": "express.say@1",
            "args": {
                "text": {"lit": "I can't complete that safely. Please rephrase or ask for a human."}
            },
            "into": "out"
        })
        .to_string(),
    )
}

pub fn calc_repeat_add_contract() -> SolHarnessContract {
    // sugar: calc_repeat_add — Loop increments n until 3
    SolHarnessContract::seed(
        "calc_repeat_add",
        "Deterministic repeat: Loop adds 1 to n three times (control Loop)",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {"nid": "seed", "op": "Const", "v": {"n": 0}},
                {
                    "nid": "loop",
                    "op": "Loop",
                    "max_iter": 5,
                    "while": {"fn": "lt", "args": [{"pull": "n"}, {"lit": 3}]},
                    "body": {
                        "nid": "inc",
                        "op": "Call",
                        "id": "compute.hold@1",
                        "args": {
                            "v": {"fn": "add", "args": [{"pull": "n"}, {"lit": 1}]}
                        },
                        "into": "n"
                    }
                },
                {
                    "nid": "say",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {"text": {"lit": "repeat-add done"}},
                    "into": "msg"
                }
            ]
        })
        .to_string(),
    )
}

pub fn calc_mul_div_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "calc_mul_div",
        "Multiply then divide with Branch on exact quotient",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {"nid": "seed", "op": "Const", "v": {"x": 12, "y": 3}},
                {
                    "nid": "prod",
                    "op": "Call",
                    "id": "compute.hold@1",
                    "args": {
                        "v": {"fn": "mul", "args": [{"pull": "x"}, {"pull": "y"}]}
                    },
                    "into": "product"
                },
                {
                    "nid": "quot",
                    "op": "Call",
                    "id": "compute.hold@1",
                    "args": {
                        "v": {"fn": "div", "args": [{"pull": "product"}, {"lit": 4}]}
                    },
                    "into": "quotient"
                },
                {
                    "nid": "gate",
                    "op": "Branch",
                    "pred": {"fn": "eq", "args": [{"pull": "quotient"}, {"lit": 9}]},
                    "then": {
                        "nid": "ok",
                        "op": "Call",
                        "id": "express.say@1",
                        "args": {"text": {"lit": "mul/div ok"}},
                        "into": "msg"
                    },
                    "else": {
                        "nid": "bad",
                        "op": "Call",
                        "id": "express.say@1",
                        "args": {"text": {"lit": "mul/div unexpected"}},
                        "into": "msg"
                    }
                }
            ]
        })
        .to_string(),
    )
}

pub fn list_filter_keep_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "list_filter_keep",
        "Filter a list by keep flag, then count kept (Filter + compute count)",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "seed",
                    "op": "Const",
                    "v": {
                        "items": [
                            {"id": 1, "keep": true},
                            {"id": 2, "keep": false},
                            {"id": 3, "keep": true}
                        ]
                    }
                },
                {
                    "nid": "filter",
                    "op": "Filter",
                    "over": "items",
                    "into": "kept",
                    "max_items": 8,
                    "pred": {"pull": "keep"}
                },
                {
                    "nid": "cnt",
                    "op": "Call",
                    "id": "compute.hold@1",
                    "args": {
                        "v": {"fn": "count", "args": [{"pull": "kept"}]}
                    },
                    "into": "kept_count"
                },
                {
                    "nid": "say",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {"text": {"lit": "filter done"}},
                    "into": "msg"
                }
            ]
        })
        .to_string(),
    )
}

pub fn string_contains_gate_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "string_contains_gate",
        "Branch if utterance contains the word help",
        json!({
            "nid": "root",
            "op": "Branch",
            "pred": {
                "fn": "contains",
                "args": [{"pull": "utterance"}, {"lit": "help"}]
            },
            "then": {
                "nid": "yes",
                "op": "Call",
                "id": "express.say@1",
                "args": {"text": {"lit": "I can help with that."}},
                "into": "out"
            },
            "else": {
                "nid": "no",
                "op": "Call",
                "id": "express.say@1",
                "args": {"text": {"lit": "Tell me how I can help."}},
                "into": "out"
            }
        })
        .to_string(),
    )
}

pub fn confirm_then_act_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "confirm_then_act",
        "Ask for yes/no confirmation, Park, then acknowledge (effect gate pattern)",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "ask",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {"text": {"lit": "Confirm this action? Reply yes or no."}},
                    "into": "ask1"
                },
                {
                    "nid": "wait",
                    "op": "Park",
                    "until": {"kind": "event"},
                    "into": "reply1"
                },
                {
                    "nid": "gate",
                    "op": "Branch",
                    "pred": {
                        "fn": "contains",
                        "args": [{"pull": "reply1.text"}, {"lit": "yes"}]
                    },
                    "then": {
                        "nid": "act",
                        "op": "Call",
                        "id": "tool.act_stub@1",
                        "args": {},
                        "into": "acted"
                    },
                    "else": {
                        "nid": "cancel",
                        "op": "Call",
                        "id": "express.say@1",
                        "args": {"text": {"lit": "Cancelled."}},
                        "into": "out"
                    }
                }
            ]
        })
        .to_string(),
    )
}

pub fn detect_fresh_utterance_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "detect_fresh_utterance",
        "If user says start over / fresh, emit fresh signal text",
        json!({
            "nid": "root",
            "op": "Branch",
            "pred": {
                "fn": "or",
                "args": [
                    {"fn": "contains", "args": [{"pull": "utterance"}, {"lit": "start over"}]},
                    {"fn": "contains", "args": [{"pull": "utterance"}, {"lit": "fresh"}]}
                ]
            },
            "then": {
                "nid": "fresh",
                "op": "Call",
                "id": "express.say@1",
                "args": {"text": {"lit": "stack_control=fresh"}},
                "into": "out"
            },
            "else": {
                "nid": "stay",
                "op": "Call",
                "id": "express.say@1",
                "args": {"text": {"lit": "stack_control=stay"}},
                "into": "out"
            }
        })
        .to_string(),
    )
}

/// Built from sugar steps → Sol (see test `build_greet_harness_steps_saved_as_sol`).
///
/// Sugar steps:
/// 1. `greet` — say hello
/// 2. `gate` — if utterance contains "help"
/// 3a. `offer_help` — offer assistance
/// 3b. `ask_need` — ask what they need
pub fn greet_then_offer_help_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "greet_then_offer_help",
        "Greet, then branch: offer help or ask what they need",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "greet",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {"text": {"lit": "Hello — I'm here."}},
                    "into": "greeting"
                },
                {
                    "nid": "gate",
                    "op": "Branch",
                    "pred": {
                        "fn": "contains",
                        "args": [{"pull": "utterance"}, {"lit": "help"}]
                    },
                    "then": {
                        "nid": "offer_help",
                        "op": "Call",
                        "id": "express.say@1",
                        "args": {"text": {"lit": "I can help. What should we do first?"}},
                        "into": "out"
                    },
                    "else": {
                        "nid": "ask_need",
                        "op": "Call",
                        "id": "express.say@1",
                        "args": {"text": {"lit": "What do you need today?"}},
                        "into": "out"
                    }
                }
            ]
        })
        .to_string(),
    )
}

/// Parent harness: Call child harness via `harness.invoke@1` (synchronous wait), then use output.
///
/// Sugar steps:
/// 1. `delegate` — invoke child `calc_sum_gate`, wait for full child bag
/// 2. `report` — parent says what child returned
pub fn parent_waits_on_child_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "parent_waits_on_child",
        "Call another harness, wait for its output, then continue",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "delegate",
                    "op": "Call",
                    "id": "harness.invoke@1",
                    "args": {"id": {"lit": "calc_sum_gate"}},
                    "into": "child"
                },
                {
                    "nid": "report",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {
                        "text": {
                            "fn": "concat",
                            "args": [
                                {"lit": "Parent got child: "},
                                {"pull": "child.msg.text"}
                            ]
                        }
                    },
                    "into": "out"
                }
            ]
        })
        .to_string(),
    )
}

/// memory_attach → then longer reply using child output.
pub fn memory_then_full_reply_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "memory_then_full_reply",
        "Combine memory_attach + full_reply: search/attach then expand answer",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "mem",
                    "op": "Call",
                    "id": "harness.invoke@1",
                    "args": {
                        "id": {"lit": "memory_attach"},
                        "utterance": {"pull": "utterance"}
                    },
                    "into": "child"
                },
                {
                    "nid": "expand",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {
                        "text": {
                            "fn": "concat",
                            "args": [
                                {"lit": "Here is a fuller answer. "},
                                {"pull": "child.out.text"}
                            ]
                        }
                    },
                    "into": "out"
                }
            ]
        })
        .to_string(),
    )
}

/// greet_then_offer_help → quick_reply.
pub fn greet_then_quick_reply_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "greet_then_quick_reply",
        "Combine greet_then_offer_help + quick_reply",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "greet_child",
                    "op": "Call",
                    "id": "harness.invoke@1",
                    "args": {
                        "id": {"lit": "greet_then_offer_help"},
                        "utterance": {"pull": "utterance"}
                    },
                    "into": "greeted"
                },
                {
                    "nid": "qr",
                    "op": "Call",
                    "id": "harness.invoke@1",
                    "args": {
                        "id": {"lit": "quick_reply"},
                        "utterance": {"pull": "utterance"}
                    },
                    "into": "child"
                },
                {
                    "nid": "wrap",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {
                        "text": {
                            "fn": "concat",
                            "args": [
                                {"pull": "greeted.out.text"},
                                {"lit": " | "},
                                {"pull": "child.out.text"}
                            ]
                        }
                    },
                    "into": "out"
                }
            ]
        })
        .to_string(),
    )
}

/// understand_intent → memory_attach.
pub fn understand_then_memory_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "understand_then_memory",
        "Combine understand_intent + memory_attach",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "intent",
                    "op": "Call",
                    "id": "harness.invoke@1",
                    "args": {
                        "id": {"lit": "understand_intent"},
                        "utterance": {"pull": "utterance"}
                    },
                    "into": "classified"
                },
                {
                    "nid": "mem",
                    "op": "Call",
                    "id": "harness.invoke@1",
                    "args": {
                        "id": {"lit": "memory_attach"},
                        "utterance": {"pull": "utterance"}
                    },
                    "into": "mem"
                },
                {
                    "nid": "wrap",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {
                        "text": {
                            "fn": "concat",
                            "args": [
                                {"pull": "classified.out.text"},
                                {"lit": " / "},
                                {"pull": "mem.out.text"}
                            ]
                        }
                    },
                    "into": "out"
                }
            ]
        })
        .to_string(),
    )
}

/// If "help" → memory_attach else → quick_reply.
pub fn help_router_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "help_router",
        "Branch: help→memory_attach, else→quick_reply",
        json!({
            "nid": "root",
            "op": "Branch",
            "pred": {
                "fn": "contains",
                "args": [{"pull": "utterance"}, {"lit": "help"}]
            },
            "then": {
                "nid": "with_mem",
                "op": "Seq",
                "steps": [
                    {
                        "nid": "inv_mem",
                        "op": "Call",
                        "id": "harness.invoke@1",
                        "args": {
                            "id": {"lit": "memory_attach"},
                            "utterance": {"pull": "utterance"}
                        },
                        "into": "child"
                    },
                    {
                        "nid": "norm_mem",
                        "op": "Call",
                        "id": "express.say@1",
                        "args": {"text": {"pull": "child.out.text"}},
                        "into": "out"
                    }
                ]
            },
            "else": {
                "nid": "qr_path",
                "op": "Seq",
                "steps": [
                    {
                        "nid": "inv_qr",
                        "op": "Call",
                        "id": "harness.invoke@1",
                        "args": {
                            "id": {"lit": "quick_reply"},
                            "utterance": {"pull": "utterance"}
                        },
                        "into": "child"
                    },
                    {
                        "nid": "norm_qr",
                        "op": "Call",
                        "id": "express.say@1",
                        "args": {"text": {"pull": "child.out.text"}},
                        "into": "out"
                    }
                ]
            }
        })
        .to_string(),
    )
}

/// If utterance looks like calc → calc_sum_gate else understand_intent.
pub fn intent_then_calc_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "intent_then_calc",
        "Branch: calc keywords→calc_sum_gate, else→understand_intent",
        json!({
            "nid": "root",
            "op": "Branch",
            "pred": {
                "fn": "or",
                "args": [
                    {"fn": "contains", "args": [{"pull": "utterance"}, {"lit": "sum"}]},
                    {"fn": "contains", "args": [{"pull": "utterance"}, {"lit": "add"}]},
                    {"fn": "contains", "args": [{"pull": "utterance"}, {"lit": "calc"}]}
                ]
            },
            "then": {
                "nid": "do_calc",
                "op": "Seq",
                "steps": [
                    {
                        "nid": "inv_calc",
                        "op": "Call",
                        "id": "harness.invoke@1",
                        "args": {"id": {"lit": "calc_sum_gate"}},
                        "into": "child"
                    },
                    {
                        "nid": "norm_calc",
                        "op": "Call",
                        "id": "express.say@1",
                        "args": {"text": {"pull": "child.msg.text"}},
                        "into": "out"
                    }
                ]
            },
            "else": {
                "nid": "do_intent",
                "op": "Seq",
                "steps": [
                    {
                        "nid": "inv_intent",
                        "op": "Call",
                        "id": "harness.invoke@1",
                        "args": {
                            "id": {"lit": "understand_intent"},
                            "utterance": {"pull": "utterance"}
                        },
                        "into": "child"
                    },
                    {
                        "nid": "norm_intent",
                        "op": "Call",
                        "id": "express.say@1",
                        "args": {"text": {"pull": "child.out.text"}},
                        "into": "out"
                    }
                ]
            }
        })
        .to_string(),
    )
}

/// greet_then_offer_help → memory_attach → wrap.
pub fn greet_memory_pipeline_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "greet_memory_pipeline",
        "Pipeline: greet_then_offer_help then memory_attach",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "g",
                    "op": "Call",
                    "id": "harness.invoke@1",
                    "args": {
                        "id": {"lit": "greet_then_offer_help"},
                        "utterance": {"pull": "utterance"}
                    },
                    "into": "greeted"
                },
                {
                    "nid": "m",
                    "op": "Call",
                    "id": "harness.invoke@1",
                    "args": {
                        "id": {"lit": "memory_attach"},
                        "utterance": {"pull": "utterance"}
                    },
                    "into": "mem"
                },
                {
                    "nid": "wrap",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {
                        "text": {
                            "fn": "concat",
                            "args": [
                                {"pull": "greeted.out.text"},
                                {"lit": " :: "},
                                {"pull": "mem.out.text"}
                            ]
                        }
                    },
                    "into": "out"
                }
            ]
        })
        .to_string(),
    )
}

/// If fresh/start over → greet_then_offer_help else quick_reply.
pub fn fresh_then_greet_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "fresh_then_greet",
        "Combine detect_fresh + greet vs quick_reply",
        json!({
            "nid": "root",
            "op": "Branch",
            "pred": {
                "fn": "or",
                "args": [
                    {"fn": "contains", "args": [{"pull": "utterance"}, {"lit": "start over"}]},
                    {"fn": "contains", "args": [{"pull": "utterance"}, {"lit": "fresh"}]}
                ]
            },
            "then": {
                "nid": "restart",
                "op": "Call",
                "id": "harness.invoke@1",
                "args": {
                    "id": {"lit": "greet_then_offer_help"},
                    "utterance": {"pull": "utterance"}
                },
                "into": "out"
            },
            "else": {
                "nid": "cont",
                "op": "Call",
                "id": "harness.invoke@1",
                "args": {
                    "id": {"lit": "quick_reply"},
                    "utterance": {"pull": "utterance"}
                },
                "into": "out"
            }
        })
        .to_string(),
    )
}

/// calc_sum_gate → calc_mul_div → report both.
pub fn calc_then_report_pipeline_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "calc_then_report_pipeline",
        "Pipeline: calc_sum_gate then calc_mul_div then report",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "c1",
                    "op": "Call",
                    "id": "harness.invoke@1",
                    "args": {"id": {"lit": "calc_sum_gate"}},
                    "into": "sum_child"
                },
                {
                    "nid": "c2",
                    "op": "Call",
                    "id": "harness.invoke@1",
                    "args": {"id": {"lit": "calc_mul_div"}},
                    "into": "mul_child"
                },
                {
                    "nid": "wrap",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {
                        "text": {
                            "fn": "concat",
                            "args": [
                                {"pull": "sum_child.msg.text"},
                                {"lit": " + "},
                                {"pull": "mul_child.msg.text"}
                            ]
                        }
                    },
                    "into": "out"
                }
            ]
        })
        .to_string(),
    )
}

/// T1 clarify_slot: ask for one missing field; Park; ack.
pub fn clarify_slot_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "clarify_slot",
        "Ask for one missing field; Park; acknowledge (T1 intent)",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "ask",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {
                        "text": {"lit": "What is the missing value I should use?"}
                    },
                    "into": "ask1"
                },
                {
                    "nid": "wait",
                    "op": "Park",
                    "until": {"kind": "event"},
                    "into": "reply1"
                },
                {
                    "nid": "ack",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {"text": {"lit": "Got the slot — thanks."}},
                    "into": "out"
                }
            ]
        })
        .to_string(),
    )
}

/// T1 memory_search: search only + surface snippet.
pub fn memory_search_only_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "memory_search",
        "Search memory only and return snippet (T1 memory)",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "search",
                    "op": "Call",
                    "id": "memory.search@1",
                    "args": {"query": {"pull": "utterance"}},
                    "into": "hits"
                },
                {
                    "nid": "say",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {
                        "text": {
                            "fn": "concat",
                            "args": [
                                {"lit": "Memory hit: "},
                                {"pull": "hits.snippet"}
                            ]
                        }
                    },
                    "into": "out"
                }
            ]
        })
        .to_string(),
    )
}

/// Deepest stack frame (C): no further children.
pub fn stack_leaf_c_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "stack_leaf_c",
        "Stack leaf C — concludes and returns a marker",
        json!({
            "nid": "root",
            "op": "Call",
            "id": "express.say@1",
            "args": {"text": {"lit": "leaf-C done"}},
            "into": "out"
        })
        .to_string(),
    )
}

/// Mid stack frame (B): calls C, waits for C to conclude, then continues.
pub fn stack_mid_b_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "stack_mid_b",
        "Stack mid B — invoke leaf C, wait, then report",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "call_c",
                    "op": "Call",
                    "id": "harness.invoke@1",
                    "args": {
                        "id": {"lit": "stack_leaf_c"},
                        "utterance": {"pull": "utterance"}
                    },
                    "into": "child"
                },
                {
                    "nid": "after_c",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {
                        "text": {
                            "fn": "concat",
                            "args": [
                                {"lit": "mid-B after "},
                                {"pull": "child.out.text"}
                            ]
                        }
                    },
                    "into": "out"
                }
            ]
        })
        .to_string(),
    )
}

/// Top stack frame (A): calls B (which calls C), waits for full subtree to conclude.
pub fn stack_top_a_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "stack_top_a",
        "Stack top A — invoke mid B (which invokes leaf C), wait for whole chain",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "call_b",
                    "op": "Call",
                    "id": "harness.invoke@1",
                    "args": {
                        "id": {"lit": "stack_mid_b"},
                        "utterance": {"pull": "utterance"}
                    },
                    "into": "child"
                },
                {
                    "nid": "after_b",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {
                        "text": {
                            "fn": "concat",
                            "args": [
                                {"lit": "top-A after "},
                                {"pull": "child.out.text"}
                            ]
                        }
                    },
                    "into": "out"
                }
            ]
        })
        .to_string(),
    )
}

// ── Proper Mother Sol (§8.4 / §10 registered-flow Calls) ─────────────────────
//
// Same A→B→C wait semantics, but children are pinned Call ids (`flow.*@1`), not a
// runtime `harness.invoke@1` lookup. Mother: registered flows as Call targets;
// call graph must be acyclic (recursion forbidden in v0).

/// Pinned flow Call ids for the proper Sol nested stack.
pub const FLOW_STACK_LEAF_C: &str = "flow.stack_leaf_c@1";
pub const FLOW_STACK_MID_B: &str = "flow.stack_mid_b@1";
pub const FLOW_STACK_TOP_A: &str = "flow.stack_top_a@1";

/// Proper Sol leaf C — identical body to [`stack_leaf_c_contract`], registered as a flow.
pub fn proper_sol_stack_leaf_c_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "proper_sol_stack_leaf_c",
        "Proper Sol leaf C (registered-flow target body)",
        stack_leaf_c_contract().program_json,
    )
}

/// Proper Sol mid B — `Call(flow.stack_leaf_c@1, …)` then continue (Mother registered-flow).
pub fn proper_sol_stack_mid_b_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "proper_sol_stack_mid_b",
        "Proper Sol mid B — Call pinned flow.stack_leaf_c@1, wait, report",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "call_c",
                    "op": "Call",
                    "id": FLOW_STACK_LEAF_C,
                    "args": {
                        "utterance": {"pull": "utterance"}
                    },
                    "into": "child"
                },
                {
                    "nid": "after_c",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {
                        "text": {
                            "fn": "concat",
                            "args": [
                                {"lit": "mid-B after "},
                                {"pull": "child.out.text"}
                            ]
                        }
                    },
                    "into": "out"
                }
            ]
        })
        .to_string(),
    )
}

/// Proper Sol top A — `Call(flow.stack_mid_b@1, …)` then continue.
pub fn proper_sol_stack_top_a_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "proper_sol_stack_top_a",
        "Proper Sol top A — Call pinned flow.stack_mid_b@1 (which Calls leaf), wait",
        json!({
            "nid": "root",
            "op": "Seq",
            "steps": [
                {
                    "nid": "call_b",
                    "op": "Call",
                    "id": FLOW_STACK_MID_B,
                    "args": {
                        "utterance": {"pull": "utterance"}
                    },
                    "into": "child"
                },
                {
                    "nid": "after_b",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {
                        "text": {
                            "fn": "concat",
                            "args": [
                                {"lit": "top-A after "},
                                {"pull": "child.out.text"}
                            ]
                        }
                    },
                    "into": "out"
                }
            ]
        })
        .to_string(),
    )
}

pub fn fallback_say_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "fallback_say",
        "Fallback: try primary express, else backup line (Fallback control)",
        json!({
            "nid": "root",
            "op": "Fallback",
            "steps": [
                {
                    "nid": "primary",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {"text": {"lit": "primary line"}},
                    "into": "out"
                },
                {
                    "nid": "backup",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": {"text": {"lit": "backup line"}},
                    "into": "out"
                }
            ]
        })
        .to_string(),
    )
}

pub fn guard_required_field_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "guard_required_field",
        "Guard: require bag.ready before saying ok (Guard control)",
        json!({
            "nid": "root",
            "op": "Guard",
            "check": "entry",
            "invariant": {"fn": "exists", "args": [{"pull": "ready"}]},
            "body": {
                "nid": "ok",
                "op": "Call",
                "id": "express.say@1",
                "args": {"text": {"lit": "ready field present"}},
                "into": "out"
            },
            "on_violation": {
                "nid": "bad",
                "op": "Call",
                "id": "express.say@1",
                "args": {"text": {"lit": "missing ready"}},
                "into": "out"
            }
        })
        .to_string(),
    )
}

pub fn once_external_stub_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "once_external_stub",
        "Once-wrapped external stub Call (idempotent effect pattern)",
        json!({
            "nid": "root",
            "op": "Once",
            "body": {
                "nid": "send",
                "op": "Call",
                "id": "tool.act_stub@1",
                "args": {},
                "into": "sent"
            }
        })
        .to_string(),
    )
}

pub fn budgeted_express_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "budgeted_express",
        "Budget-capped express Call (calls=2, ms=5000)",
        json!({
            "nid": "root",
            "op": "Budget",
            "calls": 2,
            "ms": 5000,
            "body": {
                "nid": "say",
                "op": "Call",
                "id": "express.say@1",
                "args": {"text": {"lit": "budgeted hello"}},
                "into": "out"
            }
        })
        .to_string(),
    )
}

/// `workflow.average` — production plan §5.4 / §5.16 reference compound.
///
/// Sequential pure composition (true `SpawnMany` join is a later milestone):
/// `collection.count` → Guard nonempty → `math.sum` → `math.divide` → `average`.
/// Input bag: `{ "values": [numbers] }`. Output bag field: `average`.
pub fn workflow_average_contract() -> SolHarnessContract {
    SolHarnessContract::seed(
        "workflow.average",
        "Average a number list: count → guard nonempty → sum → divide (pure Sol, P0 stdlib Calls).",
        workflow_average_program_json().to_string(),
    )
}

/// Canonical Sol program body for `workflow.average@1.0.0`.
pub fn workflow_average_program_json() -> serde_json::Value {
    json!({
        "nid": "root",
        "op": "Seq",
        "steps": [
            {
                "nid": "count_values",
                "op": "Call",
                "id": "collection.count@1",
                "args": { "list": { "pull": "values" } },
                "into": "count"
            },
            {
                "nid": "require_nonempty",
                "op": "Guard",
                "check": "entry",
                "invariant": {
                    "fn": "gt",
                    "args": [{ "pull": "count" }, { "lit": 0 }]
                },
                "body": {
                    "nid": "compute_avg",
                    "op": "Seq",
                    "steps": [
                        {
                            "nid": "sum_values",
                            "op": "Call",
                            "id": "math.sum@1",
                            "args": { "list": { "pull": "values" } },
                            "into": "sum"
                        },
                        {
                            "nid": "divide",
                            "op": "Call",
                            "id": "math.divide@1",
                            "args": {
                                "a": { "pull": "sum" },
                                "b": { "pull": "count" }
                            },
                            "into": "average"
                        }
                    ]
                },
                "on_violation": {
                    "nid": "empty_error",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": { "text": { "lit": "Math.EmptyInput" } },
                    "into": "error"
                }
            }
        ]
    })
}

// ── harness.invoke@1 (parent calls child, waits for output) ─────────────────

use crate::driver::{Instance, TurnOutcome};
use crate::error::{ErrV1, ReasonCode};
use crate::registry::{Boundedness, Declaration, Origin, Registry, TargetClass};
use crate::compile;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Registry that covers every Call id used by the seed library (stubs only).
///
/// Used for Phase 1 admission: compile + registered targets + effect rank. Nested execution still
/// needs [`registry_with_harness_invoke`] / [`registry_with_proper_stack_flows`].
pub fn seed_admission_registry() -> Registry {
    let mut r = crate::stdlib_targets::registry_with_p0_stdlib();
    r.register("harness.invoke@1", EffectClass::Read, |_| {
        Ok(SolValue::map::<_, &str>([]))
    });
    for flow_id in [FLOW_STACK_LEAF_C, FLOW_STACK_MID_B, FLOW_STACK_TOP_A] {
        r.register(flow_id, EffectClass::Read, |_| Ok(SolValue::map::<_, &str>([])));
    }
    r
}

/// Leaf Call stubs used by seed harnesses (no nested invoke).
pub fn leaf_call_registry() -> Registry {
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
    r.register("tool.act_stub@1", EffectClass::External, |_| {
        Ok(SolValue::map([("ok", SolValue::Bool(true))]))
    });
    r
}

/// Registry that can run nested Sol harnesses via `harness.invoke@1`.
///
/// Args: `{ "id": "<child harness id>", ...optional bag fields for child }`.
/// Returns the **completed child bag** (parent `into` path receives it). Call is synchronous =
/// parent waits for child output before the next Sol step.
///
/// **Nested stack:** the child registry also has `harness.invoke@1`, so B can call C while A waits
/// on B. Depth is capped by [`HARNESS_INVOKE_MAX_DEPTH`].
pub const HARNESS_INVOKE_MAX_DEPTH: u32 = 8;

pub fn registry_with_harness_invoke(programs: HashMap<String, String>) -> Registry {
    let programs = Arc::new(programs);
    let depth = Arc::new(AtomicU32::new(0));
    registry_with_harness_invoke_inner(programs, depth)
}

fn registry_with_harness_invoke_inner(
    programs: Arc<HashMap<String, String>>,
    depth: Arc<AtomicU32>,
) -> Registry {
    let mut r = leaf_call_registry();
    let programs_c = programs.clone();
    let depth_c = depth.clone();
    r.register("harness.invoke@1", EffectClass::Read, move |args| {
        let map = args.as_map().ok_or_else(|| {
            ErrV1::new(ReasonCode::Type, "harness.invoke", "args must be a map")
        })?;
        let id = match map.get("id") {
            Some(SolValue::Str(s)) => s.as_str(),
            _ => {
                return Err(ErrV1::new(
                    ReasonCode::Missing,
                    "harness.invoke",
                    "args.id string required",
                ))
            }
        };
        let cur = depth_c.fetch_add(1, Ordering::SeqCst);
        if cur >= HARNESS_INVOKE_MAX_DEPTH {
            depth_c.fetch_sub(1, Ordering::SeqCst);
            return Err(ErrV1::new(
                ReasonCode::BudgetCalls,
                "harness.invoke",
                format!("harness invoke depth exceeded max={HARNESS_INVOKE_MAX_DEPTH}"),
            ));
        }
        let run = (|| {
            let program_json = programs_c.get(id).ok_or_else(|| {
                ErrV1::new(
                    ReasonCode::Missing,
                    "harness.invoke",
                    format!("unknown child harness id={id}"),
                )
            })?;
            let child_bag = if map.len() <= 1 {
                SolValue::map::<_, &str>([])
            } else {
                SolValue::map(
                    map.iter()
                        .filter(|(k, _)| k.as_str() != "id")
                        .map(|(k, v)| (k.clone(), v.clone())),
                )
            };
            let program = compile(program_json).map_err(|e| {
                ErrV1::new(
                    ReasonCode::Shape,
                    "harness.invoke",
                    format!("child compile failed: {e:?}"),
                )
            })?;
            // Child may itself Call harness.invoke@1 (nested stack).
            let mut child_reg =
                registry_with_harness_invoke_inner(programs_c.clone(), depth_c.clone());
            let mut inst = Instance::new(program, &mut child_reg);
            match inst.start(child_bag).map_err(|e| {
                ErrV1::new(
                    ReasonCode::Internal,
                    "harness.invoke",
                    format!("child start failed: {e:?}"),
                )
            })? {
                TurnOutcome::Completed { bag, .. } => Ok(bag),
                TurnOutcome::Parked(_) => Err(ErrV1::new(
                    ReasonCode::Internal,
                    "harness.invoke",
                    "child harness parked; parent/child park handoff not in this demo",
                )),
            }
        })();
        depth_c.fetch_sub(1, Ordering::SeqCst);
        run
    });
    r
}

/// Map of pinned flow Call id → program JSON for the proper Sol A→B→C stack.
pub fn proper_sol_stack_program_map() -> HashMap<String, String> {
    HashMap::from([
        (
            FLOW_STACK_LEAF_C.to_string(),
            proper_sol_stack_leaf_c_contract().program_json,
        ),
        (
            FLOW_STACK_MID_B.to_string(),
            proper_sol_stack_mid_b_contract().program_json,
        ),
        (
            FLOW_STACK_TOP_A.to_string(),
            proper_sol_stack_top_a_contract().program_json,
        ),
    ])
}

/// Registry where nested harnesses are **registered-flow** Call targets (§10 / F6).
///
/// Parent Sol writes `Call { id: "flow.stack_mid_b@1", … }` — the child id is pinned in the
/// program (Mother style), not chosen via `harness.invoke` args. Flow call graph is acyclic.
pub fn registry_with_proper_stack_flows(tenant: &str) -> Result<Registry, String> {
    let programs = Arc::new(proper_sol_stack_program_map());
    let depth = Arc::new(AtomicU32::new(0));
    let mut r = registry_with_proper_stack_flows_inner(programs, depth, tenant)?;
    r.set_flow_calls(FLOW_STACK_TOP_A, vec![FLOW_STACK_MID_B.to_string()])?;
    r.set_flow_calls(FLOW_STACK_MID_B, vec![FLOW_STACK_LEAF_C.to_string()])?;
    r.set_flow_calls(FLOW_STACK_LEAF_C, vec![])?;
    r.validate_call_graph(tenant)?;
    Ok(r)
}

fn registry_with_proper_stack_flows_inner(
    programs: Arc<HashMap<String, String>>,
    depth: Arc<AtomicU32>,
    tenant: &str,
) -> Result<Registry, String> {
    let mut r = leaf_call_registry();
    for flow_id in [FLOW_STACK_LEAF_C, FLOW_STACK_MID_B, FLOW_STACK_TOP_A] {
        let programs_c = programs.clone();
        let depth_c = depth.clone();
        let tenant_s = tenant.to_string();
        let my_id = flow_id.to_string();
        r.register_declared(
            Declaration {
                id: my_id.clone(),
                class: TargetClass::Flow,
                input_imprint: "stack_in@1".into(),
                output_imprint: "stack_out@1".into(),
                boundedness: Boundedness::RegisteredFlow,
                effect_class: EffectClass::Read,
                policy_tags: vec![],
                tenant: tenant_s.clone(),
                origin: Origin::Tenant,
            },
            move |args| {
                let cur = depth_c.fetch_add(1, Ordering::SeqCst);
                if cur >= HARNESS_INVOKE_MAX_DEPTH {
                    depth_c.fetch_sub(1, Ordering::SeqCst);
                    return Err(ErrV1::new(
                        ReasonCode::BudgetCalls,
                        &my_id,
                        format!("registered-flow depth exceeded max={HARNESS_INVOKE_MAX_DEPTH}"),
                    ));
                }
                let run = (|| {
                    let program_json = programs_c.get(&my_id).ok_or_else(|| {
                        ErrV1::new(
                            ReasonCode::Missing,
                            &my_id,
                            format!("missing registered-flow body for {my_id}"),
                        )
                    })?;
                    let program = compile(program_json).map_err(|e| {
                        ErrV1::new(
                            ReasonCode::Shape,
                            &my_id,
                            format!("flow compile failed: {e:?}"),
                        )
                    })?;
                    let mut child_reg = registry_with_proper_stack_flows_inner(
                        programs_c.clone(),
                        depth_c.clone(),
                        &tenant_s,
                    )
                    .map_err(|e| ErrV1::new(ReasonCode::Internal, &my_id, e))?;
                    let mut inst = Instance::new(program, &mut child_reg);
                    match inst.start(args.clone()).map_err(|e| {
                        ErrV1::new(
                            ReasonCode::Internal,
                            &my_id,
                            format!("flow start failed: {e:?}"),
                        )
                    })? {
                        TurnOutcome::Completed { bag, .. } => Ok(bag),
                        TurnOutcome::Parked(_) => Err(ErrV1::new(
                            ReasonCode::Internal,
                            &my_id,
                            "registered-flow parked; park handoff not in this demo",
                        )),
                    }
                })();
                depth_c.fetch_sub(1, Ordering::SeqCst);
                run
            },
        )?;
    }
    Ok(r)
}

/// Map of id → program_json for the full seed library (for invoke).
pub fn library_program_map() -> HashMap<String, String> {
    sol_harness_library()
        .into_iter()
        .map(|c| (c.id, c.program_json))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile;
    use aelio_store::MemoryStore;

    #[test]
    fn every_library_program_compiles() {
        for c in sol_harness_library() {
            compile(&c.program_json).unwrap_or_else(|e| {
                panic!("library id={} failed plan: {e:?}", c.id);
            });
        }
    }

    #[test]
    fn list_contract_ids_handles_three_hundred_without_truncation() {
        let mut store = MemoryStore::new();
        // Phase 0.4: listing must not silently truncate a growing library.
        for i in 0..300 {
            let c = SolHarnessContract::seed(
                format!("bulk.harness_{i:04}"),
                format!("bulk {i}"),
                r#"{"nid":"root","op":"Const","v":{}}"#,
            );
            store
                .put_if_absent("bulk", CONTRACT_TABLE, &c.id, c.as_store_value())
                .unwrap();
        }
        let ids = list_contract_ids(&store, "bulk").unwrap();
        assert_eq!(ids.len(), 300);
    }

    #[test]
    fn store_library_persists_all_contracts() {
        let mut store = MemoryStore::new();
        let lib = sol_harness_library();
        let n = lib.len();
        assert!(n >= 36, "expected combination-expanded library, got {n}");
        let results = store_library(&mut store, "demo").unwrap();
        assert_eq!(results.len(), n);
        let ids = list_contract_ids(&store, "demo").unwrap();
        assert_eq!(ids.len(), n);
        for c in &lib {
            let loaded = load_contract(&store, "demo", &c.id).unwrap().unwrap();
            assert_eq!(loaded.id, c.id);
            assert!(!loaded.program_json.is_empty());
        }
        let again = store_library(&mut store, "demo").unwrap();
        assert!(again
            .iter()
            .all(|(_, r)| matches!(r, PutIfAbsent::Existing(_))));
        assert_eq!(list_contract_ids(&store, "demo").unwrap().len(), n);
    }

    #[test]
    fn phase1_seed_contracts_have_identity_version_library() {
        for c in sol_harness_library() {
            assert_eq!(c.version, 1, "id={}", c.id);
            assert!(!c.library.is_empty(), "id={}", c.id);
            assert_eq!(c.identity.len(), 64, "id={} identity not blake3 hex", c.id);
            assert!(
                c.identity.chars().all(|ch| ch.is_ascii_hexdigit()),
                "id={} identity not hex",
                c.id
            );
            let expected = contract_identity(&c.id, c.version, &c.program_json);
            assert_eq!(c.identity, expected, "id={}", c.id);
            c.budget.within_global_ceiling().unwrap();
        }
    }

    #[test]
    fn phase1_store_round_trip_preserves_contract_fields() {
        let c = quick_reply_contract();
        let v = c.as_store_value();
        let loaded = SolHarnessContract::from_store_value(&v).unwrap();
        assert_eq!(loaded.id, c.id);
        assert_eq!(loaded.version, c.version);
        assert_eq!(loaded.summary, c.summary);
        assert_eq!(loaded.library, c.library);
        assert_eq!(loaded.program_json, c.program_json);
        assert_eq!(loaded.identity, c.identity);
        assert_eq!(loaded.effect, c.effect);
        assert_eq!(loaded.budget, c.budget);
    }

    #[test]
    fn phase1_legacy_three_field_rows_load_with_defaults() {
        // Pre-Phase-1 store shape: only id / summary / program_json.
        let legacy = SolValue::map([
            ("id", SolValue::str("legacy_quick")),
            ("kind", SolValue::str("sol_harness")),
            ("summary", SolValue::str("old row")),
            (
                "program_json",
                SolValue::str(r#"{"nid":"root","op":"Const","v":{}}"#),
            ),
        ]);
        let c = SolHarnessContract::from_store_value(&legacy).unwrap();
        assert_eq!(c.id, "legacy_quick");
        assert_eq!(c.version, 1);
        assert!(!c.library.is_empty());
        assert_eq!(c.identity.len(), 64);
        assert_eq!(c.effect, EffectClass::External);
        assert!(c.budget.max_calls > 0);
    }

    #[test]
    fn phase1_every_library_contract_admits_with_seed_registry() {
        let reg = seed_admission_registry();
        for c in sol_harness_library() {
            c.admit(Some(&reg), "demo")
                .unwrap_or_else(|e| panic!("admit {}: {:?}", c.id, e));
        }
    }

    #[test]
    fn phase1_store_library_admitted_persists_identity() {
        let mut store = MemoryStore::new();
        let reg = seed_admission_registry();
        store_library_admitted(&mut store, "demo", Some(&reg)).unwrap();
        let loaded = load_contract(&store, "demo", "quick_reply")
            .unwrap()
            .expect("quick_reply stored");
        assert_eq!(loaded.identity.len(), 64);
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.library, "conv");
        assert_ne!(loaded.effect, EffectClass::Pure); // express.say is Read
    }

    #[test]
    fn parent_waits_on_child_invoke_works() {
        let parent = parent_waits_on_child_contract();
        compile(&parent.program_json).unwrap();
        let mut registry = registry_with_harness_invoke(library_program_map());
        let mut inst = Instance::new(compile(&parent.program_json).unwrap(), &mut registry);
        let (bag, _) = match inst.start(SolValue::map::<_, &str>([])).unwrap() {
            TurnOutcome::Completed { bag, bag_hash } => (bag, bag_hash),
            TurnOutcome::Parked(_) => panic!("parent should complete"),
        };
        let child_sum = bag.as_map().and_then(|m| m.get("child")).and_then(|c| {
            c.as_map().and_then(|m| m.get("sum")).cloned()
        });
        assert_eq!(child_sum, Some(SolValue::Int(15)));
        let out = bag
            .as_map()
            .and_then(|m| m.get("out"))
            .and_then(|m| m.as_map())
            .and_then(|m| m.get("text"));
        assert_eq!(
            out,
            Some(&SolValue::str("Parent got child: sum is large"))
        );
    }

    #[test]
    fn nested_stack_a_waits_on_b_waits_on_c() {
        // A → B → C; each Call harness.invoke@1 blocks until the child Completes.
        let top = stack_top_a_contract();
        compile(&top.program_json).unwrap();
        let mut registry = registry_with_harness_invoke(library_program_map());
        let mut inst = Instance::new(compile(&top.program_json).unwrap(), &mut registry);
        let bag = match inst
            .start(SolValue::map([("utterance", SolValue::str("go"))]))
            .unwrap()
        {
            TurnOutcome::Completed { bag, .. } => bag,
            TurnOutcome::Parked(_) => panic!("stack_top_a should complete after B and C"),
        };
        let text = bag
            .as_map()
            .and_then(|m| m.get("out"))
            .and_then(|m| m.as_map())
            .and_then(|m| m.get("text"));
        assert_eq!(
            text,
            Some(&SolValue::str("top-A after mid-B after leaf-C done"))
        );
    }

    #[test]
    fn proper_sol_registered_flow_stack_waits_and_is_acyclic() {
        for c in [
            proper_sol_stack_leaf_c_contract(),
            proper_sol_stack_mid_b_contract(),
            proper_sol_stack_top_a_contract(),
        ] {
            compile(&c.program_json).unwrap_or_else(|e| panic!("{}: {e:?}", c.id));
        }

        let mut registry = registry_with_proper_stack_flows("demo").expect("flow registry");
        // Top program Calls flow.stack_mid_b@1 (pinned), which Calls flow.stack_leaf_c@1.
        let top = proper_sol_stack_top_a_contract();
        let mut inst = Instance::new(compile(&top.program_json).unwrap(), &mut registry);
        let bag = match inst
            .start(SolValue::map([("utterance", SolValue::str("go"))]))
            .unwrap()
        {
            TurnOutcome::Completed { bag, .. } => bag,
            TurnOutcome::Parked(_) => panic!("proper Sol top should complete"),
        };
        let text = bag
            .as_map()
            .and_then(|m| m.get("out"))
            .and_then(|m| m.as_map())
            .and_then(|m| m.get("text"));
        assert_eq!(
            text,
            Some(&SolValue::str("top-A after mid-B after leaf-C done"))
        );

        // Cycle must be refused (Mother §8.4).
        let mut bad = registry_with_proper_stack_flows("demo").unwrap();
        bad.set_flow_calls(FLOW_STACK_LEAF_C, vec![FLOW_STACK_TOP_A.to_string()])
            .unwrap();
        assert!(bad.validate_call_graph("demo").is_err());
    }

    #[test]
    fn combination_harnesses_compile_and_help_router_runs() {
        for id in [
            "memory_then_full_reply",
            "greet_then_quick_reply",
            "understand_then_memory",
            "help_router",
            "intent_then_calc",
            "greet_memory_pipeline",
            "fresh_then_greet",
            "calc_then_report_pipeline",
            "clarify_slot",
            "memory_search",
            "stack_leaf_c",
            "stack_mid_b",
            "stack_top_a",
            "proper_sol_stack_leaf_c",
            "proper_sol_stack_mid_b",
            "proper_sol_stack_top_a",
        ] {
            let c = sol_harness_library()
                .into_iter()
                .find(|x| x.id == id)
                .unwrap_or_else(|| panic!("missing {id}"));
            compile(&c.program_json).unwrap_or_else(|e| panic!("{id}: {e:?}"));
        }

        let mut registry = registry_with_harness_invoke(library_program_map());
        let help = sol_harness_library()
            .into_iter()
            .find(|c| c.id == "help_router")
            .unwrap();
        let mut inst = Instance::new(compile(&help.program_json).unwrap(), &mut registry);
        let bag = match inst
            .start(SolValue::map([("utterance", SolValue::str("please help me"))]))
            .unwrap()
        {
            TurnOutcome::Completed { bag, .. } => bag,
            TurnOutcome::Parked(_) => panic!("help_router should complete"),
        };
        let text = bag
            .as_map()
            .and_then(|m| m.get("out"))
            .and_then(|m| m.as_map())
            .and_then(|m| m.get("text"));
        assert_eq!(text, Some(&SolValue::str("From memory: prior note")));

        let mut registry2 = registry_with_harness_invoke(library_program_map());
        let pipeline = sol_harness_library()
            .into_iter()
            .find(|c| c.id == "calc_then_report_pipeline")
            .unwrap();
        let mut inst2 = Instance::new(compile(&pipeline.program_json).unwrap(), &mut registry2);
        let bag2 = match inst2.start(SolValue::map::<_, &str>([])).unwrap() {
            TurnOutcome::Completed { bag, .. } => bag,
            TurnOutcome::Parked(_) => panic!("pipeline should complete"),
        };
        let text2 = bag2
            .as_map()
            .and_then(|m| m.get("out"))
            .and_then(|m| m.as_map())
            .and_then(|m| m.get("text"));
        assert_eq!(
            text2,
            Some(&SolValue::str("sum is large + mul/div ok"))
        );
    }

    /// `cargo test -p aelio-kernel dump_library_json_files -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_library_json_files() {
        use std::fs;
        use std::path::PathBuf;
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../docs/examples/sol_harness_essence/library");
        fs::create_dir_all(&dir).unwrap();
        let mut catalog = serde_json::json!({
            "table": CONTRACT_TABLE,
            "count": 0,
            "contracts": []
        });
        let lib = sol_harness_library();
        catalog["count"] = serde_json::json!(lib.len());
        for c in &lib {
            let program: serde_json::Value = serde_json::from_str(&c.program_json).unwrap();
            let body = serde_json::json!({
                "id": c.id,
                "summary": c.summary,
                "program": program,
            });
            let path = dir.join(format!("{}.json", c.id));
            fs::write(&path, serde_json::to_string_pretty(&body).unwrap() + "\n").unwrap();
            catalog["contracts"].as_array_mut().unwrap().push(serde_json::json!({
                "id": c.id,
                "file": format!("{}.json", c.id),
                "summary": c.summary,
            }));
        }
        fs::write(
            dir.join("catalog.json"),
            serde_json::to_string_pretty(&catalog).unwrap() + "\n",
        )
        .unwrap();
        eprintln!("dumped {} contracts to {}", lib.len(), dir.display());
    }
}
