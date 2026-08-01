//! S0-B — Pure operations. Total, side-effect-free, deterministic, cost class Free.

use crate::path::{get_path, get_path_all, has_path, set_path};
use crate::types::{AelioError, AelioResult, ReasonCode, TypeTag, Value};
use indexmap::IndexMap;
use regex::Regex;
use sha2::{Digest, Sha256};

// ── Numeric ──────────────────────────────────────────────────────────────────

pub fn add(nums: &[Value]) -> AelioResult<Value> {
    if nums.is_empty() {
        return Ok(Value::Int(0));
    }
    let mut acc = 0.0f64;
    let mut all_int = true;
    for n in nums {
        match n {
            Value::Int(i) => acc += *i as f64,
            Value::Float(f) => {
                all_int = false;
                acc += *f;
            }
            _ => {
                return Err(AelioError::new(
                    ReasonCode::TypeViolation,
                    "Add expects numbers",
                ))
            }
        }
    }
    if all_int && acc.fract() == 0.0 && acc >= i64::MIN as f64 && acc <= i64::MAX as f64 {
        Ok(Value::Int(acc as i64))
    } else {
        Ok(Value::Float(acc))
    }
}

pub fn subtract(from: &Value, by: &Value) -> AelioResult<Value> {
    let a = from
        .as_f64()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Subtract: from not num"))?;
    let b = by
        .as_f64()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Subtract: by not num"))?;
    match (from, by) {
        (Value::Int(_), Value::Int(_)) => Ok(Value::Int((a - b) as i64)),
        _ => Ok(Value::Float(a - b)),
    }
}

pub fn multiply(nums: &[Value]) -> AelioResult<Value> {
    if nums.is_empty() {
        return Ok(Value::Int(1));
    }
    let mut acc = 1.0f64;
    let mut all_int = true;
    for n in nums {
        match n {
            Value::Int(i) => acc *= *i as f64,
            Value::Float(f) => {
                all_int = false;
                acc *= *f;
            }
            _ => {
                return Err(AelioError::new(
                    ReasonCode::TypeViolation,
                    "Multiply expects numbers",
                ))
            }
        }
    }
    if all_int && acc.fract() == 0.0 && acc >= i64::MIN as f64 && acc <= i64::MAX as f64 {
        Ok(Value::Int(acc as i64))
    } else {
        Ok(Value::Float(acc))
    }
}

pub fn divide(from: &Value, by: &Value) -> AelioResult<Value> {
    let a = from
        .as_f64()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Divide: from not num"))?;
    let b = by
        .as_f64()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Divide: by not num"))?;
    if b == 0.0 {
        return Err(AelioError::new(ReasonCode::DivByZero, "division by zero"));
    }
    Ok(Value::Float(a / b))
}

pub fn min_nums(nums: &[Value]) -> AelioResult<Value> {
    if nums.is_empty() {
        return Err(AelioError::new(ReasonCode::EmptyInput, "Min on empty"));
    }
    let mut best = nums[0]
        .as_f64()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Min expects numbers"))?;
    let mut best_v = nums[0].clone();
    for n in &nums[1..] {
        let f = n
            .as_f64()
            .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Min expects numbers"))?;
        if f < best {
            best = f;
            best_v = n.clone();
        }
    }
    Ok(best_v)
}

pub fn max_nums(nums: &[Value]) -> AelioResult<Value> {
    if nums.is_empty() {
        return Err(AelioError::new(ReasonCode::EmptyInput, "Max on empty"));
    }
    let mut best = nums[0]
        .as_f64()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Max expects numbers"))?;
    let mut best_v = nums[0].clone();
    for n in &nums[1..] {
        let f = n
            .as_f64()
            .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Max expects numbers"))?;
        if f > best {
            best = f;
            best_v = n.clone();
        }
    }
    Ok(best_v)
}

pub fn sum(nums: &[Value]) -> AelioResult<Value> {
    add(nums)
}

pub fn clamp(value: &Value, lo: &Value, hi: &Value) -> AelioResult<Value> {
    let v = value
        .as_f64()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Clamp: value"))?;
    let l = lo
        .as_f64()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Clamp: lo"))?;
    let h = hi
        .as_f64()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Clamp: hi"))?;
    Ok(Value::Float(v.clamp(l, h)))
}

pub fn compare(a: &Value, b: &Value) -> AelioResult<&'static str> {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => Ok(if x < y {
            "lt"
        } else if x > y {
            "gt"
        } else {
            "eq"
        }),
        _ => match (a.as_str(), b.as_str()) {
            (Some(x), Some(y)) => Ok(if x < y {
                "lt"
            } else if x > y {
                "gt"
            } else {
                "eq"
            }),
            _ => Err(AelioError::new(
                ReasonCode::TypeViolation,
                "Compare: incompatible types",
            )),
        },
    }
}

// ── String ───────────────────────────────────────────────────────────────────

pub fn length(value: &Value) -> AelioResult<Value> {
    match value {
        Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
        Value::List(l) => Ok(Value::Int(l.len() as i64)),
        Value::Map(m) => Ok(Value::Int(m.len() as i64)),
        _ => Err(AelioError::new(
            ReasonCode::TypeViolation,
            "Length expects str/list/map",
        )),
    }
}

pub fn word_count(value: &Value) -> AelioResult<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "WordCount expects str"))?;
    let n = s.split_whitespace().filter(|w| !w.is_empty()).count();
    Ok(Value::Int(n as i64))
}

pub fn strip(value: &Value) -> AelioResult<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Strip expects str"))?;
    Ok(Value::str(s.trim()))
}

pub fn lower(value: &Value) -> AelioResult<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Lower expects str"))?;
    Ok(Value::str(s.to_lowercase()))
}

pub fn upper(value: &Value) -> AelioResult<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Upper expects str"))?;
    Ok(Value::str(s.to_uppercase()))
}

pub fn contains(value: &Value, needle: &str) -> AelioResult<bool> {
    let s = value
        .as_str()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Contains expects str"))?;
    Ok(s.contains(needle))
}

pub fn starts_with(value: &Value, needle: &str) -> AelioResult<bool> {
    let s = value
        .as_str()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "StartsWith expects str"))?;
    Ok(s.starts_with(needle))
}

pub fn ends_with(value: &Value, needle: &str) -> AelioResult<bool> {
    let s = value
        .as_str()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "EndsWith expects str"))?;
    Ok(s.ends_with(needle))
}

pub fn concat(parts: &[Value]) -> AelioResult<Value> {
    let mut out = String::new();
    for p in parts {
        match p {
            Value::Str(s) => out.push_str(s),
            Value::Int(i) => out.push_str(&i.to_string()),
            Value::Float(f) => out.push_str(&f.to_string()),
            Value::Bool(b) => out.push_str(&b.to_string()),
            Value::Null => {}
            _ => {
                return Err(AelioError::new(
                    ReasonCode::TypeViolation,
                    "Concat expects scalar parts",
                ))
            }
        }
    }
    Ok(Value::str(out))
}

pub fn join(parts: &[Value], sep: &str) -> AelioResult<Value> {
    let mut strs = Vec::new();
    for p in parts {
        let s = p
            .as_str()
            .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Join expects strings"))?;
        strs.push(s);
    }
    Ok(Value::str(strs.join(sep)))
}

pub fn split_by_delimiter(value: &Value, sep: &str, limit: Option<usize>) -> AelioResult<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Split expects str"))?;
    let parts: Vec<Value> = if let Some(n) = limit {
        s.splitn(n, sep).map(Value::str).collect()
    } else {
        s.split(sep).map(Value::str).collect()
    };
    Ok(Value::List(parts))
}

pub fn split_by_whitespace(value: &Value) -> AelioResult<Value> {
    let s = value.as_str().ok_or_else(|| {
        AelioError::new(ReasonCode::TypeViolation, "SplitByWhitespace expects str")
    })?;
    Ok(Value::List(s.split_whitespace().map(Value::str).collect()))
}

pub fn is_blank(value: &Value) -> AelioResult<bool> {
    match value {
        Value::Str(s) => Ok(s.trim().is_empty()),
        Value::Null => Ok(true),
        _ => Ok(false),
    }
}

pub fn equals_ignore_case(a: &Value, b: &Value) -> AelioResult<bool> {
    match (a.as_str(), b.as_str()) {
        (Some(x), Some(y)) => Ok(x.eq_ignore_ascii_case(y)),
        _ => Ok(a == b),
    }
}

// ── Regex (linear-time patterns only; caller validates at registration) ──────

pub fn regex_match(value: &Value, pattern: &str) -> AelioResult<bool> {
    let s = value
        .as_str()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "RegexMatch expects str"))?;
    let re = Regex::new(pattern)
        .map_err(|e| AelioError::new(ReasonCode::ParseError, format!("bad regex: {e}")))?;
    Ok(re.is_match(s))
}

pub fn regex_capture(value: &Value, pattern: &str) -> AelioResult<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "RegexCapture expects str"))?;
    let re = Regex::new(pattern)
        .map_err(|e| AelioError::new(ReasonCode::ParseError, format!("bad regex: {e}")))?;
    let caps = re
        .captures(s)
        .ok_or_else(|| AelioError::new(ReasonCode::NoMatch, "no match"))?;
    let mut m = IndexMap::new();
    for name in re.capture_names().flatten() {
        if let Some(c) = caps.name(name) {
            m.insert(name.to_string(), Value::str(c.as_str()));
        }
    }
    // also numbered groups as "1", "2", ...
    for (i, c) in caps.iter().enumerate().skip(1) {
        if let Some(c) = c {
            m.entry(i.to_string())
                .or_insert_with(|| Value::str(c.as_str()));
        }
    }
    Ok(Value::Map(m))
}

// ── Structure ────────────────────────────────────────────────────────────────

pub fn get_path_op(doc: &Value, path: &str) -> AelioResult<Value> {
    match get_path(doc, path)? {
        Some(v) => Ok(v),
        None => Ok(Value::Null),
    }
}

pub fn get_path_all_op(doc: &Value, path: &str) -> AelioResult<Value> {
    Ok(Value::List(get_path_all(doc, path)?))
}

pub fn set_path_op(doc: &Value, path: &str, value: Value) -> AelioResult<Value> {
    set_path(doc, path, value)
}

pub fn has_path_op(doc: &Value, path: &str) -> AelioResult<bool> {
    has_path(doc, path)
}

pub fn keys(obj: &Value) -> AelioResult<Value> {
    match obj {
        Value::Map(m) => Ok(Value::List(m.keys().map(Value::str).collect())),
        _ => Err(AelioError::new(
            ReasonCode::TypeViolation,
            "Keys expects map",
        )),
    }
}

pub fn values_of(obj: &Value) -> AelioResult<Value> {
    match obj {
        Value::Map(m) => Ok(Value::List(m.values().cloned().collect())),
        _ => Err(AelioError::new(
            ReasonCode::TypeViolation,
            "Values expects map",
        )),
    }
}

pub fn pick(obj: &Value, key_list: &[&str]) -> AelioResult<Value> {
    let m = obj
        .as_map()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Pick expects map"))?;
    let mut out = IndexMap::new();
    for k in key_list {
        if let Some(v) = m.get(*k) {
            out.insert((*k).to_string(), v.clone());
        }
    }
    Ok(Value::Map(out))
}

pub fn omit(obj: &Value, key_list: &[&str]) -> AelioResult<Value> {
    let m = obj
        .as_map()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "Omit expects map"))?;
    let mut out = IndexMap::new();
    for (k, v) in m {
        if !key_list.contains(&k.as_str()) {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(Value::Map(out))
}

pub fn merge_shallow(a: &Value, b: &Value) -> AelioResult<Value> {
    let ma = a
        .as_map()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "MergeShallow: a"))?;
    let mb = b
        .as_map()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "MergeShallow: b"))?;
    let mut out = ma.clone();
    for (k, v) in mb {
        out.insert(k.clone(), v.clone());
    }
    Ok(Value::Map(out))
}

pub fn parse_json(s: &str) -> AelioResult<Value> {
    let v: serde_json::Value = serde_json::from_str(s)
        .map_err(|e| AelioError::new(ReasonCode::ParseError, format!("json: {e}")))?;
    Ok(json_to_value(&v))
}

pub fn to_json(value: &Value, pretty: bool) -> AelioResult<Value> {
    let j = value_to_json(value);
    let s = if pretty {
        serde_json::to_string_pretty(&j)
    } else {
        serde_json::to_string(&j)
    }
    .map_err(|e| AelioError::new(ReasonCode::Internal, format!("to_json: {e}")))?;
    Ok(Value::str(s))
}

pub fn json_to_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                Value::Float(0.0)
            }
        }
        serde_json::Value::String(s) => Value::str(s),
        serde_json::Value::Array(a) => Value::List(a.iter().map(json_to_value).collect()),
        serde_json::Value::Object(o) => {
            let mut m = IndexMap::new();
            for (k, v) in o {
                m.insert(k.clone(), json_to_value(v));
            }
            Value::Map(m)
        }
    }
}

pub fn value_to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(i) => serde_json::json!(*i),
        Value::Float(f) => serde_json::json!(*f),
        Value::Str(s) => serde_json::Value::String(s.clone()),
        Value::List(l) => serde_json::Value::Array(l.iter().map(value_to_json).collect()),
        Value::Map(m) => {
            let mut o = serde_json::Map::new();
            for (k, v) in m {
                o.insert(k.clone(), value_to_json(v));
            }
            serde_json::Value::Object(o)
        }
    }
}

// ── Collections ──────────────────────────────────────────────────────────────

pub fn first(list: &Value) -> AelioResult<Value> {
    match list {
        Value::List(l) => Ok(l.first().cloned().unwrap_or(Value::Null)),
        _ => Err(AelioError::new(
            ReasonCode::TypeViolation,
            "First expects list",
        )),
    }
}

pub fn last(list: &Value) -> AelioResult<Value> {
    match list {
        Value::List(l) => Ok(l.last().cloned().unwrap_or(Value::Null)),
        _ => Err(AelioError::new(
            ReasonCode::TypeViolation,
            "Last expects list",
        )),
    }
}

pub fn take(list: &Value, n: usize) -> AelioResult<Value> {
    match list {
        Value::List(l) => Ok(Value::List(l.iter().take(n).cloned().collect())),
        _ => Err(AelioError::new(
            ReasonCode::TypeViolation,
            "Take expects list",
        )),
    }
}

pub fn count(list: &Value) -> AelioResult<Value> {
    match list {
        Value::List(l) => Ok(Value::Int(l.len() as i64)),
        Value::Map(m) => Ok(Value::Int(m.len() as i64)),
        Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
        _ => Err(AelioError::new(
            ReasonCode::TypeViolation,
            "Count expects list/map/str",
        )),
    }
}

pub fn append(list: &Value, item: Value) -> AelioResult<Value> {
    match list {
        Value::List(l) => {
            let mut out = l.clone();
            out.push(item);
            Ok(Value::List(out))
        }
        _ => Err(AelioError::new(
            ReasonCode::TypeViolation,
            "Append expects list",
        )),
    }
}

// ── Logic ────────────────────────────────────────────────────────────────────

pub fn and_bools(vals: &[bool]) -> bool {
    vals.iter().all(|b| *b)
}

pub fn or_bools(vals: &[bool]) -> bool {
    vals.iter().any(|b| *b)
}

pub fn not_bool(v: bool) -> bool {
    !v
}

pub fn deep_equals(a: &Value, b: &Value) -> bool {
    a == b
}

pub fn coalesce(vals: &[Value]) -> Value {
    for v in vals {
        if !v.is_null() {
            return v.clone();
        }
    }
    Value::Null
}

// ── Validation ───────────────────────────────────────────────────────────────

pub fn validate_type(value: &Value, tag: TypeTag) -> AelioResult<Value> {
    if value.type_tag() == tag || (matches!(tag, TypeTag::Float) && matches!(value, Value::Int(_)))
    {
        Ok(value.clone())
    } else {
        Err(AelioError::new(
            ReasonCode::TypeViolation,
            format!("expected {tag}, got {}", value.type_tag()),
        ))
    }
}

pub fn validate_range(value: &Value, lo: f64, hi: f64) -> AelioResult<Value> {
    let v = value
        .as_f64()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "ValidateRange expects num"))?;
    if v < lo || v > hi {
        return Err(AelioError::new(
            ReasonCode::RangeViolation,
            format!("{v} not in [{lo},{hi}]"),
        ));
    }
    Ok(value.clone())
}

pub fn validate_length(value: &Value, min: usize, max: usize) -> AelioResult<Value> {
    let n = match value {
        Value::Str(s) => s.chars().count(),
        Value::List(l) => l.len(),
        _ => {
            return Err(AelioError::new(
                ReasonCode::TypeViolation,
                "ValidateLength expects str/list",
            ))
        }
    };
    if n < min || n > max {
        return Err(AelioError::new(
            ReasonCode::LengthViolation,
            format!("length {n} not in [{min},{max}]"),
        ));
    }
    Ok(value.clone())
}

pub fn validate_enum(value: &Value, allowed: &[&str]) -> AelioResult<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "ValidateEnum expects str"))?;
    if !allowed.contains(&s) {
        return Err(AelioError::new(
            ReasonCode::EnumViolation,
            format!("{s} not in allowed set"),
        ));
    }
    Ok(value.clone())
}

pub fn validate_pattern(value: &Value, pattern: &str) -> AelioResult<Value> {
    if regex_match(value, pattern)? {
        Ok(value.clone())
    } else {
        Err(AelioError::new(
            ReasonCode::PatternViolation,
            "pattern mismatch",
        ))
    }
}

#[derive(Debug, Clone, Copy)]
pub enum FormatKind {
    Email,
    E164,
    Url,
    Uuid,
    Iso8601,
    Ipv4,
}

pub fn validate_format(value: &Value, format: FormatKind) -> AelioResult<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "ValidateFormat expects str"))?;
    let ok = match format {
        FormatKind::Email => s.contains('@') && s.contains('.') && !s.starts_with('@'),
        FormatKind::E164 => {
            let re = Regex::new(r"^\+[1-9]\d{6,14}$").unwrap();
            re.is_match(s)
        }
        FormatKind::Url => s.starts_with("http://") || s.starts_with("https://"),
        FormatKind::Uuid => {
            let re = Regex::new(
                r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$",
            )
            .unwrap();
            re.is_match(s)
        }
        FormatKind::Iso8601 => s.contains('T') || s.len() >= 10,
        FormatKind::Ipv4 => {
            let parts: Vec<_> = s.split('.').collect();
            parts.len() == 4
                && parts
                    .iter()
                    .all(|p| p.parse::<u8>().is_ok() && !(p.len() > 1 && p.starts_with('0')))
        }
    };
    if ok {
        Ok(value.clone())
    } else {
        Err(AelioError::new(
            ReasonCode::FormatViolation,
            format!("invalid {:?}", format),
        ))
    }
}

// ── Normalization / repair ───────────────────────────────────────────────────

pub fn normalize_phone(value: &Value, default_region: &str) -> AelioResult<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "NormalizePhone expects str"))?;
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    let e164 = if s.trim().starts_with('+') {
        format!("+{digits}")
    } else if default_region.eq_ignore_ascii_case("IN") && digits.len() == 10 {
        format!("+91{digits}")
    } else if default_region.eq_ignore_ascii_case("US") && digits.len() == 10 {
        format!("+1{digits}")
    } else {
        format!("+{digits}")
    };
    validate_format(&Value::str(&e164), FormatKind::E164)?;
    Ok(Value::str(e164))
}

pub fn normalize_email(value: &Value) -> AelioResult<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "NormalizeEmail expects str"))?;
    let s = s.trim().to_lowercase();
    validate_format(&Value::str(&s), FormatKind::Email)?;
    Ok(Value::str(s))
}

pub fn normalize_whitespace(value: &Value) -> AelioResult<Value> {
    let s = value.as_str().ok_or_else(|| {
        AelioError::new(ReasonCode::TypeViolation, "NormalizeWhitespace expects str")
    })?;
    let collapsed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    Ok(Value::str(collapsed))
}

pub fn coerce_number_from_text(value: &Value) -> AelioResult<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "CoerceNumber expects str"))?;
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    if cleaned.contains('.') {
        cleaned
            .parse::<f64>()
            .map(Value::Float)
            .map_err(|_| AelioError::new(ReasonCode::ParseError, "not a number"))
    } else {
        cleaned
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| AelioError::new(ReasonCode::ParseError, "not a number"))
    }
}

// ── Encoding & hashing ───────────────────────────────────────────────────────

pub fn hash_sha256(value: &Value) -> AelioResult<Value> {
    let bytes = match value {
        Value::Str(s) => s.as_bytes().to_vec(),
        _ => serde_json::to_vec(&value_to_json(value))
            .map_err(|e| AelioError::new(ReasonCode::Internal, e.to_string()))?,
    };
    let digest = Sha256::digest(&bytes);
    Ok(Value::str(hex::encode(digest)))
}

pub fn idem_key(parts: &[&str]) -> String {
    // Canonical ordering: join parts with unit separator then hash.
    let joined = parts.join("\u{1f}");
    let digest = Sha256::digest(joined.as_bytes());
    hex::encode(&digest[..16])
}

pub fn redact(value: &Value, paths: &[&str]) -> AelioResult<Value> {
    let mut out = value.clone();
    for p in paths {
        if has_path(&out, p)? {
            out = set_path(&out, p, Value::str("***"))?;
        }
    }
    Ok(out)
}

// ── Clause-split pre-gate (pure) ─────────────────────────────────────────────

const CLAUSE_MARKERS: &[&str] = &[" and ", " but ", " then ", " also ", ";", " — ", " -- "];

/// Pure pre-gate: skip LLM SplitClauses when the utterance is trivially single-clause.
pub fn skip_split_clauses(utterance: &str, max_words: usize, max_chars: usize) -> bool {
    let words = utterance.split_whitespace().count();
    if words <= max_words {
        return true;
    }
    if utterance.chars().count() < max_chars {
        return true;
    }
    let lower = utterance.to_lowercase();
    !CLAUSE_MARKERS.iter().any(|m| lower.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_phone() {
        assert_eq!(add(&[Value::Int(1), Value::Int(2)]).unwrap(), Value::Int(3));
        let phone = normalize_phone(&Value::str("98765 43210"), "IN").unwrap();
        assert_eq!(phone.as_str().unwrap(), "+919876543210");
    }

    #[test]
    fn skip_split_on_hi() {
        assert!(skip_split_clauses("Hi", 3, 20));
        assert!(!skip_split_clauses(
            "Hi, I need to cancel my order and also refund",
            3,
            20
        ));
    }

    #[test]
    fn e164_and_otp_format() {
        assert!(validate_format(&Value::str("+919876543210"), FormatKind::E164).is_ok());
        assert!(validate_pattern(&Value::str("434543"), r"^\d{6}$").is_ok());
    }
}
