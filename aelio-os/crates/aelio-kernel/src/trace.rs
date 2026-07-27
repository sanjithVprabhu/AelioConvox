//! Trace viewer (§26): **op-by-op bag diffs straight from the ledger** — no instrumentation beyond
//! what §12.2 already mandates. Each effect entry records the value that landed in the bag at that
//! op's declared write path, so the diff at every nid is reconstructable from the ledger alone
//! (`call_result`/`read_result` → `into`; `resume` → the park's `into`).

use crate::instr::{Kind, Node};
use crate::ledger::{Entry, Ledger};
use aelio_sol::SolValue;
use std::collections::BTreeMap;

/// One human-readable trace line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEntry {
    pub seq: u64,
    pub nid: Option<String>,
    pub kind: String,
    /// A one-line "what happened" — for effects, the bag write it produced.
    pub summary: String,
}

/// Render a per-entry trace of a turn ledger against its program. Reads only the ledger + the
/// program's static write paths (§26).
pub fn render(program: &Node, ledger: &Ledger) -> Vec<TraceEntry> {
    let writes = nid_write_paths(program);
    ledger.entries().iter().map(|e| line(e, &writes)).collect()
}

/// Pretty multi-line string for the CLI trace viewer.
pub fn render_string(program: &Node, ledger: &Ledger) -> String {
    render(program, ledger)
        .into_iter()
        .map(|t| {
            let nid = t.nid.as_deref().unwrap_or("-");
            format!("[{:>3}] {:<16} {:<10} {}", t.seq, t.kind, nid, t.summary)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn line(e: &Entry, writes: &BTreeMap<String, String>) -> TraceEntry {
    let payload = &e.payload;
    let summary = match e.kind.as_str() {
        "turn_start" => format!(
            "── turn start ({}) ──",
            field_str(payload, "trigger").unwrap_or_default()
        ),
        "turn_end" => match field_str(payload, "bag_hash") {
            Some(h) => format!("── turn end · bag_hash={} ──", short(&h)),
            None => format!(
                "── turn end ({}) ──",
                field_str(payload, "outcome").unwrap_or_default()
            ),
        },
        "call_result" | "read_result" => {
            let into = e
                .nid
                .as_deref()
                .and_then(|n| writes.get(n))
                .cloned()
                .unwrap_or_else(|| "?".into());
            let out = field(payload, "output")
                .map(summarize_value)
                .unwrap_or_default();
            format!("{into} := {out}")
        }
        "call_intent" => format!(
            "intent {}",
            field_str(payload, "target").unwrap_or_default()
        ),
        "resume" => {
            let into = e
                .nid
                .as_deref()
                .and_then(|n| writes.get(n))
                .cloned()
                .unwrap_or_else(|| "?".into());
            let wake = field(payload, "wake")
                .map(summarize_value)
                .unwrap_or_default();
            format!("resume · {into} := {wake}")
        }
        "park" => format!(
            "parked at {}",
            field_str(payload, "park_nid").unwrap_or_default()
        ),
        "once_intent" => "once: first execution claimed".into(),
        "once_result" => "once: recorded".into(),
        "nondet_value" => format!(
            "{} = {}",
            field_str(payload, "source").unwrap_or_default(),
            field(payload, "value")
                .map(summarize_value)
                .unwrap_or_default()
        ),
        other => other.to_string(),
    };
    TraceEntry {
        seq: e.seq,
        nid: e.nid.clone(),
        kind: e.kind.clone(),
        summary,
    }
}

/// Map every op nid to its declared write path string (the "into" of Call/Park/Map/Filter/Try).
fn nid_write_paths(program: &Node) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    collect(program, &mut map);
    map
}

fn collect(node: &Node, map: &mut BTreeMap<String, String>) {
    let into = match &node.kind {
        Kind::Call { into, .. }
        | Kind::Map { into, .. }
        | Kind::Filter { into, .. }
        | Kind::Try { err_into: into, .. } => Some(into),
        Kind::Park { into: Some(p), .. } => Some(p),
        Kind::Tee { side_root, .. } => Some(side_root),
        _ => None,
    };
    if let Some(p) = into {
        map.insert(node.nid.clone(), path_string(p));
    }
    for child in children(node) {
        collect(child, map);
    }
}

fn children(node: &Node) -> Vec<&Node> {
    match &node.kind {
        Kind::Seq(steps) | Kind::Fallback(steps) => steps.iter().collect(),
        Kind::Let { body, .. }
        | Kind::Loop { body, .. }
        | Kind::Guard { body, .. }
        | Kind::Budget { body, .. }
        | Kind::Timeout { body, .. }
        | Kind::Once { body, .. }
        | Kind::Map { body, .. } => vec![body],
        Kind::Branch { then, els, .. } => {
            let mut v = vec![then.as_ref()];
            if let Some(e) = els {
                v.push(e);
            }
            v
        }
        Kind::Try {
            body,
            catch,
            finally,
            ..
        } => {
            let mut v = vec![body.as_ref()];
            v.extend(catch.iter().map(|(_, n)| n));
            if let Some(f) = finally {
                v.push(f);
            }
            v
        }
        Kind::Tee { body, side, .. } => vec![body, side],
        _ => vec![],
    }
}

fn path_string(p: &aelio_sol::Path) -> String {
    let mut s = String::new();
    for (i, seg) in p.segments().iter().enumerate() {
        match seg {
            aelio_sol::Segment::Key(k) => {
                if i > 0 {
                    s.push('.');
                }
                s.push_str(k);
            }
            aelio_sol::Segment::Index(n) => s.push_str(&format!("[{n}]")),
        }
    }
    s
}

fn field<'a>(v: &'a SolValue, key: &str) -> Option<&'a SolValue> {
    v.as_map().and_then(|m| m.get(key))
}
fn field_str(v: &SolValue, key: &str) -> Option<String> {
    match field(v, key) {
        Some(SolValue::Str(s)) => Some(s.clone()),
        _ => None,
    }
}
fn summarize_value(v: &SolValue) -> String {
    short(&aelio_sol::canonical_string(v))
}
fn short(s: &str) -> String {
    if s.chars().count() > 60 {
        format!("{}…", s.chars().take(60).collect::<String>())
    } else {
        s.to_string()
    }
}
