//! Dispatch for `std.*` standard tools.

use super::helpers::{
    ok_map, require, require_bool, require_list, require_number, require_str, string_list,
    truncate_chars,
};
use crate::orchestration::ephemeral::{adapt_with_scope, EphemeralScope};
use crate::ops::pure::{self, FormatKind};
use crate::types::{AelioError, AelioResult, ReasonCode, Value};
use indexmap::IndexMap;

pub fn invoke_standard_tool_inner(
    tool_id: &str,
    args: &IndexMap<String, Value>,
) -> AelioResult<Value> {
    match tool_id {
        // ── math ───────────────────────────────────────────────────────────
        "std.math.add" => {
            let a = require(args, "a")?;
            let b = require(args, "b")?;
            ok_map("result", pure::add(&[a.clone(), b.clone()])?)
        }
        "std.math.subtract" => {
            ok_map(
                "result",
                pure::subtract(require(args, "a")?, require(args, "b")?)?,
            )
        }
        "std.math.multiply" => {
            ok_map("result", pure::multiply(&require_list(args, "values")?)?)
        }
        "std.math.divide" => {
            ok_map(
                "result",
                pure::divide(require(args, "a")?, require(args, "b")?)?,
            )
        }
        "std.math.sum" => ok_map("total", pure::sum(&require_list(args, "values")?)?),
        "std.math.average" => {
            let values = require_list(args, "values")?;
            if values.is_empty() {
                return Err(AelioError::new(
                    ReasonCode::TypeViolation,
                    "average requires non-empty list",
                ));
            }
            let total = pure::sum(&values)?;
            let count = Value::Int(values.len() as i64);
            ok_map("average", pure::divide(&total, &count)?)
        }
        "std.math.min" => ok_map("result", pure::min_nums(&require_list(args, "values")?)?),
        "std.math.max" => ok_map("result", pure::max_nums(&require_list(args, "values")?)?),
        "std.math.clamp" => ok_map(
            "result",
            pure::clamp(
                require(args, "value")?,
                require(args, "lo")?,
                require(args, "hi")?,
            )?,
        ),
        "std.math.percent_of" => {
            let part = require(args, "part")?;
            let whole = require(args, "whole")?;
            let ratio = pure::divide(part, whole)?;
            ok_map(
                "result",
                pure::multiply(&[ratio, Value::Int(100)])?,
            )
        }
        "std.math.modulo" => {
            let a = require_number(args, "a")?;
            let b = require_number(args, "b")?;
            if b == 0.0 {
                return Err(AelioError::new(ReasonCode::TypeViolation, "modulo by zero"));
            }
            ok_map("result", Value::Float(a % b))
        }
        "std.math.abs" => {
            let n = require_number(args, "value")?;
            ok_map("result", Value::Float(n.abs()))
        }
        "std.math.compare" => {
            let relation = pure::compare(require(args, "a")?, require(args, "b")?)?;
            ok_map("relation", Value::Str(relation.into()))
        }

        // ── text ───────────────────────────────────────────────────────────
        "std.text.length" => ok_map("length", pure::length(require(args, "text")?)?),
        "std.text.concat" => ok_map("text", pure::concat(&require_list(args, "parts")?)?),
        "std.text.join" => ok_map(
            "text",
            pure::join(
                &require_list(args, "parts")?,
                require_str(args, "separator")?,
            )?,
        ),
        "std.text.split" => ok_map(
            "parts",
            pure::split_by_delimiter(
                require(args, "text")?,
                require_str(args, "delimiter")?,
                None,
            )?,
        ),
        "std.text.split_words" => {
            ok_map("parts", pure::split_by_whitespace(require(args, "text")?)?)
        }
        "std.text.strip" => ok_map("text", pure::strip(require(args, "text")?)?),
        "std.text.lower" => ok_map("text", pure::lower(require(args, "text")?)?),
        "std.text.upper" => ok_map("text", pure::upper(require(args, "text")?)?),
        "std.text.contains" => ok_map(
            "match",
            Value::Bool(pure::contains(
                require(args, "text")?,
                require_str(args, "needle")?,
            )?),
        ),
        "std.text.starts_with" => ok_map(
            "match",
            Value::Bool(pure::starts_with(
                require(args, "text")?,
                require_str(args, "prefix")?,
            )?),
        ),
        "std.text.ends_with" => ok_map(
            "match",
            Value::Bool(pure::ends_with(
                require(args, "text")?,
                require_str(args, "suffix")?,
            )?),
        ),
        "std.text.regex_match" => ok_map(
            "match",
            Value::Bool(pure::regex_match(
                require(args, "text")?,
                require_str(args, "pattern")?,
            )?),
        ),
        "std.text.regex_extract" => {
            let captured = pure::regex_capture(
                require(args, "text")?,
                require_str(args, "pattern")?,
            )?;
            ok_map("groups", captured)
        }
        "std.text.truncate" => {
            let text = require_str(args, "text")?;
            let max = require_number(args, "max_chars")? as usize;
            ok_map("text", Value::Str(truncate_chars(text, max)))
        }

        // ── validate ───────────────────────────────────────────────────────
        "std.validate.email" => validate_bool(require(args, "text")?, FormatKind::Email),
        "std.validate.phone_e164" => {
            validate_bool(require(args, "text")?, FormatKind::E164)
        }
        "std.validate.url" => validate_bool(require(args, "text")?, FormatKind::Url),
        "std.validate.uuid" => validate_bool(require(args, "text")?, FormatKind::Uuid),
        "std.validate.in_range" => {
            let value = require(args, "value")?;
            let lo = require_number(args, "lo")?;
            let hi = require_number(args, "hi")?;
            ok_map("value", pure::validate_range(value, lo, hi)?)
        }
        "std.validate.in_enum" => {
            let value = require(args, "value")?;
            let allowed = string_list(args, "allowed")?;
            let refs: Vec<&str> = allowed.iter().map(String::as_str).collect();
            ok_map("value", pure::validate_enum(value, &refs)?)
        }
        "std.validate.matches_pattern" => ok_map(
            "value",
            pure::validate_pattern(
                require(args, "text")?,
                require_str(args, "pattern")?,
            )?,
        ),
        "std.validate.normalize_phone" => {
            let region = args
                .get("region")
                .and_then(|v| v.as_str())
                .unwrap_or("US");
            ok_map(
                "phone",
                pure::normalize_phone(require(args, "text")?, region)?,
            )
        }
        "std.validate.normalize_email" => {
            ok_map("email", pure::normalize_email(require(args, "text")?)?)
        }

        // ── data ───────────────────────────────────────────────────────────
        "std.data.count" => ok_map("count", pure::count(require(args, "list")?)?),
        "std.data.first" => ok_map("item", pure::first(require(args, "list")?)?),
        "std.data.last" => ok_map("item", pure::last(require(args, "list")?)?),
        "std.data.take" => {
            let n = require_number(args, "n")? as usize;
            ok_map("list", pure::take(require(args, "list")?, n)?)
        }
        "std.data.append" => ok_map(
            "list",
            pure::append(require(args, "list")?, require(args, "item")?.clone())?,
        ),
        "std.data.pick" => {
            let keys = string_list(args, "keys")?;
            let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
            ok_map("map", pure::pick(require(args, "map")?, &refs)?)
        }
        "std.data.omit" => {
            let keys = string_list(args, "keys")?;
            let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
            ok_map("map", pure::omit(require(args, "map")?, &refs)?)
        }
        "std.data.merge" => ok_map(
            "map",
            pure::merge_shallow(require(args, "left")?, require(args, "right")?)?,
        ),
        "std.data.keys" => ok_map("keys", pure::keys(require(args, "map")?)?),
        "std.data.values" => ok_map("values", pure::values_of(require(args, "map")?)?),
        "std.data.filter_equals" => {
            let list = require_list(args, "list")?;
            let field = require_str(args, "field")?;
            let expected = require(args, "value")?;
            let filtered: Vec<Value> = list
                .into_iter()
                .filter(|item| match item {
                    Value::Map(m) => m.get(field) == Some(expected),
                    _ => false,
                })
                .collect();
            ok_map("list", Value::List(filtered))
        }
        "std.data.sort_by_field" => {
            let mut list = require_list(args, "list")?;
            let field = require_str(args, "field")?.to_string();
            let desc = args
                .get("order")
                .and_then(|v| v.as_str())
                .is_some_and(|o| o.eq_ignore_ascii_case("desc"));
            list.sort_by(|a, b| {
                let av = match a {
                    Value::Map(m) => m.get(&field),
                    _ => None,
                };
                let bv = match b {
                    Value::Map(m) => m.get(&field),
                    _ => None,
                };
                let ord = compare_values(av, bv);
                if desc { ord.reverse() } else { ord }
            });
            ok_map("list", Value::List(list))
        }
        "std.data.dedupe" => {
            let list = require_list(args, "list")?;
            let field = args.get("field").and_then(|v| v.as_str());
            let mut seen = indexmap::IndexSet::new();
            let mut out = Vec::new();
            for item in list {
                let key = dedupe_key(&item, field);
                if seen.insert(key) {
                    out.push(item);
                }
            }
            ok_map("list", Value::List(out))
        }
        "std.data.map_field" => {
            let list = require_list(args, "list")?;
            let path = require_str(args, "path")?;
            let mut values = Vec::new();
            for item in list {
                values.push(pure::get_path_op(&item, path)?);
            }
            ok_map("values", Value::List(values))
        }

        // ── control & logic ──────────────────────────────────────────────────
        "std.control.coalesce" => {
            ok_map("value", pure::coalesce(&require_list(args, "values")?))
        }
        "std.control.default_if_missing" => {
            let value = require(args, "value")?;
            let chosen = if matches!(value, Value::Null) {
                require(args, "default")?.clone()
            } else {
                value.clone()
            };
            ok_map("value", chosen)
        }
        "std.control.pick_branch" => {
            let chosen = if require_bool(args, "condition")? {
                require(args, "if_true")?.clone()
            } else {
                require(args, "if_false")?.clone()
            };
            ok_map("value", chosen)
        }
        "std.logic.and" => ok_map(
            "result",
            Value::Bool(pure::and_bools(&[
                require_bool(args, "a")?,
                require_bool(args, "b")?,
            ])),
        ),
        "std.logic.or" => ok_map(
            "result",
            Value::Bool(pure::or_bools(&[
                require_bool(args, "a")?,
                require_bool(args, "b")?,
            ])),
        ),
        "std.logic.not" => ok_map("result", Value::Bool(pure::not_bool(require_bool(args, "value")?))),

        // ── ephemeral (runtime sub-agents; pipeline destroyed after invoke) ──
        "std.ephemeral.adapt" => {
            let goal = require_str(args, "goal")?;
            let context = args
                .get("context")
                .cloned()
                .unwrap_or(Value::Map(IndexMap::new()));
            let mut scope = EphemeralScope::default();
            let (result, _) = adapt_with_scope(&mut scope, goal, &context)?;
            ok_map("result", result)
        }
        "std.ephemeral.read_document" => {
            let document = require_str(args, "document")?;
            let query = require_str(args, "query")?;
            let mut ctx = IndexMap::new();
            ctx.insert("document_text".into(), Value::Str(document.to_string()));
            let mut scope = EphemeralScope::default();
            let (result, _) = adapt_with_scope(&mut scope, query, &Value::Map(ctx))?;
            ok_map("result", result)
        }

        _ => Err(AelioError::new(
            ReasonCode::NotFound,
            format!("unknown standard tool {tool_id}"),
        )),
    }
}

fn validate_bool(value: &Value, format: FormatKind) -> AelioResult<Value> {
    let valid = pure::validate_format(value, format).is_ok();
    ok_map("valid", Value::Bool(valid))
}

fn compare_values(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    match (a, b) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(av), Some(bv)) => {
            if let (Some(a), Some(b)) = (av.as_f64(), bv.as_f64()) {
                a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
            } else if let (Some(a), Some(b)) = (av.as_str(), bv.as_str()) {
                a.cmp(b)
            } else {
                std::cmp::Ordering::Equal
            }
        }
    }
}

fn dedupe_key(item: &Value, field: Option<&str>) -> String {
    if let Some(field) = field {
        if let Value::Map(m) = item {
            return format!("{:?}", m.get(field));
        }
    }
    format!("{item:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(pairs: &[(&str, Value)]) -> IndexMap<String, Value> {
        pairs.iter().map(|(k, v)| ((*k).into(), v.clone())).collect()
    }

    #[test]
    fn text_and_validate_tools_work() {
        let strip = invoke_standard_tool_inner(
            "std.text.strip",
            &args(&[("text", Value::Str("  hi  ".into()))]),
        )
        .unwrap();
        if let Value::Map(map) = strip {
            assert_eq!(map.get("text"), Some(&Value::Str("hi".into())));
        } else {
            panic!("expected map");
        }

        let valid = invoke_standard_tool_inner(
            "std.validate.email",
            &args(&[("text", Value::Str("a@b.co".into()))]),
        )
        .unwrap();
        if let Value::Map(map) = valid {
            assert_eq!(map.get("valid"), Some(&Value::Bool(true)));
        } else {
            panic!("expected map");
        }
    }

    #[test]
    fn data_filter_and_coalesce_work() {
        let list = Value::List(vec![
            Value::Map([("status".into(), Value::Str("paid".into()))].into()),
            Value::Map([("status".into(), Value::Str("open".into()))].into()),
        ]);
        let filtered = invoke_standard_tool_inner(
            "std.data.filter_equals",
            &args(&[
                ("list", list),
                ("field", Value::Str("status".into())),
                ("value", Value::Str("paid".into())),
            ]),
        )
        .unwrap();
        if let Value::Map(m) = filtered {
            if let Some(Value::List(items)) = m.get("list") {
                assert_eq!(items.len(), 1);
            } else {
                panic!("expected list output");
            }
        }

        let coalesced = invoke_standard_tool_inner(
            "std.control.coalesce",
            &args(&[(
                "values",
                Value::List(vec![Value::Null, Value::Int(42)]),
            )]),
        )
        .unwrap();
        if let Value::Map(map) = coalesced {
            assert_eq!(map.get("value"), Some(&Value::Int(42)));
        } else {
            panic!("expected map");
        }
    }
}
