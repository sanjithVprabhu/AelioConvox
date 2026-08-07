//! ToolSpec definitions for all `std.*` standard tools.

use crate::tenant::{
    OutputField, OutputSpec, ParamSource, ParamSpec, ToolContract, ToolEffect, ToolSpec,
};
use crate::types::Sensitivity;
use indexmap::IndexMap;

fn param(name: &str, type_name: &str, required: bool) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        type_name: type_name.into(),
        required,
        constraint: None,
        source: ParamSource::Slot { name: name.into() },
        repair: None,
        prompt_hint: None,
        sensitivity: Sensitivity::None,
        default: None,
        depends_on: vec![],
    }
}

fn tool(
    id: &str,
    capability: &str,
    params: Vec<ParamSpec>,
    outputs: Vec<(&str, &str)>,
) -> ToolSpec {
    let mut fields = IndexMap::new();
    for (field, type_name) in outputs {
        fields.insert(
            field.into(),
            OutputField {
                path: field.into(),
                type_name: type_name.into(),
                sensitivity: Sensitivity::None,
                meaning: format!("Output of {id}"),
            },
        );
    }
    ToolSpec {
        id: id.into(),
        name: id.rsplit('.').next().unwrap_or(id).into(),
        version: "1".into(),
        capability_tags: vec![capability.into()],
        // Standard tools execute over their full in-memory inputs, so their result sets are
        // complete. Their output is an operation result rather than an inferred domain entity.
        contract: Some(ToolContract::complete_read(format!("std:{id}:result"))),
        effect: Some(ToolEffect::Pure),
        effectful: false,
        idempotent: true,
        dry_run_available: true,
        params,
        output_semantics: OutputSpec {
            fields,
            role_hint: Some("data".into()),
        },
        continuations: vec![],
        errors: vec![],
    }
}

pub fn standard_tool_specs() -> Vec<ToolSpec> {
    let mut specs = vec![];

    // ── math (core + extended) ─────────────────────────────────────────────
    for id in [
        "std.math.add",
        "std.math.subtract",
        "std.math.multiply",
        "std.math.divide",
        "std.math.sum",
        "std.math.average",
        "std.math.min",
        "std.math.max",
    ] {
        let (params, out) = match id {
            "std.math.add" | "std.math.subtract" | "std.math.divide" => (
                vec![param("a", "number", true), param("b", "number", true)],
                "result",
            ),
            "std.math.multiply" | "std.math.sum" | "std.math.average" | "std.math.min"
            | "std.math.max" => (vec![param("values", "list", true)], match id {
                "std.math.sum" => "total",
                "std.math.average" => "average",
                _ => "result",
            }),
            _ => unreachable!(),
        };
        specs.push(tool(id, id, params, vec![(out, "number")]));
    }
    // Ephemeral meta-tools — mint short-lived pipelines; synthesized body is destroyed after run.
    specs.push(tool(
        "std.ephemeral.adapt",
        "std.ephemeral.adapt",
        vec![param("goal", "string", true), param("context", "map", false)],
        vec![("result", "any")],
    ));
    specs.push(tool(
        "std.ephemeral.read_document",
        "std.ephemeral.read_document",
        vec![
            param("document", "string", true),
            param("query", "string", true),
        ],
        vec![("result", "any")],
    ));
    specs.push(tool(
        "std.math.clamp",
        "std.math.clamp",
        vec![
            param("value", "number", true),
            param("lo", "number", true),
            param("hi", "number", true),
        ],
        vec![("result", "number")],
    ));
    specs.push(tool(
        "std.math.percent_of",
        "std.math.percent_of",
        vec![param("part", "number", true), param("whole", "number", true)],
        vec![("result", "number")],
    ));
    specs.push(tool(
        "std.math.modulo",
        "std.math.modulo",
        vec![param("a", "number", true), param("b", "number", true)],
        vec![("result", "number")],
    ));
    specs.push(tool(
        "std.math.abs",
        "std.math.abs",
        vec![param("value", "number", true)],
        vec![("result", "number")],
    ));
    specs.push(tool(
        "std.math.compare",
        "std.math.compare",
        vec![param("a", "number", true), param("b", "number", true)],
        vec![("relation", "string")],
    ));

    // ── text ───────────────────────────────────────────────────────────────
    specs.push(tool(
        "std.text.length",
        "std.text.length",
        vec![param("text", "string", true)],
        vec![("length", "number")],
    ));
    specs.push(tool(
        "std.text.concat",
        "std.text.concat",
        vec![param("parts", "list", true)],
        vec![("text", "string")],
    ));
    specs.push(tool(
        "std.text.join",
        "std.text.join",
        vec![param("parts", "list", true), param("separator", "string", true)],
        vec![("text", "string")],
    ));
    specs.push(tool(
        "std.text.split",
        "std.text.split",
        vec![
            param("text", "string", true),
            param("delimiter", "string", true),
        ],
        vec![("parts", "list")],
    ));
    specs.push(tool(
        "std.text.split_words",
        "std.text.split_words",
        vec![param("text", "string", true)],
        vec![("parts", "list")],
    ));
    for id in [
        "std.text.strip",
        "std.text.lower",
        "std.text.upper",
    ] {
        specs.push(tool(id, id, vec![param("text", "string", true)], vec![("text", "string")]));
    }
    for id in [
        "std.text.contains",
        "std.text.starts_with",
        "std.text.ends_with",
        "std.text.regex_match",
    ] {
        let second = if id.ends_with("contains") {
            "needle"
        } else if id.ends_with("starts_with") {
            "prefix"
        } else if id.ends_with("ends_with") {
            "suffix"
        } else {
            "pattern"
        };
        specs.push(tool(
            id,
            id,
            vec![param("text", "string", true), param(second, "string", true)],
            vec![("match", "bool")],
        ));
    }
    specs.push(tool(
        "std.text.regex_extract",
        "std.text.regex_extract",
        vec![param("text", "string", true), param("pattern", "string", true)],
        vec![("groups", "list")],
    ));
    specs.push(tool(
        "std.text.truncate",
        "std.text.truncate",
        vec![param("text", "string", true), param("max_chars", "number", true)],
        vec![("text", "string")],
    ));

    // ── validate ───────────────────────────────────────────────────────────
    for id in [
        "std.validate.email",
        "std.validate.phone_e164",
        "std.validate.url",
        "std.validate.uuid",
    ] {
        specs.push(tool(
            id,
            id,
            vec![param("text", "string", true)],
            vec![("valid", "bool")],
        ));
    }
    specs.push(tool(
        "std.validate.in_range",
        "std.validate.in_range",
        vec![
            param("value", "number", true),
            param("lo", "number", true),
            param("hi", "number", true),
        ],
        vec![("value", "number")],
    ));
    specs.push(tool(
        "std.validate.in_enum",
        "std.validate.in_enum",
        vec![param("value", "string", true), param("allowed", "list", true)],
        vec![("value", "string")],
    ));
    specs.push(tool(
        "std.validate.matches_pattern",
        "std.validate.matches_pattern",
        vec![param("text", "string", true), param("pattern", "string", true)],
        vec![("value", "string")],
    ));
    specs.push(tool(
        "std.validate.normalize_phone",
        "std.validate.normalize_phone",
        vec![
            param("text", "string", true),
            param("region", "string", false),
        ],
        vec![("phone", "string")],
    ));
    specs.push(tool(
        "std.validate.normalize_email",
        "std.validate.normalize_email",
        vec![param("text", "string", true)],
        vec![("email", "string")],
    ));

    // ── data ───────────────────────────────────────────────────────────────
    specs.push(tool(
        "std.data.count",
        "std.data.count",
        vec![param("list", "list", true)],
        vec![("count", "number")],
    ));
    specs.push(tool(
        "std.data.first",
        "std.data.first",
        vec![param("list", "list", true)],
        vec![("item", "any")],
    ));
    specs.push(tool(
        "std.data.last",
        "std.data.last",
        vec![param("list", "list", true)],
        vec![("item", "any")],
    ));
    specs.push(tool(
        "std.data.take",
        "std.data.take",
        vec![param("list", "list", true), param("n", "number", true)],
        vec![("list", "list")],
    ));
    specs.push(tool(
        "std.data.append",
        "std.data.append",
        vec![param("list", "list", true), param("item", "any", true)],
        vec![("list", "list")],
    ));
    specs.push(tool(
        "std.data.pick",
        "std.data.pick",
        vec![param("map", "map", true), param("keys", "list", true)],
        vec![("map", "map")],
    ));
    specs.push(tool(
        "std.data.omit",
        "std.data.omit",
        vec![param("map", "map", true), param("keys", "list", true)],
        vec![("map", "map")],
    ));
    specs.push(tool(
        "std.data.merge",
        "std.data.merge",
        vec![param("left", "map", true), param("right", "map", true)],
        vec![("map", "map")],
    ));
    specs.push(tool(
        "std.data.keys",
        "std.data.keys",
        vec![param("map", "map", true)],
        vec![("keys", "list")],
    ));
    specs.push(tool(
        "std.data.values",
        "std.data.values",
        vec![param("map", "map", true)],
        vec![("values", "list")],
    ));
    specs.push(tool(
        "std.data.filter_equals",
        "std.data.filter_equals",
        vec![
            param("list", "list", true),
            param("field", "string", true),
            param("value", "any", true),
        ],
        vec![("list", "list")],
    ));
    specs.push(tool(
        "std.data.sort_by_field",
        "std.data.sort_by_field",
        vec![
            param("list", "list", true),
            param("field", "string", true),
            param("order", "string", false),
        ],
        vec![("list", "list")],
    ));
    specs.push(tool(
        "std.data.dedupe",
        "std.data.dedupe",
        vec![
            param("list", "list", true),
            param("field", "string", false),
        ],
        vec![("list", "list")],
    ));
    specs.push(tool(
        "std.data.map_field",
        "std.data.map_field",
        vec![param("list", "list", true), param("path", "string", true)],
        vec![("values", "list")],
    ));

    // ── control & logic ────────────────────────────────────────────────────
    specs.push(tool(
        "std.control.coalesce",
        "std.control.coalesce",
        vec![param("values", "list", true)],
        vec![("value", "any")],
    ));
    specs.push(tool(
        "std.control.default_if_missing",
        "std.control.default_if_missing",
        vec![param("value", "any", true), param("default", "any", true)],
        vec![("value", "any")],
    ));
    specs.push(tool(
        "std.control.pick_branch",
        "std.control.pick_branch",
        vec![
            param("condition", "bool", true),
            param("if_true", "any", true),
            param("if_false", "any", true),
        ],
        vec![("value", "any")],
    ));
    for id in ["std.logic.and", "std.logic.or"] {
        specs.push(tool(
            id,
            id,
            vec![param("a", "bool", true), param("b", "bool", true)],
            vec![("result", "bool")],
        ));
    }
    specs.push(tool(
        "std.logic.not",
        "std.logic.not",
        vec![param("value", "bool", true)],
        vec![("result", "bool")],
    ));

    specs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_expected_tool_count() {
        let specs = standard_tool_specs();
        assert!(
            specs.len() >= 55,
            "expected at least 55 standard tools, got {}",
            specs.len()
        );
        assert!(specs.iter().all(|t| t.id.starts_with("std.")));
    }
}
