//! Lower Prism envelopes onto [`HybridQuery`] and project mandatory `select` columns.

use std::collections::BTreeMap;
use std::io;

use aelio_db_engine::Value;
use aelio_query::{validate_prism, MatchClause, PrismQuery, WhereOp, WherePred};
use aelio_sol::SolValue;

use crate::database::{Database, DatabaseQueryError, HybridQuery, NamedRow};
use crate::exec::{GraphBudget, PredOp};
use crate::ColumnKind;

/// Embed `text` under a pinned model id (from collection Vector column metadata).
pub trait PrismEmbedder {
    fn embed(&self, model: &str, text: &str) -> Result<Vec<f32>, String>;
}

/// Deterministic tiny embedder for tests — not for production similarity quality.
pub struct HashEmbedder {
    pub dim: usize,
}

impl PrismEmbedder for HashEmbedder {
    fn embed(&self, _model: &str, text: &str) -> Result<Vec<f32>, String> {
        let mut out = vec![0.0f32; self.dim.max(1)];
        let len = out.len();
        for (i, b) in text.bytes().enumerate() {
            out[i % len] += (b as f32) / 255.0;
        }
        let norm = out.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
        for x in &mut out {
            *x /= norm;
        }
        Ok(out)
    }
}

#[derive(Debug, Clone)]
pub struct PrismHit {
    pub row_id: u64,
    pub score: f32,
    /// Only columns listed in Prism `select`.
    pub fields: BTreeMap<String, Value>,
}

#[derive(Debug)]
pub enum PrismExecError {
    Plan(String),
    Embed(String),
    Io(io::Error),
    /// Graph/traversal budget exceeded — must not collapse to empty hits.
    Budget(String),
}

impl std::fmt::Display for PrismExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plan(s) => write!(f, "prism plan: {s}"),
            Self::Embed(s) => write!(f, "prism embed: {s}"),
            Self::Io(e) => write!(f, "prism io: {e}"),
            Self::Budget(s) => write!(f, "prism budget: {s}"),
        }
    }
}

impl std::error::Error for PrismExecError {}

fn map_db_query_err(e: DatabaseQueryError) -> PrismExecError {
    match e {
        DatabaseQueryError::Planning(io) => PrismExecError::Io(io),
        DatabaseQueryError::Execution(err) => PrismExecError::Budget(err.to_string()),
    }
}

/// Resolve embeds, lower to [`HybridQuery`], execute once, project `select`.
///
/// Uses a single ranked query pass then reconstructs only the hit rows (no second plan).
/// Graph budget failures surface as [`PrismExecError::Budget`] (never silent empty).
pub fn execute_prism(
    db: &Database,
    query: &PrismQuery,
    model_for_vector_col: &BTreeMap<String, String>,
    embedder: Option<&dyn PrismEmbedder>,
) -> Result<Vec<PrismHit>, PrismExecError> {
    // Validate against the physical catalog before paying for an embedding call. This also
    // makes projection fail closed instead of silently omitting misspelled columns.
    validate_database_query(db, query)?;
    let hybrid = lower_prism(query, model_for_vector_col, embedder)?;
    validate_vector_dimension(db, query, &hybrid)?;
    let scored = db
        .query_checked(&query.from, &hybrid)
        .map_err(map_db_query_err)?;
    // Cap again defensively — planner/limit must already constrain, but never return more.
    let scored = scored.into_iter().take(query.limit as usize);
    let mut hits = Vec::new();
    for (row_id, score) in scored {
        let Some(row) = db
            .get_row_values(&query.from, row_id)
            .map_err(PrismExecError::Io)?
        else {
            continue;
        };
        hits.push(PrismHit {
            row_id,
            score,
            fields: project_row(&row, &query.select),
        });
    }
    Ok(hits)
}

/// Lower Prism → name-based [`HybridQuery`] (no DB I/O).
pub fn lower_prism(
    query: &PrismQuery,
    model_for_vector_col: &BTreeMap<String, String>,
    embedder: Option<&dyn PrismEmbedder>,
) -> Result<HybridQuery, PrismExecError> {
    // Re-check envelope so programmatic PrismQuery construction cannot bypass parse bounds.
    validate_prism(query).map_err(PrismExecError::Plan)?;

    let mut hybrid = HybridQuery::new(query.limit as usize);
    let mut filters = Vec::new();

    for pred in &query.where_clauses {
        filters.push(where_to_filter(pred)?);
    }

    for m in &query.matches {
        match m {
            MatchClause::Key { col, value } => {
                filters.push((
                    col.clone(),
                    PredOp::Eq,
                    sol_to_engine_value(value).map_err(PrismExecError::Plan)?,
                ));
            }
            MatchClause::Text { on, query } => {
                if hybrid.text.is_some() {
                    return Err(PrismExecError::Plan(
                        "Prism v0 allows at most one text match per query".into(),
                    ));
                }
                hybrid = hybrid.text(on, query);
            }
            MatchClause::Vector { on, vector } => {
                if hybrid.vector.is_some() {
                    return Err(PrismExecError::Plan(
                        "Prism v0 allows at most one vector match per query".into(),
                    ));
                }
                hybrid = hybrid.vector(on, vector.clone());
            }
            MatchClause::VectorEmbed { on, text } => {
                if hybrid.vector.is_some() {
                    return Err(PrismExecError::Plan(
                        "Prism v0 allows at most one vector match per query".into(),
                    ));
                }
                let embedder = embedder.ok_or_else(|| {
                    PrismExecError::Embed("vector embed match requires an embedder".into())
                })?;
                let model = model_for_vector_col.get(on).ok_or_else(|| {
                    PrismExecError::Embed(format!(
                        "no embedding model declared for vector column `{on}`"
                    ))
                })?;
                let vector = embedder.embed(model, text).map_err(PrismExecError::Embed)?;
                if vector.iter().any(|x| !x.is_finite()) {
                    return Err(PrismExecError::Embed(
                        "embedder returned non-finite vector".into(),
                    ));
                }
                hybrid = hybrid.vector(on, vector);
            }
            MatchClause::Graph {
                on,
                seeds,
                max_depth,
                max_nodes,
            } => {
                if hybrid.graph.is_some() {
                    return Err(PrismExecError::Plan(
                        "Prism v0 allows at most one graph match per query".into(),
                    ));
                }
                // Wire Prism caps into engine budget — otherwise depth>8 / max_nodes are ignored
                // and `execute` would silently return empty on budget trip.
                let depth = (*max_depth as usize).max(1);
                let nodes = (*max_nodes as usize).max(1);
                let budget = GraphBudget {
                    max_seeds: seeds.len().max(1),
                    max_depth: depth,
                    max_frontier: nodes,
                    max_visited: nodes,
                    ..GraphBudget::default()
                };
                hybrid = hybrid.graph(on, seeds.clone(), depth).graph_budget(budget);
            }
            MatchClause::Fusion { method: _, rrf_k } => {
                hybrid = hybrid.rrf_k(*rrf_k);
            }
        }
    }

    if hybrid.vector.is_none()
        && hybrid.text.is_none()
        && hybrid.graph.is_none()
        && filters.is_empty()
    {
        return Err(PrismExecError::Plan(
            "lowered plan has no match and no filters".into(),
        ));
    }

    for (col, op, value) in filters {
        hybrid = hybrid.filter(&col, op, value);
    }
    Ok(hybrid)
}

fn where_to_filter(pred: &WherePred) -> Result<(String, PredOp, Value), PrismExecError> {
    let op = match pred.op {
        WhereOp::Eq => PredOp::Eq,
        WhereOp::Ne => PredOp::Ne,
        WhereOp::Gt => PredOp::Gt,
        WhereOp::Ge => PredOp::Ge,
        WhereOp::Lt => PredOp::Lt,
        WhereOp::Le => PredOp::Le,
    };
    Ok((
        pred.col.clone(),
        op,
        sol_to_engine_value(&pred.value).map_err(PrismExecError::Plan)?,
    ))
}

fn sol_to_engine_value(v: &SolValue) -> Result<Value, String> {
    Ok(match v {
        SolValue::Null => Value::Null,
        SolValue::Bool(b) => Value::Bool(*b),
        SolValue::Int(i) => Value::I64(*i),
        SolValue::Float(f) => Value::F64(*f),
        SolValue::Str(s) => Value::Utf8(s.clone()),
        SolValue::List(_) => return Err("where/key values may not be lists".into()),
        SolValue::Map(_) => return Err("where values may not be maps".into()),
    })
}

fn validate_database_query(db: &Database, query: &PrismQuery) -> Result<(), PrismExecError> {
    let columns = db.columns(&query.from).ok_or_else(|| {
        PrismExecError::Plan(format!("collection `{}` does not exist", query.from))
    })?;
    let columns: BTreeMap<_, _> = columns.into_iter().collect();
    for selected in &query.select {
        if !columns.contains_key(selected) {
            return Err(PrismExecError::Plan(format!(
                "select column `{selected}` does not exist in collection `{}`",
                query.from
            )));
        }
    }
    for pred in &query.where_clauses {
        let kind = columns.get(&pred.col).ok_or_else(|| {
            PrismExecError::Plan(format!(
                "where column `{}` does not exist in collection `{}`",
                pred.col, query.from
            ))
        })?;
        validate_filter_type(&pred.col, *kind, pred.op, &pred.value)?;
    }
    for clause in &query.matches {
        let (column, expected) = match clause {
            MatchClause::Key { col, value } => {
                let kind = columns
                    .get(col)
                    .ok_or_else(|| missing_match_col(query, col))?;
                validate_filter_type(col, *kind, WhereOp::Eq, value)?;
                continue;
            }
            MatchClause::Text { on, .. } => (on, "text"),
            MatchClause::Vector { on, .. } | MatchClause::VectorEmbed { on, .. } => (on, "vector"),
            MatchClause::Graph { on, .. } => (on, "edge"),
            MatchClause::Fusion { .. } => continue,
        };
        let kind = columns
            .get(column)
            .ok_or_else(|| missing_match_col(query, column))?;
        let compatible = matches!(
            (expected, kind),
            ("text", ColumnKind::Text)
                | ("vector", ColumnKind::Vector(_))
                | ("edge", ColumnKind::Edge)
        );
        if !compatible {
            return Err(PrismExecError::Plan(format!(
                "{expected} match is incompatible with column `{column}` ({kind:?})"
            )));
        }
    }
    Ok(())
}

fn missing_match_col(query: &PrismQuery, column: &str) -> PrismExecError {
    PrismExecError::Plan(format!(
        "match column `{column}` does not exist in collection `{}`",
        query.from
    ))
}

fn validate_filter_type(
    column: &str,
    kind: ColumnKind,
    op: WhereOp,
    value: &SolValue,
) -> Result<(), PrismExecError> {
    if matches!(kind, ColumnKind::Vector(_) | ColumnKind::Edge) {
        return Err(PrismExecError::Plan(format!(
            "column `{column}` cannot be used as a scalar predicate"
        )));
    }
    if matches!(value, SolValue::Null) {
        return if matches!(op, WhereOp::Eq | WhereOp::Ne) {
            Ok(())
        } else {
            Err(PrismExecError::Plan(format!(
                "ordered comparison on `{column}` may not use null"
            )))
        };
    }
    let compatible = matches!(
        (kind, value),
        (ColumnKind::Bool, SolValue::Bool(_))
            | (ColumnKind::I64 | ColumnKind::Timestamp, SolValue::Int(_))
            | (ColumnKind::F64, SolValue::Float(_))
            | (ColumnKind::Utf8 | ColumnKind::Text, SolValue::Str(_))
    );
    if !compatible {
        return Err(PrismExecError::Plan(format!(
            "predicate value type is incompatible with column `{column}` ({kind:?})"
        )));
    }
    Ok(())
}

fn validate_vector_dimension(
    db: &Database,
    query: &PrismQuery,
    hybrid: &HybridQuery,
) -> Result<(), PrismExecError> {
    let Some((column, vector)) = &hybrid.vector else {
        return Ok(());
    };
    let expected = db
        .columns(&query.from)
        .and_then(|columns| {
            columns.into_iter().find_map(|(name, kind)| {
                if name != *column {
                    return None;
                }
                match kind {
                    ColumnKind::Vector(dim) => Some(dim as usize),
                    _ => None,
                }
            })
        })
        .ok_or_else(|| {
            PrismExecError::Plan(format!("`{column}` is not a declared vector column"))
        })?;
    if vector.len() != expected {
        return Err(PrismExecError::Plan(format!(
            "vector length {} != declared dimension {expected} on `{column}`",
            vector.len()
        )));
    }
    if vector.iter().any(|value| !value.is_finite()) {
        return Err(PrismExecError::Plan(
            "query vector contains a non-finite value".into(),
        ));
    }
    Ok(())
}

/// Project a full named row down to Prism `select` (helper for callers using scan).
pub fn project_row(row: &NamedRow, select: &[String]) -> BTreeMap<String, Value> {
    let mut fields = BTreeMap::new();
    for col in select {
        if let Some((_, value)) = row.iter().find(|(name, _)| name == col) {
            fields.insert(col.clone(), value.clone());
        }
    }
    fields
}
