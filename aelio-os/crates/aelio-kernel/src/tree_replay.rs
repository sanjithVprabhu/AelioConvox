//! Phase 2.9 — process-tree execution records and bag-hash replay.
//!
//! A tree run captures each pure child program's ledger + final bag_hash, then
//! [`replay_tree`] re-executes every child via kernel [`crate::replay`] and
//! verifies join composition still matches.

use crate::compile;
use crate::driver::replay;
use crate::error::{ErrV1, ReasonCode};
use crate::harness_syscalls::{
    collection_count_program_json, math_divide_program_json, math_sum_program_json,
};
use crate::ledger::Ledger;
use crate::os_contract::{JoinPolicyV1, ResultEnvelopeV1, VersionPinV1};
use crate::process_tree::ProcessTree;
use crate::registry::Registry;
use crate::stdlib_targets::registry_with_p0_stdlib;
use aelio_sol::SolValue;
use serde_json::json;

/// One child's captured pure execution.
#[derive(Debug, Clone)]
pub struct ChildExecutionRecord {
    pub instance_id: String,
    pub nid: String,
    pub program_json: String,
    pub initial_bag: SolValue,
    pub final_bag: SolValue,
    pub bag_hash: String,
    pub ledger: Ledger,
}

/// Full tree record for `workflow.average` style composition.
#[derive(Debug, Clone)]
pub struct TreeExecutionRecord {
    pub root_instance_id: String,
    pub join_id: String,
    pub children: Vec<ChildExecutionRecord>,
    pub join_terminal: ResultEnvelopeV1,
    pub root_bag: SolValue,
    pub root_bag_hash: String,
    pub divide_program_json: String,
    pub divide_initial_bag: SolValue,
    pub divide_ledger: Ledger,
}

#[derive(Debug, Clone)]
pub struct TreeReplayReport {
    pub child_hashes: Vec<(String, String)>,
    pub root_bag_hash: String,
    pub matched: bool,
}

/// Run pure Sol and return bag, hash, and ledger for later replay.
pub fn run_pure_sol_recorded(
    program_json: &str,
    bag: SolValue,
    registry: &mut Registry,
) -> Result<(SolValue, String, Ledger), ErrV1> {
    let program = compile(program_json)?;
    let mut inst = crate::Instance::new(program, registry);
    match inst.start(bag)? {
        crate::TurnOutcome::Completed { bag, bag_hash } => {
            Ok((bag, bag_hash, inst.ledger().clone()))
        }
        crate::TurnOutcome::Parked(_) => Err(ErrV1::new(
            ReasonCode::Internal,
            "run_pure_sol_recorded",
            "pure program parked unexpectedly",
        )),
    }
}

/// Execute average via join, capturing full tree execution for replay.
pub fn workflow_average_recorded(
    tree: &mut ProcessTree,
    values: &[i64],
) -> Result<TreeExecutionRecord, ErrV1> {
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
    let mut children = Vec::new();

    let sum_prog = math_sum_program_json().to_string();
    let (sum_bag, sum_hash, sum_ledger) =
        run_pure_sol_recorded(&sum_prog, bag_in.clone(), &mut registry)?;
    let sum_val = sum_bag
        .as_map()
        .and_then(|m| m.get("sum"))
        .cloned()
        .ok_or_else(|| ErrV1::new(ReasonCode::Missing, "sum", "sum field missing"))?;
    tree.complete_ok(
        &kids[0],
        sol_to_json(&sum_val),
        Some(sum_bag.clone()),
        Some(sum_hash.clone()),
    )
    .map_err(|e| e.into_errv1("complete_sum"))?;
    children.push(ChildExecutionRecord {
        instance_id: kids[0].clone(),
        nid: "sum".into(),
        program_json: sum_prog,
        initial_bag: bag_in.clone(),
        final_bag: sum_bag,
        bag_hash: sum_hash,
        ledger: sum_ledger,
    });

    let count_prog = collection_count_program_json().to_string();
    let (count_bag, count_hash, count_ledger) =
        run_pure_sol_recorded(&count_prog, bag_in.clone(), &mut registry)?;
    let count_val = count_bag
        .as_map()
        .and_then(|m| m.get("count"))
        .cloned()
        .ok_or_else(|| ErrV1::new(ReasonCode::Missing, "count", "count field missing"))?;
    tree.complete_ok(
        &kids[1],
        sol_to_json(&count_val),
        Some(count_bag.clone()),
        Some(count_hash.clone()),
    )
    .map_err(|e| e.into_errv1("complete_count"))?;
    children.push(ChildExecutionRecord {
        instance_id: kids[1].clone(),
        nid: "count".into(),
        program_json: count_prog,
        initial_bag: bag_in,
        final_bag: count_bag,
        bag_hash: count_hash,
        ledger: count_ledger,
    });

    let join = tree
        .join(&join_id)
        .ok_or_else(|| ErrV1::new(ReasonCode::Missing, "join", "join missing"))?;
    assert!(matches!(join.policy, JoinPolicyV1::All));
    let term = join
        .terminal
        .clone()
        .ok_or_else(|| ErrV1::new(ReasonCode::Internal, "join", "join not terminal"))?;

    let (sum_json, count_json) = match &term {
        ResultEnvelopeV1::Ok { output, .. } => {
            let arr = output.as_array().ok_or_else(|| {
                ErrV1::new(ReasonCode::Type, "join_all", "expected ordered result array")
            })?;
            let mut sum = serde_json::Value::Null;
            let mut count = serde_json::Value::Null;
            for item in arr {
                match item.get("nid").and_then(|v| v.as_str()) {
                    Some("sum") => {
                        sum = item.get("output").cloned().unwrap_or(serde_json::Value::Null)
                    }
                    Some("count") => {
                        count = item.get("output").cloned().unwrap_or(serde_json::Value::Null)
                    }
                    _ => {}
                }
            }
            (sum, count)
        }
        ResultEnvelopeV1::Err { code, message, .. } => {
            return Err(ErrV1::new(
                ReasonCode::Type,
                "join_all",
                format!("{code}: {message}"),
            ));
        }
        ResultEnvelopeV1::Cancelled { reason } => {
            return Err(ErrV1::new(ReasonCode::Policy, "join_all", reason.clone()));
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
    let div_prog = math_divide_program_json().to_string();
    let (avg_bag, avg_hash, div_ledger) =
        run_pure_sol_recorded(&div_prog, divide_bag.clone(), &mut registry)?;
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

    Ok(TreeExecutionRecord {
        root_instance_id: root,
        join_id,
        children,
        join_terminal: term,
        root_bag: avg_bag,
        root_bag_hash: avg_hash,
        divide_program_json: div_prog,
        divide_initial_bag: divide_bag,
        divide_ledger: div_ledger,
    })
}

/// Replay every child ledger + divide step; verify bag hashes match the record.
pub fn replay_tree(record: &TreeExecutionRecord) -> Result<TreeReplayReport, ErrV1> {
    let mut child_hashes = Vec::new();
    for child in &record.children {
        let program = compile(&child.program_json)?;
        let recomputed = replay(&program, &child.ledger, child.initial_bag.clone())?;
        if recomputed != child.bag_hash {
            return Err(ErrV1::new(
                ReasonCode::Internal,
                "tree_replay",
                format!(
                    "child `{}` bag_hash divergence: live={} replay={}",
                    child.nid, child.bag_hash, recomputed
                ),
            ));
        }
        child_hashes.push((child.nid.clone(), recomputed));
    }

    let div_prog = compile(&record.divide_program_json)?;
    let root_recomputed = replay(
        &div_prog,
        &record.divide_ledger,
        record.divide_initial_bag.clone(),
    )?;
    if root_recomputed != record.root_bag_hash {
        return Err(ErrV1::new(
            ReasonCode::Internal,
            "tree_replay",
            format!(
                "root bag_hash divergence: live={} replay={}",
                record.root_bag_hash, root_recomputed
            ),
        ));
    }

    // Sanity: re-run live composition still matches (bit-stable pure path).
    let mut tree = ProcessTree::new("replay-check", "subject");
    let live = workflow_average_via_join_from_values(&mut tree, extract_values(record)?)?;
    if live.2 != record.root_bag_hash {
        return Err(ErrV1::new(
            ReasonCode::Internal,
            "tree_replay",
            "fresh live re-run bag_hash mismatch",
        ));
    }

    Ok(TreeReplayReport {
        child_hashes,
        root_bag_hash: root_recomputed,
        matched: true,
    })
}

fn workflow_average_via_join_from_values(
    tree: &mut ProcessTree,
    values: Vec<i64>,
) -> Result<(String, SolValue, String), ErrV1> {
    crate::workflow_average_via_join(tree, &values)
}

fn extract_values(record: &TreeExecutionRecord) -> Result<Vec<i64>, ErrV1> {
    let bag = &record.children[0].initial_bag;
    let list = bag
        .as_map()
        .and_then(|m| m.get("values"))
        .and_then(|v| match v {
            SolValue::List(xs) => Some(xs),
            _ => None,
        })
        .ok_or_else(|| ErrV1::new(ReasonCode::Missing, "values", "missing values list"))?;
    list.iter()
        .map(|v| match v {
            SolValue::Int(n) => Ok(*n),
            _ => Err(ErrV1::new(ReasonCode::Type, "values", "expected ints")),
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness_syscalls::run_pure_sol;
    use crate::process_tree::ProcessTree;
    use aelio_sol::value_hash;

    #[test]
    fn average_tree_replays_with_bag_hash_identity() {
        let mut tree = ProcessTree::new("t", "u");
        let record = workflow_average_recorded(&mut tree, &[2, 4, 6, 8]).unwrap();
        assert_eq!(
            record
                .root_bag
                .as_map()
                .and_then(|m| m.get("average"))
                .cloned(),
            Some(SolValue::Int(5))
        );
        let report = replay_tree(&record).unwrap();
        assert!(report.matched);
        assert_eq!(report.root_bag_hash, record.root_bag_hash);
        assert_eq!(report.child_hashes.len(), 2);
        assert_eq!(report.root_bag_hash, value_hash(&record.root_bag));
    }

    #[test]
    fn pure_recorded_matches_run_pure_sol() {
        let mut reg = registry_with_p0_stdlib();
        let bag = SolValue::map([(
            "values",
            SolValue::List(vec![SolValue::Int(1), SolValue::Int(2)]),
        )]);
        let (b1, h1) = run_pure_sol(
            &math_sum_program_json().to_string(),
            bag.clone(),
            &mut reg,
        )
        .unwrap();
        let mut reg2 = registry_with_p0_stdlib();
        let (b2, h2, led) =
            run_pure_sol_recorded(&math_sum_program_json().to_string(), bag.clone(), &mut reg2)
                .unwrap();
        assert_eq!(h1, h2);
        assert_eq!(b1, b2);
        let prog = compile(&math_sum_program_json().to_string()).unwrap();
        assert_eq!(replay(&prog, &led, bag).unwrap(), h1);
    }
}
