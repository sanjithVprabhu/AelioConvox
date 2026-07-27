//! Compute catalog (§9) — closed, pure, **checked arithmetic** (overflow ⇒ Type, never wraps, §5).
//! Comparisons are type-strict: int vs float requires an explicit cast (§9). `pull`/`exists` are
//! evaluated in the Expr layer (they read the bag); this table is pure over already-resolved args.

use crate::error::ReasonCode;
use aelio_sol::SolValue;

#[derive(Debug)]
pub struct ComputeErr {
    pub code: ReasonCode,
    pub detail: String,
}

fn type_err(detail: impl Into<String>) -> ComputeErr {
    ComputeErr { code: ReasonCode::Type, detail: detail.into() }
}

type R = Result<SolValue, ComputeErr>;

/// Apply a pure Compute op to resolved args.
pub fn apply(op: &str, args: &[SolValue]) -> R {
    use SolValue::*;
    match op {
        // ── numeric (checked) ──
        "add" => num_bin(op, args, i64::checked_add, |a, b| a + b),
        "sub" => num_bin(op, args, i64::checked_sub, |a, b| a - b),
        "mul" => num_bin(op, args, i64::checked_mul, |a, b| a * b),
        "div" => match two(op, args)? {
            (Int(_), Int(0)) => Err(type_err("division by zero (§9)")),
            (Int(a), Int(b)) => Ok(Int(a.checked_div(*b).ok_or_else(|| type_err("overflow"))?)),
            (Float(_), Float(b)) if *b == 0.0 => Err(type_err("division by zero (§9)")),
            (Float(a), Float(b)) => SolValue::float(a / b).map_err(|_| type_err("non-finite result")),
            _ => Err(type_err("div requires two ints or two floats (§9)")),
        },
        "mod" => match two(op, args)? {
            (Int(_), Int(0)) => Err(type_err("mod by zero (§9)")),
            (Int(a), Int(b)) => Ok(Int(a.checked_rem(*b).ok_or_else(|| type_err("overflow"))?)),
            _ => Err(type_err("mod requires two ints (§9)")),
        },
        "abs" => match one(op, args)? {
            Int(a) => Ok(Int(a.checked_abs().ok_or_else(|| type_err("overflow"))?)),
            Float(a) => SolValue::float(a.abs()).map_err(|_| type_err("non-finite")),
            _ => Err(type_err("abs requires a number")),
        },
        "min" | "max" => {
            let (a, b) = two(op, args)?;
            let lt = num_lt(a, b)?;
            let pick_first = (op == "min") == lt;
            Ok(if pick_first { a.clone() } else { b.clone() })
        }

        // ── compare / logic (type-strict) ──
        "eq" => Ok(Bool(strict_eq(op, args)?)),
        "ne" => Ok(Bool(!strict_eq(op, args)?)),
        "lt" => Ok(Bool(num_lt(&args[0], &args[1])?)),
        "le" => Ok(Bool(!num_lt(&args[1], &args[0])?)),
        "gt" => Ok(Bool(num_lt(&args[1], &args[0])?)),
        "ge" => Ok(Bool(!num_lt(&args[0], &args[1])?)),
        "and" => Ok(Bool(bool_arg(op, args, 0)? && bool_arg(op, args, 1)?)),
        "or" => Ok(Bool(bool_arg(op, args, 0)? || bool_arg(op, args, 1)?)),
        "not" => Ok(Bool(!bool_arg(op, args, 0)?)),

        // ── string ──
        "concat" => {
            let mut s = String::new();
            for a in args {
                match a {
                    Str(x) => s.push_str(x),
                    _ => return Err(type_err("concat requires strings (§9)")),
                }
            }
            Ok(Str(s))
        }
        "length" => match one(op, args)? {
            Str(s) => Ok(Int(s.chars().count() as i64)),
            _ => Err(type_err("length requires a string")),
        },
        "contains" => str_pred(op, args, |h, n| h.contains(n)),
        "starts_with" => str_pred(op, args, |h, n| h.starts_with(n)),
        "ends_with" => str_pred(op, args, |h, n| h.ends_with(n)),

        // ── structure (§9, §4.2.2) ──
        "merge" => match (args.first(), args.get(1), args.get(2)) {
            (Some(Map(l)), Some(Map(r)), Some(Str(policy))) => merge_maps(l, r, policy),
            _ => Err(type_err("merge(left:map, right:map, on_conflict:str) (§4.2.2)")),
        },
        "drop" => {
            let (m, keys) = map_and_keys(op, args)?;
            let mut out = m.clone();
            for k in &keys {
                out.remove(k);
            }
            Ok(Map(out))
        }
        "keep" => {
            let (m, keys) = map_and_keys(op, args)?;
            let out = m.iter().filter(|(k, _)| keys.contains(*k)).map(|(k, v)| (k.clone(), v.clone())).collect();
            Ok(Map(out))
        }
        "path_copy" => match (args.first(), args.get(1), args.get(2)) {
            // Pure form (§9/F3 refined, F-008): copy the value at `from` to `to` within a container.
            (Some(container), Some(Str(from)), Some(Str(to))) => path_copy(container, from, to),
            _ => Err(type_err("path_copy(container:map, from:str, to:str)")),
        },

        // ── list (§9 — producers carry max_items) ──
        "count" => Ok(Int(list_arg(op, args, 0)?.len() as i64)),
        "list_contains" => {
            let l = list_arg(op, args, 0)?;
            let item = args.get(1).ok_or_else(|| type_err("list_contains(list, item)"))?;
            Ok(Bool(l.iter().any(|x| x == item)))
        }
        "first" => list_arg(op, args, 0)?.first().cloned().ok_or_else(|| ComputeErr { code: ReasonCode::Missing, detail: "first: empty list (§9)".into() }),
        "last" => list_arg(op, args, 0)?.last().cloned().ok_or_else(|| ComputeErr { code: ReasonCode::Missing, detail: "last: empty list (§9)".into() }),
        "append" => {
            let l = list_arg(op, args, 0)?;
            let item = args.get(1).ok_or_else(|| type_err("append(list, item, max_items)"))?;
            let max = int_arg(op, args, 2)?;
            if (l.len() as i64) + 1 > max {
                return Err(ComputeErr { code: ReasonCode::BudgetSize, detail: format!("append exceeds max_items={max} (§9)") });
            }
            let mut out = l.to_vec();
            out.push(item.clone());
            Ok(List(out))
        }
        "slice" => {
            let l = list_arg(op, args, 0)?;
            let start = int_arg(op, args, 1)?;
            let end = int_arg(op, args, 2)?;
            let max = int_arg(op, args, 3)?;
            if start < 0 || end < start || end > l.len() as i64 {
                return Err(type_err(format!("slice bounds out of range: start={start} end={end} len={} (§9)", l.len())));
            }
            if end - start > max {
                return Err(ComputeErr { code: ReasonCode::BudgetSize, detail: format!("slice exceeds max_items={max} (§9)") });
            }
            Ok(List(l[start as usize..end as usize].to_vec()))
        }

        // ── validate ──
        "is_type" => match (args.first(), args.get(1)) {
            (Some(v), Some(Str(t))) => Ok(Bool(v.type_tag().signature() == t)),
            _ => Err(type_err("is_type(value, type_name)")),
        },

        // ── hash ──
        "blake3" => Ok(Str(aelio_sol::value_hash(one(op, args)?))),

        other => Err(ComputeErr { code: ReasonCode::Shape, detail: format!("unknown compute op `{other}`") }),
    }
}

fn one<'a>(op: &str, args: &'a [SolValue]) -> Result<&'a SolValue, ComputeErr> {
    args.first().ok_or_else(|| type_err(format!("{op} needs 1 arg")))
}
fn two<'a>(op: &str, args: &'a [SolValue]) -> Result<(&'a SolValue, &'a SolValue), ComputeErr> {
    match (args.first(), args.get(1)) {
        (Some(a), Some(b)) => Ok((a, b)),
        _ => Err(type_err(format!("{op} needs 2 args"))),
    }
}
fn bool_arg(op: &str, args: &[SolValue], i: usize) -> Result<bool, ComputeErr> {
    match args.get(i) {
        Some(SolValue::Bool(b)) => Ok(*b),
        _ => Err(type_err(format!("{op} requires bool args"))),
    }
}

fn list_arg<'a>(op: &str, args: &'a [SolValue], i: usize) -> Result<&'a [SolValue], ComputeErr> {
    match args.get(i) {
        Some(SolValue::List(l)) => Ok(l),
        _ => Err(type_err(format!("{op} requires a list at arg {i} (§9)"))),
    }
}

fn int_arg(op: &str, args: &[SolValue], i: usize) -> Result<i64, ComputeErr> {
    match args.get(i) {
        Some(SolValue::Int(n)) => Ok(*n),
        _ => Err(type_err(format!("{op} requires an int at arg {i} (§9)"))),
    }
}

/// (map, keys) where `keys` is a single Str or a List of Str — for `drop`/`keep`.
fn map_and_keys(op: &str, args: &[SolValue]) -> Result<(std::collections::BTreeMap<String, SolValue>, Vec<String>), ComputeErr> {
    let m = match args.first() {
        Some(SolValue::Map(m)) => m.clone(),
        _ => return Err(type_err(format!("{op}(map, keys) — first arg must be a map (§9)"))),
    };
    let keys = match args.get(1) {
        Some(SolValue::Str(k)) => vec![k.clone()],
        Some(SolValue::List(l)) => l
            .iter()
            .map(|v| match v {
                SolValue::Str(s) => Ok(s.clone()),
                _ => Err(type_err(format!("{op} keys must be strings (§9)"))),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(type_err(format!("{op}(map, keys) — keys must be a string or list of strings (§9)"))),
    };
    Ok((m, keys))
}

/// §4.2.2 merge with mandatory `on_conflict`: `error` (default) | `left` | `right`.
fn merge_maps(
    l: &std::collections::BTreeMap<String, SolValue>,
    r: &std::collections::BTreeMap<String, SolValue>,
    policy: &str,
) -> R {
    let mut out = l.clone();
    for (k, v) in r {
        if out.contains_key(k) {
            match policy {
                "error" => return Err(ComputeErr { code: ReasonCode::Shape, detail: format!("merge key conflict `{k}` (on_conflict=error, §4.2.2)") }),
                "left" => {} // keep left
                "right" => {
                    out.insert(k.clone(), v.clone());
                }
                other => return Err(type_err(format!("merge on_conflict must be error|left|right, got `{other}` (§4.2.2)"))),
            }
        } else {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(SolValue::Map(out))
}

/// Pure `path_copy`: copy the value at dotted `from` to dotted `to` inside a map container (F-008).
fn path_copy(container: &SolValue, from: &str, to: &str) -> R {
    let root = match container {
        SolValue::Map(m) => m.clone(),
        _ => return Err(type_err("path_copy container must be a map (§9)")),
    };
    let val = get_path(&SolValue::Map(root.clone()), from)
        .ok_or_else(|| ComputeErr { code: ReasonCode::Missing, detail: format!("path_copy: `{from}` not present (§9)") })?
        .clone();
    let mut out = SolValue::Map(root);
    set_path(&mut out, to, val)?;
    Ok(out)
}

fn get_path<'a>(v: &'a SolValue, path: &str) -> Option<&'a SolValue> {
    let mut cur = v;
    for seg in path.split('.') {
        cur = cur.as_map()?.get(seg)?;
    }
    Some(cur)
}

fn set_path(v: &mut SolValue, path: &str, val: SolValue) -> Result<(), ComputeErr> {
    let segs: Vec<&str> = path.split('.').collect();
    let mut cur = v;
    for seg in &segs[..segs.len() - 1] {
        let m = match cur {
            SolValue::Map(m) => m,
            _ => return Err(type_err("path_copy: cannot descend into non-map (§9)")),
        };
        cur = m.entry((*seg).to_string()).or_insert_with(|| SolValue::Map(Default::default()));
    }
    match cur {
        SolValue::Map(m) => {
            m.insert(segs[segs.len() - 1].to_string(), val);
            Ok(())
        }
        _ => Err(type_err("path_copy: target parent is not a map (§9)")),
    }
}

fn num_bin(
    op: &str,
    args: &[SolValue],
    ints: fn(i64, i64) -> Option<i64>,
    floats: fn(f64, f64) -> f64,
) -> R {
    match two(op, args)? {
        (SolValue::Int(a), SolValue::Int(b)) => {
            Ok(SolValue::Int(ints(*a, *b).ok_or_else(|| type_err(format!("{op} overflow (§5)")))?))
        }
        (SolValue::Float(a), SolValue::Float(b)) => {
            SolValue::float(floats(*a, *b)).map_err(|_| type_err("non-finite result (§4.3)"))
        }
        _ => Err(type_err(format!("{op} requires two ints or two floats; cast to mix (§9)"))),
    }
}

fn num_lt(a: &SolValue, b: &SolValue) -> Result<bool, ComputeErr> {
    match (a, b) {
        (SolValue::Int(x), SolValue::Int(y)) => Ok(x < y),
        (SolValue::Float(x), SolValue::Float(y)) => Ok(x < y),
        _ => Err(type_err("ordered comparison requires two ints or two floats (§9)")),
    }
}

/// Type-strict equality: same fundamental type required (int≢float). Structural within a type.
fn strict_eq(op: &str, args: &[SolValue]) -> Result<bool, ComputeErr> {
    let (a, b) = two(op, args)?;
    if a.type_tag() != b.type_tag() {
        return Err(type_err("eq/ne is type-strict — cast to compare across types (§9)"));
    }
    Ok(a == b)
}

fn str_pred(op: &str, args: &[SolValue], f: fn(&str, &str) -> bool) -> R {
    match two(op, args)? {
        (SolValue::Str(h), SolValue::Str(n)) => Ok(SolValue::Bool(f(h, n))),
        _ => Err(type_err(format!("{op} requires two strings"))),
    }
}
