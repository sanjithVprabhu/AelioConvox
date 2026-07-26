//! Compute catalog (§9) — closed, pure, **checked arithmetic** (overflow ⇒ Type, never wraps, §5).
//! Comparisons are type-strict: int vs float requires an explicit cast (§9). `pull`/`exists` are
//! evaluated in the Expr layer (they read the bag); this table is pure over already-resolved args.

use crate::error::ReasonCode;
use aelio_sol::SolValue;

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
