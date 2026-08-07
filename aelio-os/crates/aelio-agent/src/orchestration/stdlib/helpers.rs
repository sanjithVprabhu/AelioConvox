use crate::types::{AelioError, AelioResult, ReasonCode, Value};
use indexmap::IndexMap;

pub fn ok_map(field: &str, value: Value) -> AelioResult<Value> {
    let mut map = IndexMap::new();
    map.insert(field.into(), value);
    Ok(Value::Map(map))
}

pub fn require<'a>(args: &'a IndexMap<String, Value>, key: &str) -> AelioResult<&'a Value> {
    args.get(key)
        .ok_or_else(|| AelioError::new(ReasonCode::Missing, format!("missing `{key}`")))
}

pub fn require_str<'a>(args: &'a IndexMap<String, Value>, key: &str) -> AelioResult<&'a str> {
    require(args, key)?.as_str().ok_or_else(|| {
        AelioError::new(ReasonCode::TypeViolation, format!("`{key}` must be a string"))
    })
}

pub fn require_list(args: &IndexMap<String, Value>, key: &str) -> AelioResult<Vec<Value>> {
    match require(args, key)? {
        Value::List(items) => Ok(items.clone()),
        _ => Err(AelioError::new(
            ReasonCode::TypeViolation,
            format!("`{key}` must be a list"),
        )),
    }
}

pub fn require_bool(args: &IndexMap<String, Value>, key: &str) -> AelioResult<bool> {
    match require(args, key)? {
        Value::Bool(b) => Ok(*b),
        _ => Err(AelioError::new(
            ReasonCode::TypeViolation,
            format!("`{key}` must be a bool"),
        )),
    }
}

pub fn require_number(args: &IndexMap<String, Value>, key: &str) -> AelioResult<f64> {
    require(args, key)?
        .as_f64()
        .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, format!("`{key}` must be a number")))
}

pub fn string_list(args: &IndexMap<String, Value>, key: &str) -> AelioResult<Vec<String>> {
    require_list(args, key)?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| {
                    AelioError::new(ReasonCode::TypeViolation, format!("`{key}` items must be strings"))
                })
        })
        .collect()
}

pub fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
