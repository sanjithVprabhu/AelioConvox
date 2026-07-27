//! Instruction AST + Expr grammar (Appendix E), parsed from instruction JSON. Every node carries a
//! `nid` (§8.1). Unknown op/field ⇒ plan-time reject (the Planner enforces; the parser rejects
//! unknown ops and shapes here).

use crate::error::{ErrV1, ReasonCode};
use crate::json;
use aelio_sol::{Path, SolValue};
use serde_json::Value as J;

/// A planned instruction node: stable identity + its op.
#[derive(Debug, Clone)]
pub struct Node {
    pub nid: String,
    pub kind: Kind,
}

/// Expression grammar (App E): literal, bag read, or a pure Compute fn application.
#[derive(Debug, Clone)]
pub enum Expr {
    Lit(SolValue),
    Pull(Path),
    Fn { op: String, args: Vec<Expr> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardCheck {
    Entry,
    Exit,
    Both,
    Each,
}

#[derive(Debug, Clone)]
pub enum Until {
    Event,
    Ttl { ms: u64 },
    Instant { at: String },
}

/// The 17 Control ops (§8) — L0-A.
#[derive(Debug, Clone)]
pub enum Kind {
    Const(SolValue),
    Identity,
    Seq(Vec<Node>),
    Let { bindings: Vec<(String, Expr)>, body: Box<Node> },
    Branch { pred: Expr, then: Box<Node>, els: Option<Box<Node>> },
    Loop { while_: Expr, body: Box<Node>, max_iter: u64 },
    Try { body: Box<Node>, catch: Vec<(String, Node)>, err_into: Path, finally: Option<Box<Node>> },
    Fallback(Vec<Node>),
    Guard { invariant: Expr, body: Box<Node>, on_violation: Option<Box<Node>>, check: GuardCheck },
    Budget { body: Box<Node>, calls: Option<u64>, tokens: Option<u64>, ms: Option<u64> },
    Timeout { body: Box<Node>, ms: u64 },
    Once { body: Box<Node>, idem_key: Option<Vec<Expr>> },
    Park { until: Until, into: Option<Path> },
    Tee { body: Box<Node>, side: Box<Node>, side_root: Path },
    Map { over: Path, imports: Vec<(String, Path)>, body: Box<Node>, into: Path, max_items: u64 },
    Filter { over: Path, pred: Expr, into: Path, max_items: u64 },
    Call { id: String, args: Vec<(String, Expr)>, into: Path },
}

/// Parse errors are `Shape` (§11) with a nid where known.
fn shape(nid: &str, why: impl Into<String>) -> ErrV1 {
    ErrV1::new(ReasonCode::Shape, nid, why)
}

pub fn parse_node(j: &J) -> Result<Node, ErrV1> {
    let obj = j.as_object().ok_or_else(|| shape("?", "instruction must be a JSON object"))?;
    let nid = obj
        .get("nid")
        .and_then(J::as_str)
        .ok_or_else(|| shape("?", "instruction missing string `nid` (§8.1)"))?
        .to_string();
    let op = obj
        .get("op")
        .and_then(J::as_str)
        .ok_or_else(|| shape(&nid, "instruction missing string `op`"))?;

    // Closed-schema (App E, §3): unknown fields ⇒ reject. A hallucinated/misspelled field (e.g.
    // `arg` for `args`) must fail loud, never silently default. Unknown ops fall through to the
    // match's own rejection below.
    check_known_fields(&nid, op, obj)?;

    let kind = match op {
        "Const" => Kind::Const(sol(&nid, obj.get("v"))?),
        "Identity" => Kind::Identity,
        "Seq" => Kind::Seq(parse_steps(&nid, obj.get("steps"))?),
        "Let" => {
            let bindings = obj
                .get("bindings")
                .and_then(J::as_array)
                .ok_or_else(|| shape(&nid, "Let.bindings must be an array"))?
                .iter()
                .map(|b| {
                    let key = b.get("key").and_then(J::as_str).ok_or_else(|| shape(&nid, "binding.key"))?;
                    let value = parse_expr(&nid, b.get("value"))?;
                    Ok((key.to_string(), value))
                })
                .collect::<Result<Vec<_>, ErrV1>>()?;
            Kind::Let { bindings, body: boxed(&nid, obj.get("body"))? }
        }
        "Branch" => Kind::Branch {
            pred: parse_expr(&nid, obj.get("pred"))?,
            then: boxed(&nid, obj.get("then"))?,
            els: obj.get("else").map(|e| parse_node(e).map(Box::new)).transpose()?,
        },
        "Loop" => Kind::Loop {
            while_: parse_expr(&nid, obj.get("while"))?,
            body: boxed(&nid, obj.get("body"))?,
            max_iter: pos_int(&nid, obj.get("max_iter"), "max_iter")?,
        },
        "Try" => {
            let catch = obj
                .get("catch")
                .and_then(J::as_object)
                .ok_or_else(|| shape(&nid, "Try.catch must be an object of prefix→Instr"))?
                .iter()
                .map(|(k, v)| Ok((k.clone(), parse_node(v)?)))
                .collect::<Result<Vec<_>, ErrV1>>()?;
            Kind::Try {
                body: boxed(&nid, obj.get("body"))?,
                catch,
                err_into: path(&nid, obj.get("err_into"), "err_into")?,
                finally: obj.get("finally").map(|f| parse_node(f).map(Box::new)).transpose()?,
            }
        }
        "Fallback" => {
            let steps = parse_steps(&nid, obj.get("steps"))?;
            if steps.len() < 2 {
                return Err(shape(&nid, "Fallback needs ≥2 steps (App E)"));
            }
            Kind::Fallback(steps)
        }
        "Guard" => Kind::Guard {
            invariant: parse_expr(&nid, obj.get("invariant"))?,
            body: boxed(&nid, obj.get("body"))?,
            on_violation: obj.get("on_violation").map(|v| parse_node(v).map(Box::new)).transpose()?,
            check: match obj.get("check").and_then(J::as_str) {
                None | Some("both") => GuardCheck::Both,
                Some("entry") => GuardCheck::Entry,
                Some("exit") => GuardCheck::Exit,
                Some("each") => GuardCheck::Each,
                Some(other) => return Err(shape(&nid, format!("bad Guard.check `{other}`"))),
            },
        },
        "Budget" => {
            let calls = opt_pos_int(&nid, obj.get("calls"), "calls")?;
            let tokens = opt_pos_int(&nid, obj.get("tokens"), "tokens")?;
            let ms = opt_pos_int(&nid, obj.get("ms"), "ms")?;
            if calls.is_none() && tokens.is_none() && ms.is_none() {
                return Err(shape(&nid, "Budget needs ≥1 meter (App E)"));
            }
            Kind::Budget { body: boxed(&nid, obj.get("body"))?, calls, tokens, ms }
        }
        "Timeout" => Kind::Timeout {
            body: boxed(&nid, obj.get("body"))?,
            ms: pos_int(&nid, obj.get("ms"), "ms")?,
        },
        "Once" => Kind::Once {
            body: boxed(&nid, obj.get("body"))?,
            idem_key: match obj.get("idem_key") {
                None => None,
                Some(k) => {
                    let arr = k
                        .get("template")
                        .and_then(J::as_array)
                        .ok_or_else(|| shape(&nid, "Once.idem_key must be {template:[Expr,...]}"))?;
                    Some(arr.iter().map(|e| parse_expr(&nid, Some(e))).collect::<Result<_, _>>()?)
                }
            },
        },
        "Park" => {
            let until = obj.get("until").and_then(J::as_object).ok_or_else(|| shape(&nid, "Park.until"))?;
            let until = match until.get("kind").and_then(J::as_str) {
                Some("event") => Until::Event,
                Some("ttl") => Until::Ttl { ms: pos_int(&nid, until.get("ms"), "until.ms")? },
                Some("instant") => Until::Instant {
                    at: until.get("at").and_then(J::as_str).ok_or_else(|| shape(&nid, "instant.at"))?.into(),
                },
                _ => return Err(shape(&nid, "Park.until.kind ∈ {event,ttl,instant}")),
            };
            Kind::Park {
                until,
                into: obj.get("into").map(|p| path(&nid, Some(p), "into")).transpose()?,
            }
        }
        "Tee" => Kind::Tee {
            body: boxed(&nid, obj.get("body"))?,
            side: boxed(&nid, obj.get("side"))?,
            side_root: path(&nid, obj.get("side_root"), "side_root")?,
        },
        "Map" => Kind::Map {
            over: path(&nid, obj.get("over"), "over")?,
            imports: parse_imports(&nid, obj.get("imports"))?,
            body: boxed(&nid, obj.get("body"))?,
            into: path(&nid, obj.get("into"), "into")?,
            max_items: pos_int(&nid, obj.get("max_items"), "max_items")?,
        },
        "Filter" => Kind::Filter {
            over: path(&nid, obj.get("over"), "over")?,
            pred: parse_expr(&nid, obj.get("pred"))?,
            into: path(&nid, obj.get("into"), "into")?,
            max_items: pos_int(&nid, obj.get("max_items"), "max_items")?,
        },
        "Call" => Kind::Call {
            id: obj.get("id").and_then(J::as_str).ok_or_else(|| shape(&nid, "Call.id"))?.into(),
            args: parse_args(&nid, obj.get("args"))?,
            into: path(&nid, obj.get("into"), "into")?,
        },
        other => return Err(shape(&nid, format!("unknown op `{other}` (closed set §8)"))),
    };

    Ok(Node { nid, kind })
}

/// Allowed fields per op (App E), beyond the universal `nid`/`op`. Unknown op ⇒ empty allow-list
/// here; the main match rejects it. Any object key outside the op's set is a closed-schema violation.
fn check_known_fields(nid: &str, op: &str, obj: &serde_json::Map<String, J>) -> Result<(), ErrV1> {
    let allowed: &[&str] = match op {
        "Const" => &["v"],
        "Identity" => &[],
        "Seq" | "Fallback" => &["steps"],
        "Let" => &["bindings", "body"],
        "Branch" => &["pred", "then", "else"],
        "Loop" => &["while", "body", "max_iter"],
        "Try" => &["body", "catch", "err_into", "finally"],
        "Guard" => &["invariant", "body", "on_violation", "check"],
        "Budget" => &["body", "calls", "tokens", "ms"],
        "Timeout" => &["body", "ms"],
        "Once" => &["body", "idem_key"],
        "Park" => &["until", "into"],
        "Tee" => &["body", "side", "side_root"],
        "Map" => &["over", "imports", "body", "into", "max_items"],
        "Filter" => &["over", "pred", "into", "max_items"],
        "Call" => &["id", "args", "into"],
        _ => return Ok(()), // unknown op — let the main match reject it
    };
    for key in obj.keys() {
        if key == "nid" || key == "op" {
            continue;
        }
        if !allowed.contains(&key.as_str()) {
            return Err(shape(nid, format!("unknown field `{key}` on op `{op}` (closed schema, App E)")));
        }
    }
    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────────────────────────────

fn parse_steps(nid: &str, j: Option<&J>) -> Result<Vec<Node>, ErrV1> {
    j.and_then(J::as_array)
        .ok_or_else(|| shape(nid, "expected `steps` array"))?
        .iter()
        .map(parse_node)
        .collect()
}

fn parse_imports(nid: &str, j: Option<&J>) -> Result<Vec<(String, Path)>, ErrV1> {
    match j {
        None => Ok(vec![]),
        Some(obj) => obj
            .as_object()
            .ok_or_else(|| shape(nid, "Map.imports must be an object"))?
            .iter()
            .map(|(k, v)| {
                let p = Path::parse(v.as_str().ok_or_else(|| shape(nid, "import path must be string"))?)
                    .map_err(|e| shape(nid, format!("import path: {e}")))?;
                Ok((k.clone(), p))
            })
            .collect(),
    }
}

fn parse_args(nid: &str, j: Option<&J>) -> Result<Vec<(String, Expr)>, ErrV1> {
    match j {
        None => Ok(vec![]),
        Some(obj) => obj
            .as_object()
            .ok_or_else(|| shape(nid, "Call.args must be an object"))?
            .iter()
            .map(|(k, v)| Ok((k.clone(), parse_expr(nid, Some(v))?)))
            .collect(),
    }
}

pub fn parse_expr(nid: &str, j: Option<&J>) -> Result<Expr, ErrV1> {
    let obj = j
        .and_then(J::as_object)
        .ok_or_else(|| shape(nid, "expression must be a JSON object"))?;
    if let Some(lit) = obj.get("lit") {
        return Ok(Expr::Lit(json::from_json(lit).map_err(|e| shape(nid, format!("lit: {e}")))?));
    }
    if let Some(p) = obj.get("pull") {
        let p = p.as_str().ok_or_else(|| shape(nid, "pull must be a string path"))?;
        return Ok(Expr::Pull(Path::parse(p).map_err(|e| shape(nid, format!("pull path: {e}")))?));
    }
    if let Some(op) = obj.get("fn").and_then(J::as_str) {
        let args = obj
            .get("args")
            .and_then(J::as_array)
            .ok_or_else(|| shape(nid, "fn.args must be an array"))?
            .iter()
            .map(|a| parse_expr(nid, Some(a)))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Expr::Fn { op: op.to_string(), args });
    }
    Err(shape(nid, "expression must be one of {lit, pull, fn}"))
}

fn boxed(nid: &str, j: Option<&J>) -> Result<Box<Node>, ErrV1> {
    Ok(Box::new(parse_node(j.ok_or_else(|| shape(nid, "missing child instruction"))?)?))
}

fn sol(nid: &str, j: Option<&J>) -> Result<SolValue, ErrV1> {
    json::from_json(j.ok_or_else(|| shape(nid, "Const.v required"))?)
        .map_err(|e| shape(nid, format!("Const.v: {e}")))
}

fn path(nid: &str, j: Option<&J>, field: &str) -> Result<Path, ErrV1> {
    let s = j
        .and_then(J::as_str)
        .ok_or_else(|| shape(nid, format!("`{field}` must be a literal path string")))?;
    Path::parse(s).map_err(|e| shape(nid, format!("`{field}` path: {e}")))
}

fn pos_int(nid: &str, j: Option<&J>, field: &str) -> Result<u64, ErrV1> {
    let n = j.and_then(J::as_u64).ok_or_else(|| shape(nid, format!("`{field}` must be a positive int")))?;
    if n == 0 {
        return Err(shape(nid, format!("`{field}` must be > 0 (App E)")));
    }
    Ok(n)
}

fn opt_pos_int(nid: &str, j: Option<&J>, field: &str) -> Result<Option<u64>, ErrV1> {
    match j {
        None => Ok(None),
        Some(_) => Ok(Some(pos_int(nid, j, field)?)),
    }
}
