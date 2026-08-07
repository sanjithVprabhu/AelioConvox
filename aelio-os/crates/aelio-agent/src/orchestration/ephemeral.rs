//! Runtime ephemeral sub-agents — synthesize, execute, destroy.
//!
//! When no pinned workflow or registered tool matches a TaskNode goal, the orchestrator
//! mints a short-lived pipeline of `std.*` steps. The pipeline exists only for the
//! current invocation (RAII); nothing is persisted to the tenant registry.

use crate::orchestration::stdlib::invoke_standard_tool;
use crate::orchestration::pins::extract_numbers;
use crate::types::{AelioError, AelioResult, ReasonCode, Value};
use indexmap::IndexMap;

/// One step in a synthesized micro-plan.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineStep {
    pub tool_id: String,
    pub args: IndexMap<String, Value>,
}

/// Ephemeral tool body — destroyed when the scope ends.
#[derive(Debug, Clone, PartialEq)]
pub struct EphemeralPipeline {
    pub id: String,
    pub goal: String,
    pub steps: Vec<PipelineStep>,
}

/// Turn-scoped ledger of ephemeral pipelines created and destroyed this orchestration.
#[derive(Debug, Default, Clone)]
pub struct EphemeralScope {
    pub created: Vec<String>,
    pub destroyed: Vec<String>,
}

impl EphemeralScope {
    /// Mint a pipeline id from the scope sequence and the goal, never from the clock: these ids
    /// reach decision-log traces, and a wall-clock component would make every replay diverge.
    pub fn mint(&mut self, goal: &str, steps: Vec<PipelineStep>) -> EphemeralPipeline {
        let seq = self.created.len();
        let digest = aelio_sol::blake3_hex(goal.as_bytes());
        let id = format!("ephemeral.{seq}.{}", &digest[..12.min(digest.len())]);
        self.created.push(id.clone());
        EphemeralPipeline {
            id,
            goal: goal.to_string(),
            steps,
        }
    }

    pub fn destroy(&mut self, pipeline: &EphemeralPipeline) {
        if !self.destroyed.contains(&pipeline.id) {
            self.destroyed.push(pipeline.id.clone());
        }
    }

    pub fn active_count(&self) -> usize {
        self.created.len().saturating_sub(self.destroyed.len())
    }
}

/// Synthesize a pure std-tool pipeline for a generic goal + context.
pub fn synthesize_pipeline(goal: &str, context: &Value) -> EphemeralPipeline {
    if crate::orchestration::tool_router::looks_like_client_effect(goal) {
        return EphemeralPipeline {
            id: String::new(),
            goal: goal.to_string(),
            steps: vec![],
        };
    }
    let lower = goal.to_lowercase();
    let steps = if let Some(doc) = document_text(context) {
        if mentions_document_task(&lower) {
            document_query_steps(goal, &doc)
        } else {
            default_steps(goal, context)
        }
    } else if mentions_text_transform(&lower) {
        text_transform_steps(goal, context)
    } else if mentions_count(&lower) && list_in_context(context).is_some() {
        vec![PipelineStep {
            tool_id: "std.data.count".into(),
            args: step_args(&[("list", list_in_context(context).unwrap())]),
        }]
    } else if (lower.contains("average") && list_in_context(context).is_some())
        || !extract_numbers(goal).is_empty()
    {
        average_steps(context)
    } else {
        default_steps(goal, context)
    };

    EphemeralPipeline {
        id: String::new(), // filled by scope.mint
        goal: goal.to_string(),
        steps,
    }
}

/// Execute a pipeline locally, returning the last step's primary output field.
pub fn run_pipeline(pipeline: &EphemeralPipeline, seed_context: &Value) -> AelioResult<Value> {
    if pipeline.steps.is_empty() {
        return Err(AelioError::new(
            ReasonCode::Validation,
            "ephemeral pipeline refused: query requires a client-registered tool",
        ));
    }
    let mut bindings: IndexMap<String, Value> = context_map(seed_context);
    let mut last: Option<Value> = None;

    for (idx, step) in pipeline.steps.iter().enumerate() {
        let args = resolve_step_args(&step.args, &bindings)?;
        let output = invoke_standard_tool(&step.tool_id, &args)
            .ok_or_else(|| {
                AelioError::new(
                    ReasonCode::NotFound,
                    format!("ephemeral step tool `{}` unavailable", step.tool_id),
                )
            })??;
        let primary = extract_primary_output(&output);
        bindings.insert(format!("step_{idx}"), primary.clone());
        bindings.insert("last".into(), primary.clone());
        last = Some(primary);
    }

    last.ok_or_else(|| {
        AelioError::new(
            ReasonCode::Validation,
            "ephemeral pipeline produced no output",
        )
    })
}

fn resolve_step_args(
    template: &IndexMap<String, Value>,
    bindings: &IndexMap<String, Value>,
) -> AelioResult<IndexMap<String, Value>> {
    let mut args = IndexMap::new();
    for (key, value) in template {
        args.insert(key.clone(), resolve_value(value, bindings)?);
    }
    Ok(args)
}

fn resolve_value(value: &Value, bindings: &IndexMap<String, Value>) -> AelioResult<Value> {
    match value {
        Value::Str(s) if s.starts_with('$') => bindings
            .get(&s[1..])
            .cloned()
            .ok_or_else(|| AelioError::new(ReasonCode::Missing, format!("unbound `{s}`"))),
        Value::Map(map) => {
            let mut out = IndexMap::new();
            for (k, v) in map {
                out.insert(k.clone(), resolve_value(v, bindings)?);
            }
            Ok(Value::Map(out))
        }
        Value::List(items) => Ok(Value::List(
            items
                .iter()
                .map(|v| resolve_value(v, bindings))
                .collect::<AelioResult<_>>()?,
        )),
        other => Ok(other.clone()),
    }
}

fn extract_primary_output(value: &Value) -> Value {
    match value {
        Value::Map(map) => map
            .values()
            .next()
            .cloned()
            .unwrap_or_else(|| value.clone()),
        other => other.clone(),
    }
}

fn context_map(context: &Value) -> IndexMap<String, Value> {
    match context {
        Value::Map(map) => map.clone(),
        other => {
            let mut map = IndexMap::new();
            map.insert("value".into(), other.clone());
            map
        }
    }
}

fn document_text(context: &Value) -> Option<String> {
    let map = context_map(context);
    for key in ["document_text", "document", "attached_text", "text"] {
        if let Some(Value::Str(s)) = map.get(key) {
            if !s.trim().is_empty() {
                return Some(s.clone());
            }
        }
    }
    None
}

fn list_in_context(context: &Value) -> Option<Value> {
    let map = context_map(context);
    map.get("values")
        .or_else(|| map.get("list"))
        .or_else(|| map.get("items"))
        .cloned()
}

fn mentions_document_task(lower: &str) -> bool {
    [
        "document", "attached", "file", "pdf", "read", "extract", "summarize", "summary",
        "find in", "search", "what does", "what is in",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
}

fn mentions_text_transform(lower: &str) -> bool {
    lower.contains("uppercase")
        || lower.contains("upper case")
        || lower.contains("lowercase")
        || lower.contains("lower case")
        || lower.contains("trim")
        || lower.contains("strip")
}

fn mentions_count(lower: &str) -> bool {
    lower.contains("how many") || lower.contains("count")
}

fn step_args(pairs: &[(&str, Value)]) -> IndexMap<String, Value> {
    pairs.iter().map(|(k, v)| ((*k).into(), v.clone())).collect()
}

fn document_query_steps(goal: &str, document: &str) -> Vec<PipelineStep> {
    let lower = goal.to_lowercase();
    if let Some(keyword) = extract_quoted_or_after(goal, &["find", "search for", "containing"]) {
        vec![
            PipelineStep {
                tool_id: "std.text.contains".into(),
                args: step_args(&[
                    ("text", Value::Str(document.into())),
                    ("needle", Value::Str(keyword.clone())),
                ]),
            },
            PipelineStep {
                tool_id: "std.text.regex_extract".into(),
                args: step_args(&[
                    ("text", Value::Str(document.into())),
                    ("pattern", Value::Str(format!("(?i){}", regex_escape(&keyword)))),
                ]),
            },
        ]
    } else if lower.contains("word") {
        vec![PipelineStep {
            tool_id: "std.text.split_words".into(),
            args: step_args(&[("text", Value::Str(document.into()))]),
        }]
    } else if lower.contains("length") || lower.contains("how long") {
        vec![PipelineStep {
            tool_id: "std.text.length".into(),
            args: step_args(&[("text", Value::Str(document.into()))]),
        }]
    } else {
        vec![PipelineStep {
            tool_id: "std.text.truncate".into(),
            args: step_args(&[
                ("text", Value::Str(document.into())),
                ("max_chars", Value::Int(500)),
            ]),
        }]
    }
}

fn text_transform_steps(goal: &str, context: &Value) -> Vec<PipelineStep> {
    let text = document_text(context)
        .or_else(|| {
            context_map(context)
                .get("text")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| goal.to_string());
    let tool = if goal.to_lowercase().contains("upper") {
        "std.text.upper"
    } else if goal.to_lowercase().contains("lower") {
        "std.text.lower"
    } else {
        "std.text.strip"
    };
    vec![PipelineStep {
        tool_id: tool.into(),
        args: step_args(&[("text", Value::Str(text))]),
    }]
}

fn average_steps(context: &Value) -> Vec<PipelineStep> {
    let values = list_in_context(context).unwrap_or(Value::List(vec![]));
    vec![PipelineStep {
        tool_id: "std.math.average".into(),
        args: step_args(&[("values", values)]),
    }]
}

/// Last resort when no std step fits the goal: refuse by returning no steps.
///
/// `run_pipeline` errors on an empty pipeline and the turn falls through to ProposePath. The
/// previous behaviour — coalescing the utterance — "answered" the user with their own question.
fn default_steps(goal: &str, context: &Value) -> Vec<PipelineStep> {
    if let Some(text) = document_text(context) {
        return document_query_steps(goal, &text);
    }
    vec![]
}

fn extract_quoted_or_after(goal: &str, prefixes: &[&str]) -> Option<String> {
    if let Some(start) = goal.find('"') {
        let rest = &goal[start + 1..];
        if let Some(end) = rest.find('"') {
            let quoted = rest[..end].trim();
            if !quoted.is_empty() {
                return Some(quoted.to_string());
            }
        }
    }
    let lower = goal.to_lowercase();
    for prefix in prefixes {
        if let Some(idx) = lower.find(prefix) {
            let tail = goal[idx + prefix.len()..].trim();
            let word = tail.split_whitespace().next().unwrap_or("").trim_matches(|c: char| !c.is_alphanumeric());
            if !word.is_empty() {
                return Some(word.to_string());
            }
        }
    }
    None
}

fn regex_escape(s: &str) -> String {
    s.chars()
        .map(|c| {
            if ".^$|?*+()[]{}\\".contains(c) {
                format!("\\{c}")
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// Run adapt: mint pipeline, execute, destroy — the full ephemeral lifecycle.
pub fn adapt_with_scope(
    scope: &mut EphemeralScope,
    goal: &str,
    context: &Value,
) -> AelioResult<(Value, String)> {
    let mut pipeline = synthesize_pipeline(goal, context);
    let pipeline_id = {
        let minted = scope.mint(goal, std::mem::take(&mut pipeline.steps));
        pipeline.id = minted.id.clone();
        pipeline.steps = minted.steps;
        minted.id
    };
    let result = run_pipeline(&pipeline, context);
    scope.destroy(&pipeline);
    result.map(|value| (value, pipeline_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ephemeral_scope_tracks_lifecycle() {
        let mut scope = EphemeralScope::default();
        let pipeline = scope.mint("test", vec![]);
        assert_eq!(scope.active_count(), 1);
        scope.destroy(&pipeline);
        assert_eq!(scope.active_count(), 0);
        assert_eq!(scope.destroyed.len(), 1);
    }

    #[test]
    fn document_query_pipeline_runs_and_destroys() {
        let mut scope = EphemeralScope::default();
        let context = Value::Map(
            [(
                "document_text".into(),
                Value::Str("Invoice total is 42 dollars".into()),
            )]
            .into(),
        );
        let (result, id) = adapt_with_scope(
            &mut scope,
            "how many words in the attached document",
            &context,
        )
        .expect("adapt");
        assert!(id.starts_with("ephemeral."));
        assert_eq!(scope.active_count(), 0);
        assert!(matches!(result, Value::List(_)));
    }

    #[test]
    fn synthesize_average_from_context_values() {
        let context = Value::Map(
            [(
                "values".into(),
                Value::List(vec![Value::Int(10), Value::Int(20), Value::Int(30)]),
            )]
            .into(),
        );
        let pipeline = synthesize_pipeline("calculate average", &context);
        assert!(pipeline.steps.iter().any(|s| s.tool_id == "std.math.average"));
    }
}
