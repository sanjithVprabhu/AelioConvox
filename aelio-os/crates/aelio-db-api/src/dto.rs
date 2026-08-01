//! Wire types for the HTTP/JSON API and their conversions to the engine's domain types.
//!
//! These DTOs are deliberately separate from `aelio-db-query`'s internal types: they give the API a
//! stable, friendly JSON shape (and keep `serde` out of the engine crates). A value is tagged
//! so its type is unambiguous on the wire:
//!
//! ```json
//! {"type": "i64",    "value": 2020}
//! {"type": "f64",    "value": 1.5}
//! {"type": "utf8",   "value": "hello"}
//! {"type": "vector", "value": [0.1, 0.2]}
//! {"type": "edges",  "value": [1, 2, 3]}
//! {"type": "null"}
//! ```

use aelio_db_query::{ColumnKind, HybridQuery, PredOp, Value};
use aelio_query::MAX_QUERY_LIMIT;
use serde::{Deserialize, Serialize};

/// A JSON-friendly, type-tagged column value. `embed` is an input-only convenience: a string
/// to be turned into a vector by the server's embedder before insert (`{"type":"embed",
/// "value":"some text"}`); it is resolved away in the row handlers, never stored as-is.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum ApiValue {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    Utf8(String),
    Vector(Vec<f32>),
    Edges(Vec<u64>),
    Embed(String),
}

impl From<ApiValue> for Value {
    fn from(v: ApiValue) -> Self {
        match v {
            ApiValue::Null => Value::Null,
            ApiValue::Bool(b) => Value::Bool(b),
            ApiValue::I64(i) => Value::I64(i),
            ApiValue::F64(f) => Value::F64(f),
            ApiValue::Utf8(s) => Value::Utf8(s),
            ApiValue::Vector(v) => Value::Vector(v),
            ApiValue::Edges(e) => Value::Edges(e),
            // Resolved to a Vector by the row handlers before conversion; if one reaches here
            // (e.g. used in a filter, where it makes no sense) it is treated as null.
            ApiValue::Embed(_) => Value::Null,
        }
    }
}

impl From<Value> for ApiValue {
    fn from(v: Value) -> Self {
        match v {
            Value::Null => ApiValue::Null,
            Value::Bool(b) => ApiValue::Bool(b),
            Value::I64(i) => ApiValue::I64(i),
            Value::F64(f) => ApiValue::F64(f),
            Value::Utf8(s) => ApiValue::Utf8(s),
            Value::Vector(v) => ApiValue::Vector(v),
            Value::Edges(e) => ApiValue::Edges(e),
        }
    }
}

/// A column in a `CREATE TABLE` request. `kind` is one of `bool|i64|f64|utf8|timestamp|text|
/// edge|vector`; `vector` additionally requires `dim`.
#[derive(Debug, Deserialize)]
pub struct ColumnSpec {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub dim: Option<u16>,
}

impl ColumnSpec {
    pub fn to_kind(&self) -> Result<ColumnKind, String> {
        Ok(match self.kind.as_str() {
            "bool" => ColumnKind::Bool,
            "i64" => ColumnKind::I64,
            "f64" => ColumnKind::F64,
            "utf8" => ColumnKind::Utf8,
            "timestamp" => ColumnKind::Timestamp,
            "text" => ColumnKind::Text,
            "edge" => ColumnKind::Edge,
            "vector" => {
                let dim = self.dim.ok_or_else(|| {
                    format!("column '{}': vector kind requires a 'dim'", self.name)
                })?;
                ColumnKind::Vector(dim)
            }
            other => return Err(format!("column '{}': unknown kind '{other}'", self.name)),
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateTableRequest {
    pub name: String,
    pub columns: Vec<ColumnSpec>,
}

#[derive(Debug, Serialize)]
pub struct CreateTableResponse {
    pub table_id: u32,
}

/// `{"values": {"embedding": {"type":"vector","value":[...]}, "year": {"type":"i64","value":2020}}}`
#[derive(Debug, Deserialize)]
pub struct RowRequest {
    pub values: std::collections::HashMap<String, ApiValue>,
}

#[derive(Debug, Serialize)]
pub struct InsertResponse {
    pub row_id: u64,
}

#[derive(Debug, Serialize)]
pub struct MutateResponse {
    /// Whether the row existed and the mutation took effect (delete/update return false for a
    /// missing or already-deleted row).
    pub applied: bool,
}

// --- Query ---------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct VectorClause {
    pub col: String,
    pub query: Vec<f32>,
}

/// Semantic search: the server embeds `text` into a query vector for column `col`. Resolved
/// into a [`VectorClause`] by the query handlers before planning.
#[derive(Debug, Clone, Deserialize)]
pub struct SemanticClause {
    pub col: String,
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct TextClause {
    pub col: String,
    pub query: String,
}

#[derive(Debug, Deserialize)]
pub struct GraphClause {
    pub col: String,
    pub seeds: Vec<u64>,
    pub depth: usize,
}

/// `op` is one of `eq|ne|gt|ge|lt|le`.
#[derive(Debug, Deserialize)]
pub struct FilterClause {
    pub col: String,
    pub op: String,
    pub value: ApiValue,
}

impl FilterClause {
    fn op(&self) -> Result<PredOp, String> {
        Ok(match self.op.as_str() {
            "eq" => PredOp::Eq,
            "ne" => PredOp::Ne,
            "gt" => PredOp::Gt,
            "ge" => PredOp::Ge,
            "lt" => PredOp::Lt,
            "le" => PredOp::Le,
            other => return Err(format!("unknown filter op '{other}'")),
        })
    }
}

/// A hybrid query: any combination of a vector anchor, a text anchor, scalar filters, and a
/// graph reachability constraint, fused into one ranking.
#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    pub k: usize,
    #[serde(default)]
    pub vector: Option<VectorClause>,
    /// Semantic search by text; the server embeds it into `vector` before planning.
    #[serde(default)]
    pub semantic: Option<SemanticClause>,
    #[serde(default)]
    pub text: Option<TextClause>,
    #[serde(default)]
    pub filters: Vec<FilterClause>,
    #[serde(default)]
    pub graph: Option<GraphClause>,
}

impl QueryRequest {
    pub fn validate_shape(&self) -> Result<(), String> {
        if self.k == 0 || self.k > MAX_QUERY_LIMIT as usize {
            return Err(format!("query `k` must be in 1..={MAX_QUERY_LIMIT}"));
        }
        if self.filters.len() > 64 {
            return Err("query may contain at most 64 filters".into());
        }
        Ok(())
    }

    /// Build the engine's name-based [`HybridQuery`] from this request.
    pub fn to_hybrid(&self) -> Result<HybridQuery, String> {
        self.validate_shape()?;
        let mut q = HybridQuery::new(self.k);
        if let Some(v) = &self.vector {
            q = q.vector(&v.col, v.query.clone());
        }
        if let Some(t) = &self.text {
            q = q.text(&t.col, &t.query);
        }
        for f in &self.filters {
            q = q.filter(&f.col, f.op()?, f.value.clone().into());
        }
        if let Some(g) = &self.graph {
            q = q.graph(&g.col, g.seeds.clone(), g.depth);
        }
        Ok(q)
    }
}

#[derive(Debug, Serialize)]
pub struct Hit {
    pub row_id: u64,
    pub score: f32,
}

#[derive(Debug, Serialize)]
pub struct QueryResponse {
    pub results: Vec<Hit>,
}

#[derive(Debug, Serialize)]
pub struct PrismHitResponse {
    pub row_id: u64,
    pub score: f32,
    pub fields: std::collections::BTreeMap<String, ApiValue>,
}

#[derive(Debug, Serialize)]
pub struct PrismResponse {
    pub results: Vec<PrismHitResponse>,
}

#[derive(Debug, Serialize)]
pub struct ExplainResponse {
    pub plan: String,
}

// --- Natural language + schema --------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct NlRequest {
    /// The user's request in plain English.
    pub query: String,
}

/// The NL result echoes the `compiled` structured query (for transparency/debugging) next to
/// the `results`.
#[derive(Debug, Serialize)]
pub struct NlResponse {
    pub compiled: serde_json::Value,
    pub results: Vec<Hit>,
}

#[derive(Debug, Serialize)]
pub struct ColumnInfo {
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dim: Option<u16>,
}

#[derive(Debug, Serialize)]
pub struct SchemaResponse {
    pub table: String,
    pub columns: Vec<ColumnInfo>,
}

#[derive(Debug, Serialize)]
pub struct RowValueResponse {
    pub row_id: u64,
    pub values: std::collections::HashMap<String, ApiValue>,
}

#[derive(Debug, Serialize)]
pub struct ScanResponse {
    pub rows: Vec<RowValueResponse>,
}
