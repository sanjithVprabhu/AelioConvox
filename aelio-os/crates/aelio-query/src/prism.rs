//! # Prism — closed multimodal query language (v0)
//!
//! Surface (authoring) compiles to a Sol-shaped envelope that is stored, hashed, and executed.
//! Runtime never sees free query text as an injection surface: only this structure runs.
//!
//! Envelope: `from` · `match` · `where` · `select` · `limit` · `into`
//! - `select` and `limit` are mandatory (least privilege + no silent truncation).
//! - No joins, subqueries, or expressions in `where` (AND of column predicates only).
//! - Fusion is an explicit match kind (`rrf`); the engine already fuses ranks, not raw scores.
//!
//! RATIFIED by mother Amendment #4: Prism multi-match + fusion is the intended
//! flexible-read language; legacy [`QueryAst`] remains as single-modality sugar (FLAGS F-018).

use aelio_sol::{Limits, Path, SolValue};
use serde_json::Value as J;
use std::collections::{BTreeMap, BTreeSet};

use crate::{MAX_QUERY_LIMIT, MAX_TRAVERSE_DEPTH, MAX_TRAVERSE_NODES};

/// Structural limits for the authoring envelope. Output rows are separately capped by
/// [`MAX_QUERY_LIMIT`]; these bounds prevent a small-result query from carrying an unbounded
/// parser/planner payload.
pub const MAX_PRISM_MATCH_CLAUSES: usize = 8;
pub const MAX_PRISM_WHERE_CLAUSES: usize = 64;
pub const MAX_PRISM_SELECT_COLUMNS: usize = 256;
pub const MAX_PRISM_GRAPH_SEEDS: usize = 64;
pub const MAX_PRISM_NAME_BYTES: usize = 255;
pub const MAX_PRISM_QUERY_TEXT_BYTES: usize = 64 * 1024;

/// Prism query envelope — the only executable query shape for multimodal recall.
#[derive(Debug, Clone, PartialEq)]
pub struct PrismQuery {
    pub from: String,
    pub matches: Vec<MatchClause>,
    pub where_clauses: Vec<WherePred>,
    /// Mandatory projection — never "select *".
    pub select: Vec<String>,
    /// Mandatory, positive, capped.
    pub limit: u64,
    /// Bag landing path when invoked via kernel Call (optional at pure-DB layer).
    pub into: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchClause {
    /// Point / filter-only plans use empty matches + where.
    Key {
        col: String,
        value: SolValue,
    },
    Text {
        on: String,
        query: String,
    },
    /// Precomputed query vector.
    Vector {
        on: String,
        vector: Vec<f32>,
    },
    /// Embed this string at execute time with the collection's declared model.
    VectorEmbed {
        on: String,
        text: String,
    },
    Graph {
        on: String,
        seeds: Vec<u64>,
        max_depth: u64,
        max_nodes: u64,
    },
    /// Explicit fusion of ranked modalities in this plan (RRF).
    Fusion {
        method: FusionMethod,
        /// RRF constant k (typical 60).
        rrf_k: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FusionMethod {
    Rrf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WherePred {
    pub col: String,
    pub op: WhereOp,
    pub value: SolValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhereOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

/// Declared collection schema — prerequisite for deploy-time `where` / `select` / match checks.
#[derive(Debug, Clone)]
pub struct CollectionSchema {
    pub name: String,
    pub columns: BTreeMap<String, PrismColumn>,
    /// Default projection when using [`recall`] sugar.
    pub default_select: Vec<String>,
    pub default_limit: u64,
}

#[derive(Debug, Clone)]
pub struct PrismColumn {
    pub kind: PrismColKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrismColKind {
    Bool,
    Int,
    Float,
    Utf8,
    Timestamp,
    /// BM25-indexed text.
    Text,
    /// Fixed-dim vector; `model` pins the embedding space.
    Vector {
        dim: u16,
        model: String,
    },
    Edge,
}

#[derive(Default)]
pub struct CollectionRegistry {
    /// (tenant, collection) → schema
    inner: BTreeMap<(String, String), CollectionSchema>,
}

impl CollectionRegistry {
    pub fn declare(&mut self, tenant: impl Into<String>, schema: CollectionSchema) {
        self.inner
            .insert((tenant.into(), schema.name.clone()), schema);
    }

    pub fn get(&self, tenant: &str, collection: &str) -> Option<&CollectionSchema> {
        self.inner
            .get(&(tenant.to_string(), collection.to_string()))
    }
}

/// Parse a Sol/JSON Prism envelope (closed fields).
pub fn parse_prism(j: &J) -> Result<PrismQuery, String> {
    // Apply the same structural/canonical-byte limits as every other Sol-shaped query before
    // walking or allocating its nested values.
    let envelope = to_sol(j)?;
    Limits::default()
        .check(&envelope)
        .map_err(|error| format!("Prism envelope exceeds Sol limits: {error}"))?;
    let o = j.as_object().ok_or("Prism query must be an object")?;
    let allowed = ["from", "match", "where", "select", "limit", "into", "order"];
    if let Some(unknown) = o.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!("unknown Prism field `{unknown}` — closed envelope"));
    }
    if o.contains_key("order") {
        return Err("Prism v0 rejects `order` — ranking is match/fusion only".into());
    }

    let from = o
        .get("from")
        .and_then(J::as_str)
        .ok_or("Prism missing `from`")?
        .to_string();
    validate_name("from", &from)?;

    let select = o
        .get("select")
        .and_then(J::as_array)
        .ok_or("Prism `select` is mandatory (array of column names)")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| "select entries must be strings".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if select.is_empty() {
        return Err("Prism `select` must be non-empty — no select *".into());
    }
    if select.len() > MAX_PRISM_SELECT_COLUMNS {
        return Err(format!(
            "Prism `select` may contain at most {MAX_PRISM_SELECT_COLUMNS} columns"
        ));
    }

    let limit = o
        .get("limit")
        .and_then(J::as_u64)
        .filter(|&n| n > 0 && n <= MAX_QUERY_LIMIT)
        .ok_or("Prism `limit` is mandatory and must be in 1..=10000")?;

    let into = o.get("into").and_then(J::as_str).map(str::to_string);
    if let Some(path) = &into {
        Path::parse(path).map_err(|error| format!("invalid Prism `into` path: {error}"))?;
    }

    let matches = match o.get("match") {
        None => Vec::new(),
        Some(J::Array(items)) => items
            .iter()
            .map(parse_match)
            .collect::<Result<Vec<_>, _>>()?,
        Some(one) => vec![parse_match(one)?],
        // unreachable for other kinds — parse_match handles objects
    };
    if matches.len() > MAX_PRISM_MATCH_CLAUSES {
        return Err(format!(
            "Prism `match` may contain at most {MAX_PRISM_MATCH_CLAUSES} clauses"
        ));
    }

    let where_clauses = match o.get("where") {
        None => Vec::new(),
        Some(J::Array(items)) => items
            .iter()
            .map(parse_where)
            .collect::<Result<Vec<_>, _>>()?,
        Some(one) => vec![parse_where(one)?],
    };
    if where_clauses.len() > MAX_PRISM_WHERE_CLAUSES {
        return Err(format!(
            "Prism `where` may contain at most {MAX_PRISM_WHERE_CLAUSES} predicates"
        ));
    }

    let query = PrismQuery {
        from,
        matches,
        where_clauses,
        select,
        limit,
        into,
    };
    validate_prism(&query)?;
    Ok(query)
}

/// Validate a Prism value constructed programmatically. Execution calls this again so public
/// struct construction cannot bypass parser-enforced bounds.
pub fn validate_prism(q: &PrismQuery) -> Result<(), String> {
    validate_name("from", &q.from)?;
    if q.select.is_empty() {
        return Err("Prism `select` must be non-empty — no select *".into());
    }
    if q.select.iter().any(|c| c.trim().is_empty()) {
        return Err("Prism `select` entries must be non-empty column names".into());
    }
    if q.select.len() > MAX_PRISM_SELECT_COLUMNS {
        return Err(format!(
            "Prism `select` may contain at most {MAX_PRISM_SELECT_COLUMNS} columns"
        ));
    }
    let mut seen = BTreeSet::new();
    for col in &q.select {
        validate_name("select", col)?;
        if !seen.insert(col.clone()) {
            return Err(format!("Prism `select` has duplicate column `{col}`"));
        }
    }
    if q.limit == 0 || q.limit > MAX_QUERY_LIMIT {
        return Err(format!("Prism `limit` must be in 1..={MAX_QUERY_LIMIT}"));
    }
    if q.matches.len() > MAX_PRISM_MATCH_CLAUSES {
        return Err(format!(
            "Prism `match` may contain at most {MAX_PRISM_MATCH_CLAUSES} clauses"
        ));
    }
    if q.where_clauses.len() > MAX_PRISM_WHERE_CLAUSES {
        return Err(format!(
            "Prism `where` may contain at most {MAX_PRISM_WHERE_CLAUSES} predicates"
        ));
    }
    if let Some(path) = &q.into {
        Path::parse(path).map_err(|error| format!("invalid Prism `into` path: {error}"))?;
    }
    // Bare `from`+`limit`+`select` with no match/where is a truncated table dump — reject.
    if q.matches.is_empty() && q.where_clauses.is_empty() {
        return Err(
            "Prism requires at least one `match` or `where` predicate (no bare table dump)".into(),
        );
    }
    let ranked_count = q
        .matches
        .iter()
        .filter(|m| {
            matches!(
                m,
                MatchClause::Text { .. }
                    | MatchClause::Vector { .. }
                    | MatchClause::VectorEmbed { .. }
            )
        })
        .count();
    let fusion_count = q
        .matches
        .iter()
        .filter(|m| matches!(m, MatchClause::Fusion { .. }))
        .count();
    if fusion_count > 1 {
        return Err("Prism allows at most one `fusion` clause".into());
    }
    if fusion_count == 1 && ranked_count == 0 {
        return Err("Prism `fusion` requires at least one ranked match (text or vector)".into());
    }
    if ranked_count > 1 && fusion_count != 1 {
        return Err(
            "Prism multimodal ranking requires exactly one explicit `fusion` clause".into(),
        );
    }
    let text_count = q
        .matches
        .iter()
        .filter(|m| matches!(m, MatchClause::Text { .. }))
        .count();
    let vector_count = q
        .matches
        .iter()
        .filter(|m| {
            matches!(
                m,
                MatchClause::Vector { .. } | MatchClause::VectorEmbed { .. }
            )
        })
        .count();
    let graph_count = q
        .matches
        .iter()
        .filter(|m| matches!(m, MatchClause::Graph { .. }))
        .count();
    if text_count > 1 || vector_count > 1 || graph_count > 1 {
        return Err("Prism v0 allows at most one text, vector, and graph match per query".into());
    }
    for m in &q.matches {
        let col = match m {
            MatchClause::Key { col, .. } => Some(col),
            MatchClause::Text { on, .. }
            | MatchClause::Vector { on, .. }
            | MatchClause::VectorEmbed { on, .. }
            | MatchClause::Graph { on, .. } => Some(on),
            MatchClause::Fusion { .. } => None,
        };
        if let Some(col) = col {
            validate_name("match column", col)?;
        }
        if let MatchClause::Text { query, .. } = m {
            if query.trim().is_empty() {
                return Err("text match `query` must be non-empty".into());
            }
            validate_query_text("text match `query`", query)?;
        }
        if let MatchClause::VectorEmbed { text, .. } = m {
            if text.trim().is_empty() {
                return Err("vector embed text must be non-empty".into());
            }
            validate_query_text("vector embed text", text)?;
        }
        if let MatchClause::Vector { vector, .. } = m {
            if vector.is_empty() {
                return Err("vector match must be non-empty".into());
            }
            if vector.iter().any(|x| !x.is_finite()) {
                return Err("vector match must be finite (no NaN/Inf)".into());
            }
        }
        if let MatchClause::Graph {
            seeds,
            max_depth,
            max_nodes,
            ..
        } = m
        {
            if seeds.is_empty() {
                return Err("graph match `seeds` must be non-empty".into());
            }
            if seeds.len() > MAX_PRISM_GRAPH_SEEDS {
                return Err(format!(
                    "graph match may contain at most {MAX_PRISM_GRAPH_SEEDS} seeds"
                ));
            }
            if *max_depth == 0 || *max_depth > MAX_TRAVERSE_DEPTH {
                return Err(format!(
                    "graph `max_depth` must be in 1..={MAX_TRAVERSE_DEPTH}"
                ));
            }
            if *max_nodes == 0 || *max_nodes > MAX_TRAVERSE_NODES {
                return Err(format!(
                    "graph `max_nodes` must be in 1..={MAX_TRAVERSE_NODES}"
                ));
            }
            if seeds.len() as u64 > *max_nodes {
                return Err("graph `max_nodes` must be at least the number of unique seeds".into());
            }
        }
        if let MatchClause::Fusion { rrf_k, .. } = m {
            if !rrf_k.is_finite() || *rrf_k < 0.0 {
                return Err("fusion `rrf_k` must be finite and >= 0".into());
            }
            if *rrf_k > 1_000_000.0 {
                return Err("fusion `rrf_k` must be <= 1e6".into());
            }
        }
    }
    for pred in &q.where_clauses {
        validate_name("where column", &pred.col)?;
        validate_prism_scalar("where value", &pred.value)?;
    }
    for m in &q.matches {
        if let MatchClause::Key { value, .. } = m {
            validate_prism_scalar("key value", value)?;
        }
    }
    Ok(())
}

fn validate_name(context: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("Prism `{context}` must be non-empty"));
    }
    if value.len() > MAX_PRISM_NAME_BYTES {
        return Err(format!(
            "Prism `{context}` exceeds {MAX_PRISM_NAME_BYTES} UTF-8 bytes"
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(format!(
            "Prism `{context}` may not contain control characters"
        ));
    }
    Ok(())
}

fn validate_query_text(context: &str, value: &str) -> Result<(), String> {
    if value.len() > MAX_PRISM_QUERY_TEXT_BYTES {
        return Err(format!(
            "Prism {context} exceeds {MAX_PRISM_QUERY_TEXT_BYTES} UTF-8 bytes"
        ));
    }
    Ok(())
}

fn validate_prism_scalar(context: &str, value: &SolValue) -> Result<(), String> {
    match value {
        SolValue::Map(_) => {
            return Err(format!("Prism {context} may not be a map"));
        }
        SolValue::List(_) => {
            return Err(format!("Prism {context} may not be a list"));
        }
        SolValue::Null
        | SolValue::Bool(_)
        | SolValue::Int(_)
        | SolValue::Float(_)
        | SolValue::Str(_) => {}
    }
    Limits::default()
        .check(value)
        .map_err(|error| format!("Prism {context} exceeds Sol limits: {error}"))
}

fn parse_match(j: &J) -> Result<MatchClause, String> {
    let o = j.as_object().ok_or("match clause must be an object")?;
    let kind = o
        .get("kind")
        .and_then(J::as_str)
        .ok_or("match missing `kind`")?;
    match kind {
        "key" => {
            reject_unknown_fields(o, &["kind", "on", "value"], "key match")?;
            Ok(MatchClause::Key {
                col: req_str(o, "on")?,
                value: sol_req(o, "value")?,
            })
        }
        "text" => {
            reject_unknown_fields(o, &["kind", "on", "query"], "text match")?;
            Ok(MatchClause::Text {
                on: req_str(o, "on")?,
                query: req_str(o, "query")?,
            })
        }
        "vector" => {
            let has_embed = o.contains_key("embed");
            let has_vector = o.contains_key("vector");
            if has_embed == has_vector {
                return Err(
                    "vector match requires exactly one of `vector` array or `embed` string".into(),
                );
            }
            if has_embed {
                reject_unknown_fields(o, &["kind", "on", "embed"], "vector embed match")?;
                Ok(MatchClause::VectorEmbed {
                    on: req_str(o, "on")?,
                    text: req_str(o, "embed")?,
                })
            } else {
                reject_unknown_fields(o, &["kind", "on", "vector"], "vector match")?;
                let vector = o
                    .get("vector")
                    .and_then(J::as_array)
                    .ok_or("vector match needs `vector` array or `embed` string")?;
                let vector = vector
                    .iter()
                    .map(|v| {
                        v.as_f64()
                            .map(|f| f as f32)
                            .ok_or_else(|| "vector entries must be numbers".to_string())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(MatchClause::Vector {
                    on: req_str(o, "on")?,
                    vector,
                })
            }
        }
        "graph" => {
            reject_unknown_fields(
                o,
                &["kind", "on", "seeds", "max_depth", "max_nodes"],
                "graph match",
            )?;
            let seeds = o
                .get("seeds")
                .and_then(J::as_array)
                .ok_or("graph match needs `seeds`")?
                .iter()
                .map(|v| {
                    v.as_u64()
                        .ok_or_else(|| "graph seeds must be u64".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            let max_depth = o
                .get("max_depth")
                .and_then(J::as_u64)
                .filter(|&n| n > 0 && n <= MAX_TRAVERSE_DEPTH)
                .ok_or("graph `max_depth` must be in 1..=32")?;
            let max_nodes = o
                .get("max_nodes")
                .and_then(J::as_u64)
                .filter(|&n| n > 0 && n <= MAX_TRAVERSE_NODES)
                .ok_or("graph `max_nodes` must be in 1..=10000")?;
            Ok(MatchClause::Graph {
                on: req_str(o, "on")?,
                seeds,
                max_depth,
                max_nodes,
            })
        }
        "fusion" => {
            reject_unknown_fields(o, &["kind", "method", "rrf_k"], "fusion match")?;
            let method = match o.get("method").and_then(J::as_str).unwrap_or("rrf") {
                "rrf" => FusionMethod::Rrf,
                other => return Err(format!("unknown fusion method `{other}`")),
            };
            let rrf_k = o
                .get("rrf_k")
                .and_then(J::as_f64)
                .map(|f| f as f32)
                .unwrap_or(60.0);
            Ok(MatchClause::Fusion { method, rrf_k })
        }
        other => Err(format!(
            "unknown match kind `{other}` — closed set key|text|vector|graph|fusion"
        )),
    }
}

fn parse_where(j: &J) -> Result<WherePred, String> {
    let o = j.as_object().ok_or("where predicate must be an object")?;
    reject_unknown_fields(o, &["col", "op", "value"], "where predicate")?;
    let col = req_str(o, "col")?;
    let op = match o
        .get("op")
        .and_then(J::as_str)
        .ok_or("where missing `op`")?
    {
        "eq" | "=" => WhereOp::Eq,
        "ne" | "!=" => WhereOp::Ne,
        "gt" | ">" => WhereOp::Gt,
        "ge" | ">=" => WhereOp::Ge,
        "lt" | "<" => WhereOp::Lt,
        "le" | "<=" => WhereOp::Le,
        other => return Err(format!("unknown where op `{other}`")),
    };
    Ok(WherePred {
        col,
        op,
        value: sol_req(o, "value")?,
    })
}

fn reject_unknown_fields(
    o: &serde_json::Map<String, J>,
    allowed: &[&str],
    context: &str,
) -> Result<(), String> {
    if let Some(unknown) = o.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!(
            "unknown {context} field `{unknown}` — closed clause"
        ));
    }
    Ok(())
}

fn req_str(o: &serde_json::Map<String, J>, key: &str) -> Result<String, String> {
    o.get(key)
        .and_then(J::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing string field `{key}`"))
}

fn sol_req(o: &serde_json::Map<String, J>, key: &str) -> Result<SolValue, String> {
    let j = o.get(key).ok_or_else(|| format!("missing `{key}`"))?;
    to_sol(j)
}

fn to_sol(j: &J) -> Result<SolValue, String> {
    Ok(match j {
        J::Null => SolValue::Null,
        J::Bool(b) => SolValue::Bool(*b),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                SolValue::Int(i)
            } else if let Some(f) = n.as_f64() {
                SolValue::float(f).map_err(|_| "non-finite float".to_string())?
            } else {
                return Err("number out of range".into());
            }
        }
        J::String(s) => SolValue::Str(s.clone()),
        J::Array(a) => SolValue::List(a.iter().map(to_sol).collect::<Result<_, _>>()?),
        J::Object(map) => {
            let mut m = BTreeMap::new();
            for (k, v) in map {
                m.insert(k.clone(), to_sol(v)?);
            }
            SolValue::Map(m)
        }
    })
}

/// Deploy-time check against a declared collection schema.
pub fn check_prism(reg: &CollectionRegistry, tenant: &str, q: &PrismQuery) -> Result<(), String> {
    validate_prism(q)?;
    let schema = reg
        .get(tenant, &q.from)
        .ok_or_else(|| format!("collection `{}` not declared for tenant `{tenant}`", q.from))?;

    for col in &q.select {
        if !schema.columns.contains_key(col) {
            return Err(format!(
                "select column `{col}` not in collection `{}`",
                q.from
            ));
        }
    }
    for pred in &q.where_clauses {
        let Some(column) = schema.columns.get(&pred.col) else {
            return Err(format!(
                "where column `{}` not in collection `{}`",
                pred.col, q.from
            ));
        };
        check_predicate_value(&pred.col, &column.kind, pred.op, &pred.value)?;
    }
    for m in &q.matches {
        match m {
            MatchClause::Key { col, .. }
            | MatchClause::Text { on: col, .. }
            | MatchClause::Vector { on: col, .. }
            | MatchClause::VectorEmbed { on: col, .. }
            | MatchClause::Graph { on: col, .. } => {
                let Some(c) = schema.columns.get(col) else {
                    return Err(format!(
                        "match column `{col}` not in collection `{}`",
                        q.from
                    ));
                };
                match m {
                    MatchClause::Key { value, .. } => {
                        check_predicate_value(col, &c.kind, WhereOp::Eq, value)?;
                    }
                    MatchClause::Text { .. } if !matches!(c.kind, PrismColKind::Text) => {
                        return Err(format!("text match requires Text column `{col}`"));
                    }
                    MatchClause::Vector { vector, .. } => match &c.kind {
                        PrismColKind::Vector { dim, .. } if vector.len() == *dim as usize => {}
                        PrismColKind::Vector { dim, .. } => {
                            return Err(format!(
                                "vector length {} != declared dim {dim} on `{col}`",
                                vector.len()
                            ));
                        }
                        _ => return Err(format!("vector match requires Vector column `{col}`")),
                    },
                    MatchClause::VectorEmbed { .. } => {
                        if !matches!(c.kind, PrismColKind::Vector { .. }) {
                            return Err(format!(
                                "vector embed match requires Vector column `{col}`"
                            ));
                        }
                    }
                    MatchClause::Graph { .. } => {
                        if !matches!(c.kind, PrismColKind::Edge) {
                            return Err(format!("graph match requires Edge column `{col}`"));
                        }
                    }
                    _ => {}
                }
            }
            MatchClause::Fusion { .. } => {}
        }
    }
    Ok(())
}

fn check_predicate_value(
    col: &str,
    kind: &PrismColKind,
    op: WhereOp,
    value: &SolValue,
) -> Result<(), String> {
    if matches!(kind, PrismColKind::Vector { .. } | PrismColKind::Edge) {
        return Err(format!(
            "column `{col}` cannot be used as a scalar predicate"
        ));
    }
    if matches!(value, SolValue::Null) {
        return if matches!(op, WhereOp::Eq | WhereOp::Ne) {
            Ok(())
        } else {
            Err(format!("ordered comparison on `{col}` may not use null"))
        };
    }
    let compatible = matches!(
        (kind, value),
        (PrismColKind::Bool, SolValue::Bool(_))
            | (
                PrismColKind::Int | PrismColKind::Timestamp,
                SolValue::Int(_)
            )
            | (PrismColKind::Float, SolValue::Float(_))
            | (PrismColKind::Utf8 | PrismColKind::Text, SolValue::Str(_))
    );
    if !compatible {
        return Err(format!(
            "predicate value type is incompatible with column `{col}` ({kind:?})"
        ));
    }
    Ok(())
}

/// Sugar: `recall(collection, need, modality)` with schema defaults for select/limit.
pub fn recall(
    schema: &CollectionSchema,
    need: &str,
    modality: RecallModality,
) -> Result<PrismQuery, String> {
    validate_name("collection", &schema.name)?;
    validate_query_text("recall need", need)?;
    if need.trim().is_empty() && !matches!(modality, RecallModality::Filter) {
        return Err("recall need must be non-empty".into());
    }
    if schema.default_select.is_empty() {
        return Err("collection default_select must be non-empty for recall sugar".into());
    }
    if schema.default_limit == 0 || schema.default_limit > MAX_QUERY_LIMIT {
        return Err("collection default_limit must be in 1..=10000".into());
    }
    let matches = match modality {
        RecallModality::Fulltext => {
            let on = schema
                .columns
                .iter()
                .find_map(|(n, c)| matches!(c.kind, PrismColKind::Text).then(|| n.clone()))
                .ok_or("collection has no Text column for fulltext recall")?;
            vec![MatchClause::Text {
                on,
                query: need.to_string(),
            }]
        }
        RecallModality::Vector => {
            let on = schema
                .columns
                .iter()
                .find_map(|(n, c)| matches!(c.kind, PrismColKind::Vector { .. }).then(|| n.clone()))
                .ok_or("collection has no Vector column for vector recall")?;
            vec![MatchClause::VectorEmbed {
                on,
                text: need.to_string(),
            }]
        }
        RecallModality::Hybrid => {
            let text_on = schema
                .columns
                .iter()
                .find_map(|(n, c)| matches!(c.kind, PrismColKind::Text).then(|| n.clone()));
            let vec_on = schema.columns.iter().find_map(|(n, c)| {
                matches!(c.kind, PrismColKind::Vector { .. }).then(|| n.clone())
            });
            let (Some(text_on), Some(vec_on)) = (text_on, vec_on) else {
                return Err("hybrid recall needs both Text and Vector columns".into());
            };
            vec![
                MatchClause::Text {
                    on: text_on,
                    query: need.to_string(),
                },
                MatchClause::VectorEmbed {
                    on: vec_on,
                    text: need.to_string(),
                },
                MatchClause::Fusion {
                    method: FusionMethod::Rrf,
                    rrf_k: 60.0,
                },
            ]
        }
        RecallModality::Filter => {
            return Err(
                "filter-only recall needs explicit `where` — build via parse_prism (no bare dump)"
                    .into(),
            );
        }
    };
    let query = PrismQuery {
        from: schema.name.clone(),
        matches,
        where_clauses: Vec::new(),
        select: schema.default_select.clone(),
        limit: schema.default_limit,
        into: None,
    };
    validate_prism(&query)?;
    Ok(query)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecallModality {
    Fulltext,
    Vector,
    Hybrid,
    Filter,
}

/// Serialize Prism back to canonical JSON (for hashing / ledger).
pub fn prism_to_json(q: &PrismQuery) -> J {
    let mut match_arr = Vec::new();
    for m in &q.matches {
        match_arr.push(match m {
            MatchClause::Key { col, value } => serde_json::json!({
                "kind": "key", "on": col, "value": sol_to_json(value)
            }),
            MatchClause::Text { on, query } => serde_json::json!({
                "kind": "text", "on": on, "query": query
            }),
            MatchClause::Vector { on, vector } => serde_json::json!({
                "kind": "vector", "on": on, "vector": vector
            }),
            MatchClause::VectorEmbed { on, text } => serde_json::json!({
                "kind": "vector", "on": on, "embed": text
            }),
            MatchClause::Graph {
                on,
                seeds,
                max_depth,
                max_nodes,
            } => serde_json::json!({
                "kind": "graph", "on": on, "seeds": seeds,
                "max_depth": max_depth, "max_nodes": max_nodes
            }),
            MatchClause::Fusion { method, rrf_k } => serde_json::json!({
                "kind": "fusion",
                "method": match method { FusionMethod::Rrf => "rrf" },
                "rrf_k": rrf_k
            }),
        });
    }
    let where_arr: Vec<J> = q
        .where_clauses
        .iter()
        .map(|w| {
            serde_json::json!({
                "col": w.col,
                "op": match w.op {
                    WhereOp::Eq => "eq",
                    WhereOp::Ne => "ne",
                    WhereOp::Gt => "gt",
                    WhereOp::Ge => "ge",
                    WhereOp::Lt => "lt",
                    WhereOp::Le => "le",
                },
                "value": sol_to_json(&w.value),
            })
        })
        .collect();
    let mut obj = serde_json::Map::new();
    obj.insert("from".into(), J::String(q.from.clone()));
    obj.insert("match".into(), J::Array(match_arr));
    obj.insert("where".into(), J::Array(where_arr));
    obj.insert(
        "select".into(),
        J::Array(q.select.iter().cloned().map(J::String).collect()),
    );
    obj.insert("limit".into(), J::Number(q.limit.into()));
    if let Some(into) = &q.into {
        obj.insert("into".into(), J::String(into.clone()));
    }
    J::Object(obj)
}

fn sol_to_json(v: &SolValue) -> J {
    match v {
        SolValue::Null => J::Null,
        SolValue::Bool(b) => J::Bool(*b),
        SolValue::Int(i) => J::Number((*i).into()),
        SolValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(J::Number)
            .unwrap_or(J::Null),
        SolValue::Str(s) => J::String(s.clone()),
        SolValue::List(xs) => J::Array(xs.iter().map(sol_to_json).collect()),
        SolValue::Map(m) => {
            let mut o = serde_json::Map::new();
            for (k, v) in m {
                o.insert(k.clone(), sol_to_json(v));
            }
            J::Object(o)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompts_schema() -> CollectionSchema {
        let mut columns = BTreeMap::new();
        columns.insert(
            "id".into(),
            PrismColumn {
                kind: PrismColKind::Utf8,
            },
        );
        columns.insert(
            "body".into(),
            PrismColumn {
                kind: PrismColKind::Text,
            },
        );
        columns.insert(
            "embedding".into(),
            PrismColumn {
                kind: PrismColKind::Vector {
                    dim: 3,
                    model: "hash-3".into(),
                },
            },
        );
        columns.insert(
            "category".into(),
            PrismColumn {
                kind: PrismColKind::Utf8,
            },
        );
        columns.insert(
            "active".into(),
            PrismColumn {
                kind: PrismColKind::Bool,
            },
        );
        columns.insert(
            "version".into(),
            PrismColumn {
                kind: PrismColKind::Int,
            },
        );
        CollectionSchema {
            name: "prompts".into(),
            columns,
            default_select: vec!["id".into(), "body".into(), "version".into()],
            default_limit: 10,
        }
    }

    #[test]
    fn parse_rejects_missing_select_and_limit() {
        let j = serde_json::json!({"from": "prompts", "limit": 5});
        assert!(parse_prism(&j).unwrap_err().contains("select"));
        let j = serde_json::json!({"from": "prompts", "select": ["id"]});
        assert!(parse_prism(&j).unwrap_err().contains("limit"));
    }

    #[test]
    fn parse_and_check_vector_embed_envelope() {
        let j = serde_json::json!({
            "from": "prompts",
            "match": {
                "kind": "vector",
                "on": "embedding",
                "embed": "how to reply to a customer message"
            },
            "where": [
                {"col": "category", "op": "eq", "value": "reply"},
                {"col": "active", "op": "eq", "value": true}
            ],
            "select": ["id", "body", "version"],
            "limit": 10,
            "into": "candidates"
        });
        let q = parse_prism(&j).unwrap();
        assert_eq!(q.from, "prompts");
        assert_eq!(q.into.as_deref(), Some("candidates"));
        let mut reg = CollectionRegistry::default();
        reg.declare("demo", prompts_schema());
        check_prism(&reg, "demo", &q).unwrap();
    }

    #[test]
    fn recall_sugar_fulltext_and_vector() {
        let schema = prompts_schema();
        let q = recall(&schema, "to reply to a message", RecallModality::Fulltext).unwrap();
        assert!(matches!(q.matches[0], MatchClause::Text { .. }));
        let q = recall(&schema, "to reply to a message", RecallModality::Vector).unwrap();
        assert!(matches!(q.matches[0], MatchClause::VectorEmbed { .. }));
        let q = recall(&schema, "to reply to a message", RecallModality::Hybrid).unwrap();
        assert_eq!(q.matches.len(), 3);
    }

    #[test]
    fn parse_rejects_bare_table_dump() {
        let j = serde_json::json!({
            "from": "prompts",
            "select": ["id"],
            "limit": 1
        });
        assert!(parse_prism(&j)
            .unwrap_err()
            .contains("at least one `match` or `where`"));
    }

    #[test]
    fn parse_rejects_fusion_without_ranked_match() {
        let j = serde_json::json!({
            "from": "prompts",
            "match": [{"kind": "fusion", "method": "rrf"}],
            "select": ["id"],
            "limit": 1
        });
        assert!(parse_prism(&j).unwrap_err().contains("fusion"));
    }

    #[test]
    fn parse_rejects_duplicate_select() {
        let j = serde_json::json!({
            "from": "prompts",
            "where": [{"col": "active", "op": "eq", "value": true}],
            "select": ["id", "id"],
            "limit": 1
        });
        assert!(parse_prism(&j).unwrap_err().contains("duplicate"));
    }

    #[test]
    fn check_rejects_unknown_select_column() {
        let j = serde_json::json!({
            "from": "prompts",
            "where": [{"col": "active", "op": "eq", "value": true}],
            "select": ["nope"],
            "limit": 1
        });
        let q = parse_prism(&j).unwrap();
        let mut reg = CollectionRegistry::default();
        reg.declare("demo", prompts_schema());
        assert!(check_prism(&reg, "demo", &q).unwrap_err().contains("nope"));
    }

    #[test]
    fn recall_filter_modality_is_rejected() {
        let schema = prompts_schema();
        assert!(recall(&schema, "x", RecallModality::Filter)
            .unwrap_err()
            .contains("filter-only"));
    }
}
