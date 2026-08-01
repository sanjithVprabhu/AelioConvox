//! # aelio-query — dataset registry + closed query AST (§10.4) + Prism
//!
//! "Queries are not strings either." Aelio DB reads are a **Sol-shaped structure, never query text**:
//! one op per database modality (tables / vector / full-text / graph), every op carrying a **mandatory
//! `limit`** that feeds the §8.4 boundedness contract (so I/O can never break the termination
//! theorem, G1).
//!
//! Multimodal recall uses [`prism::PrismQuery`] (from/match/where/select/limit/into). Legacy
//! single-op [`QueryAst`] remains as sugar for one-modality Calls.

pub mod prism;

pub use prism::{
    check_prism, parse_prism, prism_to_json, recall, validate_prism, CollectionRegistry,
    CollectionSchema, FusionMethod, MatchClause, PrismColKind, PrismColumn, PrismQuery,
    RecallModality, WhereOp, WherePred, MAX_PRISM_GRAPH_SEEDS, MAX_PRISM_MATCH_CLAUSES,
    MAX_PRISM_NAME_BYTES, MAX_PRISM_QUERY_TEXT_BYTES, MAX_PRISM_SELECT_COLUMNS,
    MAX_PRISM_WHERE_CLAUSES,
};

use aelio_sol::SolValue;
use serde_json::Value as J;
use std::collections::BTreeMap;

pub const MAX_QUERY_LIMIT: u64 = 10_000;
pub const MAX_TRAVERSE_DEPTH: u64 = 32;
pub const MAX_TRAVERSE_NODES: u64 = 10_000;

/// One op per Aelio database modality (§10.4). No free-text variant exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QOp {
    Get,
    Range,
    TopkVector,
    TopkBm25,
    Traverse,
}

impl QOp {
    fn parse(s: &str) -> Option<QOp> {
        Some(match s {
            "get" => QOp::Get,
            "range" => QOp::Range,
            "topk_vector" => QOp::TopkVector,
            "topk_bm25" => QOp::TopkBm25,
            "traverse" => QOp::Traverse,
            _ => return None,
        })
    }
}

/// A closed, static query. `params` are Sol data (from `args`); there is deliberately no query text.
#[derive(Debug, Clone)]
pub struct QueryAst {
    pub dataset: String,
    pub qop: QOp,
    pub params: SolValue,
    /// Mandatory (§10.4) — feeds the boundedness contract (§8.4).
    pub limit: u64,
    /// `traverse` only, both mandatory (§10.4).
    pub max_depth: Option<u64>,
    pub max_nodes: Option<u64>,
}

/// A tenant-scoped dataset registry (§17.2): every lookup is keyed by tenant first; a missing tenant
/// key is an error, never a fallback to global (§17.2).
#[derive(Default)]
pub struct DatasetRegistry {
    /// (tenant_id, dataset_id) → declared.
    datasets: BTreeMap<(String, String), Dataset>,
}

#[derive(Debug, Clone)]
pub struct Dataset {
    pub id: String,
    /// The modalities this dataset supports — a `topk_vector` on a table-only dataset is rejected.
    pub modalities: Vec<QOp>,
}

impl DatasetRegistry {
    pub fn declare(&mut self, tenant: impl Into<String>, dataset: Dataset) {
        self.datasets
            .insert((tenant.into(), dataset.id.clone()), dataset);
    }

    pub fn get(&self, tenant: &str, dataset: &str) -> Option<&Dataset> {
        self.datasets
            .get(&(tenant.to_string(), dataset.to_string()))
    }
}

/// Parse + validate a query AST (App E `aelio_db.query` shape). Enforces: mandatory positive `limit`;
/// `traverse` requires positive `max_depth` + `max_nodes`; unknown `qop` rejected.
pub fn parse_query(j: &J) -> Result<QueryAst, String> {
    let o = j.as_object().ok_or("query AST must be an object")?;
    let dataset = o
        .get("dataset")
        .and_then(J::as_str)
        .ok_or("query missing `dataset`")?
        .to_string();
    let qop = QOp::parse(
        o.get("qop")
            .and_then(J::as_str)
            .ok_or("query missing `qop`")?,
    )
    .ok_or("unknown `qop` — closed set (§10.4)")?;
    let allowed: &[&str] = if qop == QOp::Traverse {
        &[
            "dataset",
            "qop",
            "params",
            "limit",
            "max_depth",
            "max_nodes",
        ]
    } else {
        &["dataset", "qop", "params", "limit"]
    };
    if let Some(unknown) = o.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!(
            "unknown query field `{unknown}` — closed AST (§10.4)"
        ));
    }
    let params = o
        .get("params")
        .map(sol)
        .transpose()?
        .unwrap_or_else(|| SolValue::map::<_, &str>([]));
    let limit = o
        .get("limit")
        .and_then(J::as_u64)
        .filter(|&n| n > 0 && n <= MAX_QUERY_LIMIT)
        .ok_or("query `limit` must be in 1..=10000 (§10.4 → G1)")?;

    let (max_depth, max_nodes) = if qop == QOp::Traverse {
        let d = o
            .get("max_depth")
            .and_then(J::as_u64)
            .filter(|&n| n > 0 && n <= MAX_TRAVERSE_DEPTH)
            .ok_or("traverse `max_depth` must be in 1..=32 (§10.4)")?;
        let n = o
            .get("max_nodes")
            .and_then(J::as_u64)
            .filter(|&n| n > 0 && n <= MAX_TRAVERSE_NODES)
            .ok_or("traverse `max_nodes` must be in 1..=10000 (§10.4)")?;
        (Some(d), Some(n))
    } else {
        (None, None)
    };

    Ok(QueryAst {
        dataset,
        qop,
        params,
        limit,
        max_depth,
        max_nodes,
    })
}

/// Plan-time check that a parsed query is admissible for a tenant: the dataset is declared for that
/// tenant (§17.2) and supports the requested modality.
pub fn check_admissible(reg: &DatasetRegistry, tenant: &str, q: &QueryAst) -> Result<(), String> {
    let ds = reg.get(tenant, &q.dataset).ok_or_else(|| {
        format!(
            "dataset `{}` not declared for tenant `{tenant}` (§17.2)",
            q.dataset
        )
    })?;
    if !ds.modalities.contains(&q.qop) {
        return Err(format!(
            "dataset `{}` does not support {:?} (§10.4)",
            q.dataset, q.qop
        ));
    }
    Ok(())
}

fn sol(j: &J) -> Result<SolValue, String> {
    let value = to_sol(j)?;
    aelio_sol::Limits::default()
        .check(&value)
        .map_err(|error| format!("query params exceed Sol limits: {error}"))?;
    Ok(value)
}

fn to_sol(j: &J) -> Result<SolValue, String> {
    Ok(match j {
        J::Null => SolValue::Null,
        J::Bool(b) => SolValue::Bool(*b),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                SolValue::Int(i)
            } else if let Some(f) = n.as_f64() {
                SolValue::float(f).map_err(|_| "non-finite".to_string())?
            } else {
                return Err("number out of range".into());
            }
        }
        J::String(s) => SolValue::Str(s.clone()),
        J::Array(a) => SolValue::List(a.iter().map(to_sol).collect::<Result<_, _>>()?),
        J::Object(o) => {
            let mut m = std::collections::BTreeMap::new();
            for (k, v) in o {
                m.insert(k.clone(), to_sol(v)?);
            }
            SolValue::Map(m)
        }
    })
}
