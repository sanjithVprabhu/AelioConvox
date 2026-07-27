//! P2 surface sugar — `Switch` / `Retry` / `Pipe` — that **compiles down to core ops** (§4.2.4,
//! §11.5, §8). Kernel semantics stay singular: this is a pure JSON→JSON desugaring pass that runs
//! before the Planner, so the executor and ledger never see a sugar op. Prompt/flow decisions stay
//! in the ledgered core.
//!
//! - `Switch{on, cases, default}` → a nested `Branch` chain (`on == label` per case, else next).
//! - `Pipe{steps}` → `Seq{steps}` (stage sugar → sequential edits, §4.2.4).
//! - `Retry{body, max, retry_on}` → nested `Try` re-running the body on retryable codes (§11.5).

use serde_json::{json, Map, Value as J};

/// Recursively expand any sugar ops in an instruction tree into core ops. Idempotent on trees with
/// no sugar. Returns the desugared JSON (ready for `parse_node` + `plan`).
pub fn desugar(node: &J) -> Result<J, String> {
    let Some(obj) = node.as_object() else {
        return Ok(node.clone());
    };
    let nid = obj.get("nid").and_then(J::as_str).unwrap_or("?").to_string();
    match obj.get("op").and_then(J::as_str) {
        Some("Switch") => desugar_switch(&nid, obj),
        Some("Pipe") => {
            let steps = arr(obj, "steps")?;
            let out: Result<Vec<J>, String> = steps.iter().map(desugar).collect();
            Ok(json!({"nid": nid, "op": "Seq", "steps": out?}))
        }
        Some("Retry") => desugar_retry(&nid, obj),
        _ => desugar_children(node),
    }
}

/// `Switch{on, cases:{label:instr,...}, default}` → nested `Branch`.
fn desugar_switch(nid: &str, obj: &Map<String, J>) -> Result<J, String> {
    let on = obj.get("on").ok_or("Switch.on required")?;
    let cases = obj.get("cases").and_then(J::as_object).ok_or("Switch.cases object")?;
    let default = obj.get("default").ok_or("Switch.default required")?;

    // Fold cases into an else-chain terminating in the default.
    let mut chain = desugar(default)?;
    for (i, (label, instr)) in cases.iter().enumerate().rev() {
        let branch_nid = format!("{nid}__sw{i}");
        chain = json!({
            "nid": branch_nid,
            "op": "Branch",
            "pred": {"fn": "eq", "args": [on, {"lit": label}]},
            "then": desugar(instr)?,
            "else": chain
        });
    }
    Ok(chain)
}

/// `Retry{body, max, retry_on:[codes]}` → nested `Try` re-running the body up to `max` times on the
/// retryable codes. Innermost has no catch (propagates) — bounded by construction (§11.5, §8.4 G1).
fn desugar_retry(nid: &str, obj: &Map<String, J>) -> Result<J, String> {
    let body = desugar(obj.get("body").ok_or("Retry.body required")?)?;
    let max = obj.get("max").and_then(J::as_u64).filter(|&n| n >= 1).ok_or("Retry.max >= 1")?;
    let retry_on: Vec<String> = obj
        .get("retry_on")
        .and_then(J::as_array)
        .map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_string)).collect())
        .unwrap_or_else(|| vec!["Tool.Transient".into(), "Timeout".into()]);
    let err_into = format!("{nid}__retry_err");

    // Attempt `max` runs: attempt(k) = Try(body, catch{codes → attempt(k-1)}); attempt(1) = body.
    let mut current = body.clone();
    for k in 2..=max {
        let mut catch = Map::new();
        for code in &retry_on {
            catch.insert(code.clone(), current.clone());
        }
        current = json!({
            "nid": format!("{nid}__retry{k}"),
            "op": "Try",
            "body": body,
            "catch": catch,
            "err_into": err_into,
        });
    }
    Ok(current)
}

fn desugar_children(node: &J) -> Result<J, String> {
    let obj = node.as_object().unwrap();
    let mut out = obj.clone();
    for key in ["body", "then", "else", "side", "finally"] {
        if let Some(child) = obj.get(key) {
            out.insert(key.into(), desugar(child)?);
        }
    }
    if let Some(J::Array(steps)) = obj.get("steps") {
        let ds: Result<Vec<J>, String> = steps.iter().map(desugar).collect();
        out.insert("steps".into(), J::Array(ds?));
    }
    if let Some(J::Object(catch)) = obj.get("catch") {
        let mut nc = Map::new();
        for (k, v) in catch {
            nc.insert(k.clone(), desugar(v)?);
        }
        out.insert("catch".into(), J::Object(nc));
    }
    Ok(J::Object(out))
}

fn arr<'a>(obj: &'a Map<String, J>, key: &str) -> Result<&'a Vec<J>, String> {
    obj.get(key).and_then(J::as_array).ok_or(format!("expected `{key}` array"))
}
