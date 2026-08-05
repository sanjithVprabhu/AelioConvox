//! Privileged harness orchestration helpers over [`ProcessTree`].
//!
//! These are the Phase 3.1 spine (`harness.spawn` / `join_all`) as Rust APIs.
//! Registry Call adapters can wrap them once a durable tree handle is injected.

use crate::compile;
use crate::driver::{Instance, TurnOutcome};
use crate::error::{ErrV1, ReasonCode};
use crate::os_contract::{JoinPolicyV1, ResultEnvelopeV1, VersionPinV1};
use crate::process_tree::ProcessTree;
use crate::registry::Registry;
use crate::stdlib_targets::registry_with_p0_stdlib;
use crate::workflow_average_program_json;
use aelio_sol::{value_hash, SolValue};
use serde_json::json;

/// Run a pure Sol program to completion; returns bag + bag_hash.
pub fn run_pure_sol(
    program_json: &str,
    bag: SolValue,
    registry: &mut Registry,
) -> Result<(SolValue, String), ErrV1> {
    let program = compile(program_json)?;
    let mut inst = Instance::new(program, registry);
    match inst.start(bag)? {
        TurnOutcome::Completed { bag, bag_hash } => Ok((bag, bag_hash)),
        TurnOutcome::Parked(_) => Err(ErrV1::new(
            ReasonCode::Internal,
            "run_pure_sol",
            "pure program parked unexpectedly",
        )),
    }
}

/// Tiny Sol programs for sum/count leaf harnesses (stdlib Call only).
pub fn math_sum_program_json() -> serde_json::Value {
    json!({
        "nid": "root",
        "op": "Call",
        "id": "math.sum@1",
        "args": { "list": { "pull": "values" } },
        "into": "sum"
    })
}

pub fn collection_count_program_json() -> serde_json::Value {
    json!({
        "nid": "root",
        "op": "Call",
        "id": "collection.count@1",
        "args": { "list": { "pull": "values" } },
        "into": "count"
    })
}

pub fn math_divide_program_json() -> serde_json::Value {
    json!({
        "nid": "root",
        "op": "Call",
        "id": "math.divide@1",
        "args": {
            "a": { "pull": "sum" },
            "b": { "pull": "count" }
        },
        "into": "average"
    })
}

/// Execute `workflow.average` using process-tree spawn + join=all + divide.
///
/// This is the structured-concurrency form of average (plan §5.4). Children run
/// sequentially here; join ordering is still deterministic. Parallel dispatch
/// is a later executor change.
pub fn workflow_average_via_join(
    tree: &mut ProcessTree,
    values: &[i64],
) -> Result<(String, SolValue, String), ErrV1> {
    let bag_in = SolValue::map([(
        "values",
        SolValue::List(values.iter().copied().map(SolValue::Int).collect()),
    )]);

    let root = tree
        .spawn_root(
            VersionPinV1 {
                id: "workflow.average".into(),
                version: "1.0.0".into(),
                artifact_hash: None,
            },
            None,
        )
        .map_err(|e| e.into_errv1("spawn_root"))?;

    let (join_id, kids) = tree
        .spawn_many_all(
            &root,
            vec![
                (
                    "sum".into(),
                    VersionPinV1 {
                        id: "math.sum".into(),
                        version: "1.0.0".into(),
                        artifact_hash: None,
                    },
                ),
                (
                    "count".into(),
                    VersionPinV1 {
                        id: "collection.count".into(),
                        version: "1.0.0".into(),
                        artifact_hash: None,
                    },
                ),
            ],
        )
        .map_err(|e| e.into_errv1("spawn_many_all"))?;

    let mut registry = registry_with_p0_stdlib();

    // Child 0: sum
    let (sum_bag, sum_hash) = run_pure_sol(
        &math_sum_program_json().to_string(),
        bag_in.clone(),
        &mut registry,
    )?;
    let sum_val = sum_bag
        .as_map()
        .and_then(|m| m.get("sum"))
        .cloned()
        .ok_or_else(|| ErrV1::new(ReasonCode::Missing, "sum", "sum field missing"))?;
    tree.complete_ok(
        &kids[0],
        sol_to_json(&sum_val),
        Some(sum_bag),
        Some(sum_hash),
    )
    .map_err(|e| e.into_errv1("complete_sum"))?;

    // Child 1: count
    let (count_bag, count_hash) = run_pure_sol(
        &collection_count_program_json().to_string(),
        bag_in,
        &mut registry,
    )?;
    let count_val = count_bag
        .as_map()
        .and_then(|m| m.get("count"))
        .cloned()
        .ok_or_else(|| ErrV1::new(ReasonCode::Missing, "count", "count field missing"))?;
    tree.complete_ok(
        &kids[1],
        sol_to_json(&count_val),
        Some(count_bag),
        Some(count_hash),
    )
    .map_err(|e| e.into_errv1("complete_count"))?;

    let join = tree
        .join(&join_id)
        .ok_or_else(|| ErrV1::new(ReasonCode::Missing, "join", "join missing"))?;
    let term = join
        .terminal
        .clone()
        .ok_or_else(|| ErrV1::new(ReasonCode::Internal, "join", "join not terminal"))?;

    let (sum_json, count_json) = match term {
        ResultEnvelopeV1::Ok { output, .. } => {
            let arr = output.as_array().ok_or_else(|| {
                ErrV1::new(ReasonCode::Type, "join_all", "expected ordered result array")
            })?;
            let mut sum = serde_json::Value::Null;
            let mut count = serde_json::Value::Null;
            for item in arr {
                match item.get("nid").and_then(|v| v.as_str()) {
                    Some("sum") => sum = item.get("output").cloned().unwrap_or(serde_json::Value::Null),
                    Some("count") => {
                        count = item.get("output").cloned().unwrap_or(serde_json::Value::Null)
                    }
                    _ => {}
                }
            }
            (sum, count)
        }
        ResultEnvelopeV1::Err { code, message, .. } => {
            return Err(ErrV1::new(ReasonCode::Type, "join_all", format!("{code}: {message}")));
        }
        ResultEnvelopeV1::Cancelled { reason } => {
            return Err(ErrV1::new(ReasonCode::Policy, "join_all", reason));
        }
    };

    if count_json.as_i64() == Some(0) {
        tree.complete_err(&root, "Math.EmptyInput", "empty values")
            .map_err(|e| e.into_errv1("complete_root_empty"))?;
        return Err(ErrV1::new(
            ReasonCode::Type,
            "workflow.average",
            "Math.EmptyInput",
        ));
    }

    let divide_bag = SolValue::map([
        ("sum", json_to_sol(&sum_json)?),
        ("count", json_to_sol(&count_json)?),
    ]);
    let (avg_bag, avg_hash) = run_pure_sol(
        &math_divide_program_json().to_string(),
        divide_bag,
        &mut registry,
    )?;
    let average = avg_bag
        .as_map()
        .and_then(|m| m.get("average"))
        .cloned()
        .ok_or_else(|| ErrV1::new(ReasonCode::Missing, "average", "average field missing"))?;

    tree.complete_ok(
        &root,
        sol_to_json(&average),
        Some(avg_bag.clone()),
        Some(avg_hash.clone()),
    )
    .map_err(|e| e.into_errv1("complete_root"))?;

    // Sanity: join policy was All
    assert!(matches!(
        tree.join(&join_id).unwrap().policy,
        JoinPolicyV1::All
    ));
    let _ = value_hash(&avg_bag);
    Ok((root, avg_bag, avg_hash))
}

fn sol_to_json(v: &SolValue) -> serde_json::Value {
    match v {
        SolValue::Null => serde_json::Value::Null,
        SolValue::Bool(b) => serde_json::Value::Bool(*b),
        SolValue::Int(i) => json!(i),
        SolValue::Float(f) => json!(f),
        SolValue::Str(s) => json!(s),
        SolValue::List(xs) => serde_json::Value::Array(xs.iter().map(sol_to_json).collect()),
        SolValue::Map(m) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in m {
                obj.insert(k.clone(), sol_to_json(v));
            }
            serde_json::Value::Object(obj)
        }
    }
}

fn json_to_sol(v: &serde_json::Value) -> Result<SolValue, ErrV1> {
    crate::json_from(v).map_err(|e| ErrV1::new(ReasonCode::Type, "json_to_sol", e.to_string()))
}

/// Deterministic Conductor decision for bootstrap (no model).
///
/// Aligned with agent `select_starter_harness` so Phase 4.4 shadow comparison is meaningful.
/// `SpawnAverage` is OS-only (stdlib compound) when the utterance names averaging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeterministicConductorDecision {
    /// Alias kept for older call sites / greeting-specific traces.
    QuickReplyGreeting,
    QuickReply,
    UnderstandIntent,
    WaitForUser,
    /// Same as WaitForUser (legacy name).
    SpawnWaitForUser,
    MemoryAttach,
    SpawnAverage,
    Escalate,
}

pub fn decide_deterministic(event_type: &str, text: &str) -> DeterministicConductorDecision {
    if event_type != "user.message" {
        return DeterministicConductorDecision::Escalate;
    }
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return DeterministicConductorDecision::QuickReply;
    }
    // OS-specific compound before generic escalate.
    if t.contains("average") || t.contains("mean of") {
        return DeterministicConductorDecision::SpawnAverage;
    }
    // Explicit wait / clarify (agent parity).
    if t.contains("wait")
        || t.contains("hold on")
        || t.contains("what do you need")
        || t.contains("ask me")
    {
        return DeterministicConductorDecision::WaitForUser;
    }
    // Memory-ish.
    if t.contains("remember")
        || t.contains("recall")
        || t.contains("what did i say")
        || t.contains("from earlier")
    {
        return DeterministicConductorDecision::MemoryAttach;
    }
    // Tool-shaped intents → escalate to installed tool harness / cold path.
    // (Promoted `tool.send_otp` / `workflow.confirm_then_act` are selected by host after escalate.)
    const EFFECT_HINTS: &[&str] = &[
        "send otp",
        "login",
        "log in",
        "verify otp",
        "create job",
        "list jobs",
        "delete",
        "cancel",
        "pay",
        "book",
        "confirm",
    ];
    if EFFECT_HINTS.iter().any(|h| t.contains(h)) {
        return DeterministicConductorDecision::Escalate;
    }
    // Ambiguous / deep task → understand first.
    if t.len() > 80
        || t.contains('?')
            && (t.contains("how") || t.contains("why") || t.contains("which") || t.contains("should"))
        || t.contains("help me")
        || t.contains("i want to")
        || t.contains("i need to")
    {
        return DeterministicConductorDecision::UnderstandIntent;
    }
    // Greetings / tiny chat.
    if t.split_whitespace().count() <= 4
        || ["hi", "hello", "hey", "thanks", "thank you", "ok", "okay", "yes", "no"]
            .iter()
            .any(|w| t == *w || t.starts_with(&format!("{w} ")))
    {
        return DeterministicConductorDecision::QuickReply;
    }
    DeterministicConductorDecision::UnderstandIntent
}

/// Sol program for conductor.root deterministic routes (decision recorded via Const + Branch chain).
///
/// Input bag: `{ "utterance": str, "event_type": str, "route": str }`
/// where `route` is precomputed by [`decide_deterministic`] (kernel remains deterministic;
/// selection may live outside Sol until model-validated select ships).
pub fn conductor_root_program_json() -> serde_json::Value {
    // Nested Branch chain over precomputed `route` (closed Sol; no Switch sugar required).
    json!({
        "nid": "root",
        "op": "Seq",
        "steps": [
            {
                "nid": "hold_route",
                "op": "Call",
                "id": "compute.hold@1",
                "args": { "v": { "pull": "route" } },
                "into": "decision"
            },
            {
                "nid": "route_switch",
                "op": "Branch",
                "pred": {
                    "fn": "eq",
                    "args": [{ "pull": "decision" }, { "lit": "quick_reply" }]
                },
                "then": {
                    "nid": "say_hi",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": { "text": { "lit": "Hey! I can help you." } },
                    "into": "reply"
                },
                "else": {
                    "nid": "maybe_understand",
                    "op": "Branch",
                    "pred": {
                        "fn": "eq",
                        "args": [{ "pull": "decision" }, { "lit": "understand_intent" }]
                    },
                    "then": {
                        "nid": "ack_intent",
                        "op": "Call",
                        "id": "express.say@1",
                        "args": { "text": { "lit": "Spawning understand_intent" } },
                        "into": "reply"
                    },
                    "else": {
                        "nid": "maybe_average",
                        "op": "Branch",
                        "pred": {
                            "fn": "eq",
                            "args": [{ "pull": "decision" }, { "lit": "spawn_average" }]
                        },
                        "then": {
                            "nid": "ack_avg",
                            "op": "Call",
                            "id": "express.say@1",
                            "args": { "text": { "lit": "Spawning workflow.average" } },
                            "into": "reply"
                        },
                        "else": {
                            "nid": "maybe_wait",
                            "op": "Branch",
                            "pred": {
                                "fn": "eq",
                                "args": [{ "pull": "decision" }, { "lit": "spawn_wait" }]
                            },
                            "then": {
                                "nid": "ack_wait",
                                "op": "Call",
                                "id": "express.say@1",
                                "args": { "text": { "lit": "Spawning wait_for_user" } },
                                "into": "reply"
                            },
                            "else": {
                                "nid": "maybe_memory",
                                "op": "Branch",
                                "pred": {
                                    "fn": "eq",
                                    "args": [{ "pull": "decision" }, { "lit": "memory_attach" }]
                                },
                                "then": {
                                    "nid": "ack_mem",
                                    "op": "Call",
                                    "id": "express.say@1",
                                    "args": { "text": { "lit": "Spawning memory_attach" } },
                                    "into": "reply"
                                },
                                "else": {
                                    "nid": "escalate",
                                    "op": "Call",
                                    "id": "express.say@1",
                                    "args": { "text": { "lit": "Escalate: no deterministic route" } },
                                    "into": "reply"
                                }
                            }
                        }
                    }
                }
            }
        ]
    })
}

/// Run conductor.root with deterministic pre-route.
pub fn run_conductor_root_deterministic(
    utterance: &str,
    event_type: &str,
) -> Result<(DeterministicConductorDecision, SolValue, String), ErrV1> {
    let decision = decide_deterministic(event_type, utterance);
    let route = match decision {
        DeterministicConductorDecision::QuickReplyGreeting
        | DeterministicConductorDecision::QuickReply => "quick_reply",
        DeterministicConductorDecision::UnderstandIntent => "understand_intent",
        DeterministicConductorDecision::SpawnWaitForUser
        | DeterministicConductorDecision::WaitForUser => "spawn_wait",
        DeterministicConductorDecision::MemoryAttach => "memory_attach",
        DeterministicConductorDecision::SpawnAverage => "spawn_average",
        DeterministicConductorDecision::Escalate => "escalate",
    };
    let bag = SolValue::map([
        ("utterance", SolValue::str(utterance)),
        ("event_type", SolValue::str(event_type)),
        ("route", SolValue::str(route)),
    ]);
    let mut registry = registry_with_p0_stdlib();
    let (out, hash) = run_pure_sol(
        &conductor_root_program_json().to_string(),
        bag,
        &mut registry,
    )?;
    let _ = workflow_average_program_json; // keep related surface linked
    Ok((decision, out, hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn average_via_join_matches_sequential() {
        let mut tree = ProcessTree::new("t", "u");
        let (_root, bag, hash) = workflow_average_via_join(&mut tree, &[2, 4, 6]).unwrap();
        let avg = bag.as_map().and_then(|m| m.get("average")).cloned();
        assert_eq!(avg, Some(SolValue::Int(4)));
        assert_eq!(hash, value_hash(&bag));
        assert_eq!(tree.instance_count(), 3); // root + 2 children
    }

    #[test]
    fn average_via_join_empty_errors() {
        let mut tree = ProcessTree::new("t", "u");
        let err = workflow_average_via_join(&mut tree, &[]).unwrap_err();
        assert!(err.detail.contains("EmptyInput") || err.detail.contains("empty"));
    }

    #[test]
    fn conductor_root_greeting_is_deterministic() {
        let (d, bag, _) =
            run_conductor_root_deterministic("hello", "user.message").unwrap();
        assert!(matches!(
            d,
            DeterministicConductorDecision::QuickReply
                | DeterministicConductorDecision::QuickReplyGreeting
        ));
        let text = bag
            .as_map()
            .and_then(|m| m.get("reply"))
            .and_then(|r| r.as_map())
            .and_then(|m| m.get("text"))
            .cloned();
        assert_eq!(text, Some(SolValue::str("Hey! I can help you.")));
    }

    #[test]
    fn conductor_root_average_route() {
        let (d, bag, _) =
            run_conductor_root_deterministic("please average these numbers", "user.message")
                .unwrap();
        assert_eq!(d, DeterministicConductorDecision::SpawnAverage);
        let text = bag
            .as_map()
            .and_then(|m| m.get("reply"))
            .and_then(|r| r.as_map())
            .and_then(|m| m.get("text"))
            .and_then(|v| match v {
                SolValue::Str(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");
        assert!(text.contains("workflow.average"));
    }
}
