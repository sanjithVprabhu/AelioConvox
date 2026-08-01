//! Natural-language query compilation: English → a structured query → the engine.
//!
//! The NL layer is a thin, schema-grounded translation step, not a storage mode. An
//! [`LlmClient`] is shown the table's real columns and the JSON schema of the query tool, and
//! must respond with a tool call whose input is a [`QueryRequest`](crate::dto::QueryRequest) —
//! so it cannot invent columns or syntax, and the result runs through the exact same planner
//! as a hand-written query. The model's output is returned alongside the results for
//! transparency.
//!
//! Scope today: fuaelio-db-text (`text`), scalar `filters`, and `graph` reachability. Semantic
//! vector search from natural language needs an embedding step (turn the user's words into a
//! query vector) — a pluggable embedder is the next increment; until then the vector clause is
//! intentionally absent from the NL tool.

use aelio_db_query::ColumnKind;
use serde_json::{json, Value};

pub use crate::BoxFuture;

/// A backend that turns a natural-language request into a structured query object.
///
/// Implementors receive the system prompt (schema-grounded), the user's words, and the JSON
/// schema of the query tool; they return the tool's input — a `QueryRequest` as JSON. The
/// real implementation calls an LLM; tests inject a canned one.
pub trait LlmClient: Send + Sync {
    fn translate<'a>(
        &'a self,
        system: String,
        user: String,
        tool_schema: Value,
    ) -> BoxFuture<'a, Result<Value, String>>;
}

/// Human-facing label for a column kind (also used by the `/schema` endpoint).
pub fn kind_label(kind: ColumnKind) -> (&'static str, Option<u16>) {
    match kind {
        ColumnKind::Bool => ("bool", None),
        ColumnKind::I64 => ("i64", None),
        ColumnKind::F64 => ("f64", None),
        ColumnKind::Utf8 => ("utf8", None),
        ColumnKind::Timestamp => ("timestamp", None),
        ColumnKind::Text => ("text", None),
        ColumnKind::Edge => ("edge", None),
        ColumnKind::Vector(d) => ("vector", Some(d)),
    }
}

/// How a column can participate in a natural-language query (the hint given to the model).
/// When an embedder is configured, vector columns become usable via the `semantic` clause.
fn clause_hint(kind: ColumnKind, semantic_enabled: bool) -> &'static str {
    match kind {
        ColumnKind::Text => "fuaelio-db-text search via the `text` clause (BM25)",
        ColumnKind::Edge => "graph reachability via the `graph` clause",
        ColumnKind::Vector(_) if semantic_enabled => {
            "semantic similarity via the `semantic` clause (the server embeds your text)"
        }
        ColumnKind::Vector(_) => {
            "semantic vector search — NOT available from natural language; ignore for queries"
        }
        _ => "scalar filtering via the `filters` clause",
    }
}

/// The system prompt: the table's real schema plus strict instructions to only emit a tool
/// call over existing columns. `semantic_enabled` advertises the `semantic` clause for vector
/// columns (only when an embedder is configured).
pub fn system_prompt(table: &str, cols: &[(String, ColumnKind)], semantic_enabled: bool) -> String {
    let mut s = String::new();
    s.push_str(
        "You translate a user's natural-language request into a structured database query by \
         calling the `run_query` tool. Use ONLY the columns listed below, by their exact names. \
         Never invent columns. If the request implies a relevance/keyword search, use the \
         `text` clause on a text column; for meaning/topic similarity use the `semantic` clause \
         on a vector column when available; for numeric/categorical constraints use `filters`; \
         for \"reachable from\"/relationship constraints use `graph`. Always set a reasonable \
         `k` (default 10).\n\n",
    );
    s.push_str(&format!("Table `{table}` columns:\n"));
    for (name, kind) in cols {
        let (label, dim) = kind_label(*kind);
        let dim = dim.map(|d| format!("({d})")).unwrap_or_default();
        s.push_str(&format!(
            "- `{name}`: {label}{dim} — {}\n",
            clause_hint(*kind, semantic_enabled)
        ));
    }
    s
}

/// The JSON schema for the `run_query` tool's input — a `QueryRequest` restricted to the
/// clauses the NL layer supports (text / filters / graph, plus `semantic` when an embedder is
/// configured). Kept in lockstep with `crate::dto::QueryRequest`.
pub fn query_tool_schema(semantic_enabled: bool) -> Value {
    let mut props = json!({
        "k": { "type": "integer", "description": "number of results to return", "minimum": 1 },
        "text": {
            "type": "object",
            "description": "BM25 fuaelio-db-text search on a text column",
            "properties": {
                "col": { "type": "string" },
                "query": { "type": "string" }
            },
            "required": ["col", "query"]
        },
        "filters": {
            "type": "array",
            "description": "scalar predicates, ANDed together",
            "items": {
                "type": "object",
                "properties": {
                    "col": { "type": "string" },
                    "op": { "type": "string", "enum": ["eq", "ne", "gt", "ge", "lt", "le"] },
                    "value": {
                        "type": "object",
                        "description": "a tagged value, e.g. {\"type\":\"i64\",\"value\":2020}",
                        "properties": {
                            "type": { "type": "string", "enum": ["bool", "i64", "f64", "utf8"] },
                            "value": {}
                        },
                        "required": ["type", "value"]
                    }
                },
                "required": ["col", "op", "value"]
            }
        },
        "graph": {
            "type": "object",
            "description": "rows reachable from the seed row ids along an edge column",
            "properties": {
                "col": { "type": "string" },
                "seeds": { "type": "array", "items": { "type": "integer" } },
                "depth": { "type": "integer", "minimum": 1 }
            },
            "required": ["col", "seeds", "depth"]
        }
    });

    if semantic_enabled {
        // The server embeds `text` into a query vector for `col` — semantic similarity search
        // without the client computing embeddings.
        props["semantic"] = json!({
            "type": "object",
            "description": "semantic similarity search: the server embeds `text` into a vector for `col`",
            "properties": {
                "col": { "type": "string" },
                "text": { "type": "string" }
            },
            "required": ["col", "text"]
        });
    }

    json!({ "type": "object", "properties": props, "required": ["k"] })
}

/// The real LLM backend: the Anthropic Messages API with a forced `run_query` tool call.
pub struct AnthropicClient {
    http: reqwest::Client,
    api_key: String,
    model: String,
}

impl AnthropicClient {
    /// Build from an API key. The model defaults to `claude-sonnet-4-6` (override with the
    /// `ANTHROPIC_MODEL` env var at the call site).
    pub fn new(api_key: String, model: Option<String>) -> Self {
        AnthropicClient {
            http: reqwest::Client::new(),
            api_key,
            model: model.unwrap_or_else(|| "claude-sonnet-4-6".to_string()),
        }
    }
}

impl LlmClient for AnthropicClient {
    fn translate<'a>(
        &'a self,
        system: String,
        user: String,
        tool_schema: Value,
    ) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            let body = json!({
                "model": self.model,
                "max_tokens": 1024,
                "system": system,
                "messages": [{ "role": "user", "content": user }],
                "tools": [{
                    "name": "run_query",
                    "description": "Run a structured query against the database.",
                    "input_schema": tool_schema
                }],
                "tool_choice": { "type": "tool", "name": "run_query" }
            });
            let resp = self
                .http
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("anthropic request failed: {e}"))?;
            let status = resp.status();
            let v: Value = resp
                .json()
                .await
                .map_err(|e| format!("anthropic response not JSON: {e}"))?;
            if !status.is_success() {
                return Err(format!("anthropic API error {status}: {v}"));
            }
            // Extract the forced tool call's input.
            let content = v
                .get("content")
                .and_then(|c| c.as_array())
                .ok_or_else(|| format!("anthropic response missing content: {v}"))?;
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    return block
                        .get("input")
                        .cloned()
                        .ok_or_else(|| "tool_use block missing input".to_string());
                }
            }
            Err(format!("model did not call run_query: {v}"))
        })
    }
}
