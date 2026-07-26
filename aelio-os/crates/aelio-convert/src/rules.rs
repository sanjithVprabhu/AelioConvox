//! Conversion rule ops (§14) — the **closed, non-computational** set. Rules execute sequentially,
//! each rewriting the working Sol; **no rule references the rule list** and no rule's output feeds
//! rule *selection* (A14.4) — proven by construction here and by `rules_are_non_computational` in
//! the tests. This is what makes the §15 containment theorem hold: the worst a malicious payload can
//! do is produce a wrong *mapping*, never execution.
//!
//! Admission rule (§14): a rule op is admissible only if (a) pure, (b) total or fail-loud, (c)
//! meaning-free, (d) locale/environment-independent. `trim` qualifies; case folding does not.

use aelio_kernel::ReasonCode;
use aelio_sol::{Path, Segment, SolValue};
use serde_json::Value as J;
use std::collections::BTreeMap;

/// A failure at rule index `rule_index`; always `Convert.RuleFail` (§14), routed to `on_parse_fail`.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleFail {
    pub rule_index: usize,
    pub code: ReasonCode,
    pub detail: String,
}

/// The closed rule set (§14). Nothing else is constructible — the parser rejects unknown ops, so a
/// cold-path proposer can only ever emit these.
#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    Rename { from: Path, to: Path },
    Drop { path: Path },
    Keep { paths: Vec<Path> },
    /// Fabricating (§14): writes `v` iff `path` absent.
    Default { path: Path, v: SolValue },
    Cast { path: Path, to: String, mode: Option<String> },
    Wrap { path: Path, key: String },
    Unwrap { path: Path },
    MapEnum { path: Path, table: BTreeMap<String, SolValue> },
    PathCopy { from: Path, to: Path },
    /// Fabricating (§14): unconditional write.
    ConstSet { path: Path, v: SolValue },
    Trim { path: Path },
}

impl Rule {
    /// Fabricating rules mint unobserved data → the edge escalates to reviewed tier regardless of
    /// reach (§14 fabrication escalation). Computed from the rule, never a stored flag (F4).
    pub fn is_fabricating(&self) -> bool {
        matches!(self, Rule::Default { .. } | Rule::ConstSet { .. })
    }
}

/// Any rule in the set fabricates ⇒ the whole converter escalates (§14).
pub fn any_fabricating(rules: &[Rule]) -> bool {
    rules.iter().any(Rule::is_fabricating)
}

/// Parse a closed-JSON rule list (the cold-path proposer's only output surface). Unknown ops ⇒
/// reject (containment).
pub fn parse_rules(j: &J) -> Result<Vec<Rule>, String> {
    j.as_array()
        .ok_or("rules must be an array")?
        .iter()
        .map(parse_rule)
        .collect()
}

fn parse_rule(j: &J) -> Result<Rule, String> {
    let o = j.as_object().ok_or("rule must be an object")?;
    let op = o.get("op").and_then(J::as_str).ok_or("rule missing `op`")?;
    let path = |k: &str| -> Result<Path, String> {
        Path::parse(o.get(k).and_then(J::as_str).ok_or(format!("rule missing path `{k}`"))?)
            .map_err(|e| format!("{k}: {e}"))
    };
    let sol = |k: &str| -> Result<SolValue, String> {
        json_to_sol(o.get(k).ok_or(format!("missing `{k}`"))?)
    };
    Ok(match op {
        "rename" => Rule::Rename { from: path("from")?, to: path("to")? },
        "drop" => Rule::Drop { path: path("path")? },
        "keep" => Rule::Keep {
            paths: o
                .get("paths")
                .and_then(J::as_array)
                .ok_or("keep.paths array")?
                .iter()
                .map(|p| Path::parse(p.as_str().ok_or("path string")?).map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?,
        },
        "default" => Rule::Default { path: path("path")?, v: sol("v")? },
        "cast" => Rule::Cast {
            path: path("path")?,
            to: o.get("to").and_then(J::as_str).ok_or("cast.to")?.to_string(),
            mode: o.get("mode").and_then(J::as_str).map(str::to_string),
        },
        "wrap" => Rule::Wrap { path: path("path")?, key: o.get("key").and_then(J::as_str).ok_or("wrap.key")?.into() },
        "unwrap" => Rule::Unwrap { path: path("path")? },
        "map_enum" => Rule::MapEnum {
            path: path("path")?,
            table: o
                .get("table")
                .and_then(J::as_object)
                .ok_or("map_enum.table")?
                .iter()
                .map(|(k, v)| Ok::<_, String>((k.clone(), json_to_sol(v)?)))
                .collect::<Result<_, _>>()?,
        },
        "path_copy" => Rule::PathCopy { from: path("from")?, to: path("to")? },
        "const_set" => Rule::ConstSet { path: path("path")?, v: sol("v")? },
        "trim" => Rule::Trim { path: path("path")? },
        other => return Err(format!("unknown rule op `{other}` — closed set (§14)")),
    })
}

fn json_to_sol(j: &J) -> Result<SolValue, String> {
    aelio_kernel::json_from(j).map_err(|e| e.to_string())
}

/// Apply rules sequentially to the input Sol (§14). Returns the rewritten Sol or the first
/// `Convert.RuleFail`. Input must be a map.
pub fn apply_rules(rules: &[Rule], input: &SolValue) -> Result<SolValue, RuleFail> {
    let mut work = input.clone();
    for (i, rule) in rules.iter().enumerate() {
        apply_one(rule, &mut work).map_err(|detail| RuleFail {
            rule_index: i,
            code: ReasonCode::ConvertRuleFail,
            detail,
        })?;
    }
    Ok(work)
}

fn apply_one(rule: &Rule, work: &mut SolValue) -> Result<(), String> {
    match rule {
        Rule::Rename { from, to } => {
            let v = get(work, from).cloned().ok_or("rename: `from` absent")?;
            remove(work, from);
            set(work, to, v)
        }
        Rule::Drop { path } => {
            remove(work, path); // total
            Ok(())
        }
        Rule::Keep { paths } => {
            // v0: keep operates over top-level keys — drop all not named.
            let keep_keys: Vec<String> = paths
                .iter()
                .filter_map(|p| match p.segments() {
                    [Segment::Key(k)] => Some(k.clone()),
                    _ => None,
                })
                .collect();
            if let SolValue::Map(m) = work {
                m.retain(|k, _| keep_keys.contains(k));
            }
            Ok(())
        }
        Rule::Default { path, v } => {
            if get(work, path).is_none() {
                set(work, path, v.clone())?;
            }
            Ok(())
        }
        Rule::ConstSet { path, v } => set(work, path, v.clone()),
        Rule::PathCopy { from, to } => {
            let v = get(work, from).cloned().ok_or("path_copy: `from` absent")?;
            set(work, to, v)
        }
        Rule::Trim { path } => {
            let v = get(work, path).ok_or("trim: path absent")?;
            match v {
                SolValue::Str(s) => {
                    let trimmed = s.trim_matches(|c: char| c.is_ascii_whitespace()).to_string();
                    set(work, path, SolValue::Str(trimmed))
                }
                _ => Err("trim requires a string (§14)".into()),
            }
        }
        Rule::Wrap { path, key } => {
            let v = get(work, path).cloned().ok_or("wrap: path absent")?;
            set(work, path, SolValue::map([(key.clone(), v)]))
        }
        Rule::Unwrap { path } => {
            let v = get(work, path).ok_or("unwrap: path absent")?;
            match v {
                SolValue::Map(m) if m.len() == 1 => {
                    let inner = m.values().next().unwrap().clone();
                    set(work, path, inner)
                }
                SolValue::Map(_) => Err("unwrap: not a single-key map (§14)".into()),
                _ => Err("unwrap of non-map (§14)".into()),
            }
        }
        Rule::MapEnum { path, table } => {
            let v = get(work, path).ok_or("map_enum: path absent")?;
            let key = aelio_sol::canonical_string(v);
            // Try direct string key first (common), else canonical form.
            let mapped = if let SolValue::Str(s) = v {
                table.get(s).or_else(|| table.get(&key))
            } else {
                table.get(&key)
            };
            match mapped {
                Some(mv) => set(work, path, mv.clone()),
                None => Err("map_enum: unmapped value — never invented (§14, A14.2)".into()),
            }
        }
        Rule::Cast { path, to, mode } => {
            let v = get(work, path).cloned().ok_or("cast: path absent")?;
            let out = cast(&v, to, mode.as_deref())?;
            set(work, path, out)
        }
    }
}

/// §5.2 cast matrix (representation only). FORBIDDEN casts error (would be plan-time rejected).
fn cast(v: &SolValue, to: &str, mode: Option<&str>) -> Result<SolValue, String> {
    use SolValue::*;
    match (v, to) {
        // identity
        (x, t) if x.type_tag().signature() == t => Ok(x.clone()),
        // TOTAL to str
        (Int(i), "str") => Ok(Str(i.to_string())),
        (Float(_), "str") => Ok(Str(aelio_sol::canonical_string(v))),
        (Bool(b), "str") => Ok(Str(if *b { "true".into() } else { "false".into() })),
        // CHECKED from str
        (Str(s), "int") => parse_strict_int(s).map(Int),
        (Str(s), "float") => parse_strict_float(s).and_then(|f| SolValue::float(f).map_err(|_| "non-finite".into())),
        (Str(s), "bool") => match s.as_str() {
            "true" => Ok(Bool(true)),
            "false" => Ok(Bool(false)),
            _ => Err("str→bool requires exactly \"true\"/\"false\" (§5.2)".into()),
        },
        // CHECKED int→float (exact only)
        (Int(i), "float") => {
            let f = *i as f64;
            if f as i64 == *i {
                SolValue::float(f).map_err(|_| "non-finite".into())
            } else {
                Err("int→float not exactly representable (§5.2 CHECKED)".into())
            }
        }
        // MODE-REQUIRED float→int
        (Float(f), "int") => {
            let m = mode.ok_or("float→int requires mode (§5.2)")?;
            let r = match m {
                "trunc" => f.trunc(),
                "floor" => f.floor(),
                "ceil" => f.ceil(),
                "round" => f.round(),
                other => return Err(format!("bad float→int mode `{other}`")),
            };
            if r >= i64::MIN as f64 && r <= i64::MAX as f64 {
                Ok(Int(r as i64))
            } else {
                Err("float→int out of i64 range (§5.2)".into())
            }
        }
        // FORBIDDEN
        _ => Err(format!("forbidden cast {}→{to} (§5.2) — use map_enum/default/wrap", v.type_tag().signature())),
    }
}

fn parse_strict_int(s: &str) -> Result<i64, String> {
    // Optional sign + digits; no whitespace/underscores (§5.2).
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return Err("empty str→int".into());
    }
    let (start, _neg) = match bytes[0] {
        b'-' | b'+' => (1, bytes[0] == b'-'),
        _ => (0, false),
    };
    if start == bytes.len() || !bytes[start..].iter().all(|b| b.is_ascii_digit()) {
        return Err("str→int: strict grammar (sign + digits) (§5.2)".into());
    }
    s.parse::<i64>().map_err(|_| "str→int overflow".into())
}

fn parse_strict_float(s: &str) -> Result<f64, String> {
    if s.chars().any(|c| c.is_whitespace() || c == '_') || s.eq_ignore_ascii_case("nan") || s.to_ascii_lowercase().contains("inf") {
        return Err("str→float: no whitespace/underscore/nan/inf (§5.2)".into());
    }
    s.parse::<f64>().map_err(|_| "str→float parse".into())
}

// ── path-addressed get/set/remove over a SolValue map ─────────────────────────────────────────
fn get<'a>(root: &'a SolValue, path: &Path) -> Option<&'a SolValue> {
    path.get(root)
}
fn set(root: &mut SolValue, path: &Path, value: SolValue) -> Result<(), String> {
    let mut bag = aelio_kernel::bag::Bag::from_value(std::mem::replace(root, SolValue::Null));
    let r = bag.set(path, value).map_err(str::to_string);
    *root = bag.value().clone();
    r
}
fn remove(root: &mut SolValue, path: &Path) {
    let segs = path.segments();
    if let [prefix @ .., Segment::Key(last)] = segs {
        let mut cur = root;
        for seg in prefix {
            cur = match (seg, cur) {
                (Segment::Key(k), SolValue::Map(m)) => match m.get_mut(k) {
                    Some(c) => c,
                    None => return,
                },
                (Segment::Index(i), SolValue::List(l)) => match l.get_mut(*i) {
                    Some(c) => c,
                    None => return,
                },
                _ => return,
            };
        }
        if let SolValue::Map(m) = cur {
            m.remove(last);
        }
    }
}
