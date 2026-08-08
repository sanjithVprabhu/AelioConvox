//! `ll-server` — an HTTP/JSON network boundary for a single-node [`Database`].
//!
//! Wraps the in-process engine in an `axum` router so it can be reached over the wire: create
//! tables, insert/update/delete rows, run hybrid queries, and trigger flush/compaction — all
//! as JSON. A single writer is serialized behind an `RwLock` (concurrent reads, exclusive
//! writes), matching the single-node engine's model. Authentication is a bearer API key.
//!
//! This is the managed-cloud entry point: the same `Database` that embeds in a Rust process,
//! exposed as a service. (gRPC, per-tenant isolation, and TLS termination are later additions;
//! for TLS today, run behind a reverse proxy.)

mod dto;
mod error;
pub mod embed;
pub mod nl;

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::sync::RwLock;

use ll_query::{
    ColumnKind, Database, TransactionMutation, TransactionMutationResult, TransactionPrecondition,
    Value,
};

use crate::dto::*;
use crate::embed::Embedder;
use crate::error::AppError;
use crate::nl::LlmClient;

/// A boxed, `Send` future — lets the [`LlmClient`](nl::LlmClient) and [`Embedder`](embed::Embedder)
/// traits stay object-safe (`Arc<dyn _>`) without an async-trait macro.
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// The set of accepted API keys, or "open" (no auth) when none are configured.
enum KeySet {
    Open,
    Keys(HashSet<String>),
}

impl KeySet {
    fn accepts(&self, token: &str) -> bool {
        match self {
            KeySet::Open => true,
            KeySet::Keys(s) => s.contains(token),
        }
    }
    fn is_open(&self) -> bool {
        matches!(self, KeySet::Open)
    }
}

/// Shared server state: the database behind a read/write lock, the accepted keys, and an
/// optional LLM backend for natural-language queries.
#[derive(Clone)]
pub struct AppState {
    db: Arc<RwLock<Database>>,
    keys: Arc<KeySet>,
    llm: Option<Arc<dyn LlmClient>>,
    embedder: Option<Arc<dyn Embedder>>,
}

impl AppState {
    /// Build state from a database and a list of API keys. An empty list means **open mode**
    /// (no authentication) — convenient for local development, never for production. Natural
    /// language and embedding are disabled until [`with_llm`](Self::with_llm) /
    /// [`with_embedder`](Self::with_embedder) supply backends.
    pub fn new(db: Database, api_keys: Vec<String>) -> Self {
        let keys = if api_keys.is_empty() {
            KeySet::Open
        } else {
            KeySet::Keys(api_keys.into_iter().collect())
        };
        AppState {
            db: Arc::new(RwLock::new(db)),
            keys: Arc::new(keys),
            llm: None,
            embedder: None,
        }
    }

    /// Attach an LLM backend, enabling the `/nl` endpoint.
    pub fn with_llm(mut self, client: Arc<dyn LlmClient>) -> Self {
        self.llm = Some(client);
        self
    }

    /// Attach an embedding backend, enabling the `semantic` query clause and the
    /// `{"type":"embed",...}` row value.
    pub fn with_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    /// Whether the server is running without authentication.
    pub fn is_open(&self) -> bool {
        self.keys.is_open()
    }

    /// Embed a single text, or 503 if no embedder is configured.
    async fn embed_one(&self, text: String) -> Result<Vec<f32>, AppError> {
        let mut v = self.embed_many(vec![text]).await?;
        v.pop().ok_or_else(|| AppError::Internal("embedder returned no vector".into()))
    }

    /// Embed a batch of texts, or 503 if no embedder is configured.
    async fn embed_many(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, AppError> {
        let embedder = self
            .embedder
            .as_ref()
            .ok_or_else(|| AppError::Unavailable("embedding is not configured (no embedder backend)".into()))?;
        embedder.embed(texts).await.map_err(AppError::Internal)
    }
}

/// Build the full router: an unauthenticated `/v1/health` plus the API surface behind the
/// bearer-key middleware.
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/v1/tables", post(create_table))
        .route("/v1/tables/{table}/columns", post(add_columns))
        .route("/v1/transactions", post(transact))
        .route("/v1/tables/{table}/rows", post(insert_row))
        .route(
            "/v1/tables/{table}/rows/{id}",
            get(get_row)
                .patch(update_row)
                .delete(delete_row),
        )
        .route("/v1/tables/{table}/scan", post(scan_rows))
        .route("/v1/tables/{table}/query", post(query))
        .route("/v1/tables/{table}/explain", post(explain))
        .route("/v1/tables/{table}/nl", post(nl_query))
        .route("/v1/tables/{table}/schema", get(schema))
        .route("/v1/admin/flush", post(flush))
        .route("/v1/admin/compact", post(compact))
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state);

    Router::new().route("/v1/health", get(health)).merge(api)
}

/// Bearer-token gate. In open mode every request passes; otherwise the `Authorization: Bearer
/// <key>` header must carry an accepted key.
async fn auth(State(state): State<AppState>, req: Request, next: Next) -> Result<Response, AppError> {
    if state.keys.is_open() {
        return Ok(next.run(req).await);
    }
    let presented = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    match presented {
        Some(token) if state.keys.accepts(token) => Ok(next.run(req).await),
        _ => Err(AppError::Unauthorized),
    }
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "service": "ll-server", "version": env!("CARGO_PKG_VERSION") }))
}

async fn create_table(
    State(state): State<AppState>,
    Json(body): Json<CreateTableRequest>,
) -> Result<Json<CreateTableResponse>, AppError> {
    // Resolve kinds first (owned names + kinds), then borrow for the catalog call.
    let mut named: Vec<(String, ColumnKind)> = Vec::with_capacity(body.columns.len());
    for c in &body.columns {
        named.push((c.name.clone(), c.to_kind().map_err(AppError::BadRequest)?));
    }
    let cols: Vec<(&str, ColumnKind)> = named.iter().map(|(n, k)| (n.as_str(), *k)).collect();
    let mut db = state.db.write().await;
    let table_id = db.create_table(&body.name, &cols)?;
    Ok(Json(CreateTableResponse { table_id }))
}

async fn add_columns(
    State(state): State<AppState>,
    Path(table): Path<String>,
    Json(body): Json<AddColumnsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut named: Vec<(String, ColumnKind)> = Vec::with_capacity(body.columns.len());
    for column in &body.columns {
        named.push((column.name.clone(), column.to_kind().map_err(AppError::BadRequest)?));
    }
    if named.is_empty() {
        return Err(AppError::BadRequest("at least one column is required".into()));
    }
    let columns: Vec<(&str, ColumnKind)> = named.iter().map(|(name, kind)| (name.as_str(), *kind)).collect();
    let mut db = state.db.write().await;
    db.add_columns(&table, &columns)?;
    Ok(Json(serde_json::json!({ "applied": true })))
}

async fn insert_row(
    State(state): State<AppState>,
    Path(table): Path<String>,
    Json(mut body): Json<RowRequest>,
) -> Result<Json<InsertResponse>, AppError> {
    resolve_row_embeds(&state, &table, &mut body.values).await?;
    let pairs = to_value_pairs(body);
    let vals: Vec<(&str, Value)> = pairs.iter().map(|(n, v)| (n.as_str(), v.clone())).collect();
    let mut db = state.db.write().await;
    let row_id = db.insert(&table, &vals)?;
    Ok(Json(InsertResponse { row_id }))
}

/// Resolve any input embeddings, prepare every mutation, then submit the entire group through
/// Aelio DB's one-WAL-transaction boundary. We intentionally do all network work before taking
/// the database write lock, so slow embedding calls cannot stall readers or another writer.
async fn transact(
    State(state): State<AppState>,
    Json(body): Json<TransactionRequest>,
) -> Result<Json<TransactionResponse>, AppError> {
    let mut preconditions = Vec::with_capacity(body.preconditions.len());
    for condition in body.preconditions {
        match condition {
            TransactionPreconditionRequest::Absent { table, equals } => {
                preconditions.push(TransactionPrecondition::Absent {
                    table,
                    equals: condition_values(equals)?,
                });
            }
            TransactionPreconditionRequest::RowMatches { table, row_id, equals } => {
                preconditions.push(TransactionPrecondition::RowMatches {
                    table,
                    row_id,
                    equals: condition_values(equals)?,
                });
            }
        }
    }

    let mut mutations = Vec::with_capacity(body.mutations.len());
    for request in body.mutations {
        match request {
            TransactionMutationRequest::Insert { table, mut values } => {
                resolve_row_embeds(&state, &table, &mut values).await?;
                mutations.push(TransactionMutation::Insert {
                    table,
                    values: values.into_iter().map(|(name, value)| (name, value.into())).collect(),
                });
            }
            TransactionMutationRequest::Update { table, row_id, mut values } => {
                resolve_row_embeds(&state, &table, &mut values).await?;
                mutations.push(TransactionMutation::Update {
                    table,
                    row_id,
                    values: values.into_iter().map(|(name, value)| (name, value.into())).collect(),
                });
            }
            TransactionMutationRequest::Delete { table, row_id } => {
                mutations.push(TransactionMutation::Delete { table, row_id });
            }
        }
    }

    let conditional = !preconditions.is_empty();
    let result = if conditional {
        state.db.write().await.transact_conditional(preconditions, mutations)?
    } else {
        let transaction = state.db.write().await.transact(mutations)?;
        ll_query::ConditionalTransactionResult { applied: true, transaction: Some(transaction) }
    };
    let Some(transaction) = result.transaction else {
        return Ok(Json(TransactionResponse { applied: false, commit_lsn: None, results: Vec::new() }));
    };
    let results = transaction
        .results
        .into_iter()
        .map(|result| match result {
            TransactionMutationResult::Inserted { row_id } => TransactionMutationResponse { op: "insert", row_id },
            TransactionMutationResult::Updated { row_id } => TransactionMutationResponse { op: "update", row_id },
            TransactionMutationResult::Deleted { row_id } => TransactionMutationResponse { op: "delete", row_id },
        })
        .collect();
    Ok(Json(TransactionResponse { applied: true, commit_lsn: Some(transaction.commit_lsn), results }))
}

async fn update_row(
    State(state): State<AppState>,
    Path((table, id)): Path<(String, u64)>,
    Json(mut body): Json<RowRequest>,
) -> Result<Json<MutateResponse>, AppError> {
    resolve_row_embeds(&state, &table, &mut body.values).await?;
    let pairs = to_value_pairs(body);
    let vals: Vec<(&str, Value)> = pairs.iter().map(|(n, v)| (n.as_str(), v.clone())).collect();
    let mut db = state.db.write().await;
    let applied = db.update(&table, id, &vals)?;
    Ok(Json(MutateResponse { applied }))
}

/// The declared dimension of vector column `name`, or `None` if it isn't a vector column.
fn vector_dim(cols: &[(String, ColumnKind)], name: &str) -> Option<u16> {
    cols.iter().find(|(n, _)| n == name).and_then(|(_, k)| match k {
        ColumnKind::Vector(d) => Some(*d),
        _ => None,
    })
}

/// Snapshot a table's columns (drops the read lock before any network/embed await).
async fn table_columns(state: &AppState, table: &str) -> Result<Vec<(String, ColumnKind)>, AppError> {
    let db = state.db.read().await;
    db.columns(table).ok_or_else(|| AppError::NotFound(format!("no such table: {table}")))
}

/// Pre-embed check: the target must be a vector column (so we don't spend a paid embed call on
/// an `embed`/`semantic` request aimed at a non-vector column). Returns its declared dimension.
fn require_vector_col(cols: &[(String, ColumnKind)], col: &str) -> Result<u16, AppError> {
    vector_dim(cols, col).ok_or_else(|| {
        AppError::BadRequest(format!(
            "`{col}` is not a vector column; `embed`/`semantic` only apply to vector columns"
        ))
    })
}

/// Post-embed dimension guard: an embedded vector's length must match its target column's
/// declared dimension, or the vector index would silently ignore it (wrong-length vectors are
/// dropped), making semantic search mysteriously return nothing. We turn that silent failure
/// into a clear 400.
fn check_dim(col: &str, want: u16, got: usize) -> Result<(), AppError> {
    if want as usize != got {
        return Err(AppError::BadRequest(format!(
            "embedder produced {got}-dimensional vectors but column `{col}` is {want}-dimensional — \
             set LL_EMBED_MODEL to a model whose output dimension is {want}"
        )));
    }
    Ok(())
}

/// Replace any `{"type":"embed","value":"text"}` row values with the embedded vector, in one
/// batched embedder call. No-op if the row has no embed values. Validates each embedded
/// vector's dimension against its target column.
async fn resolve_row_embeds(
    state: &AppState,
    table: &str,
    values: &mut std::collections::HashMap<String, ApiValue>,
) -> Result<(), AppError> {
    let mut cols: Vec<String> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    for (name, v) in values.iter() {
        if let ApiValue::Embed(text) = v {
            cols.push(name.clone());
            texts.push(text.clone());
        }
    }
    if texts.is_empty() {
        return Ok(());
    }
    let schema = table_columns(state, table).await?;
    // Validate every target is a vector column *before* paying for the embed batch.
    let wants: Vec<u16> = cols.iter().map(|c| require_vector_col(&schema, c)).collect::<Result<_, _>>()?;
    let vectors = state.embed_many(texts).await?;
    for ((col, want), vec) in cols.into_iter().zip(wants).zip(vectors) {
        check_dim(&col, want, vec.len())?;
        values.insert(col, ApiValue::Vector(vec));
    }
    Ok(())
}

/// If the query carries a `semantic` clause, embed its text into the `vector` clause. Errors
/// if both are set (ambiguous), no embedder is configured, or the embedding's dimension does
/// not match the target column.
async fn resolve_semantic(state: &AppState, table: &str, req: &mut QueryRequest) -> Result<(), AppError> {
    let Some(sem) = req.semantic.take() else {
        return Ok(());
    };
    if req.vector.is_some() {
        return Err(AppError::BadRequest("provide either `vector` or `semantic`, not both".into()));
    }
    let schema = table_columns(state, table).await?;
    let want = require_vector_col(&schema, &sem.col)?; // reject before paying for the embed
    let query = state.embed_one(sem.text).await?;
    check_dim(&sem.col, want, query.len())?;
    req.vector = Some(VectorClause { col: sem.col, query });
    Ok(())
}

async fn delete_row(
    State(state): State<AppState>,
    Path((table, id)): Path<(String, u64)>,
) -> Result<Json<MutateResponse>, AppError> {
    let mut db = state.db.write().await;
    let applied = db.delete(&table, id)?;
    Ok(Json(MutateResponse { applied }))
}

async fn get_row(
    State(state): State<AppState>,
    Path((table, id)): Path<(String, u64)>,
) -> Result<Json<RowValueResponse>, AppError> {
    let db = state.db.read().await;
    let vals = db
        .get_row_values(&table, id)?
        .ok_or_else(|| AppError::NotFound(format!("no such row: {id}")))?;
    let mut values = std::collections::HashMap::new();
    for (name, v) in vals {
        values.insert(name, v.into());
    }
    Ok(Json(RowValueResponse { row_id: id, values }))
}

async fn scan_rows(
    State(state): State<AppState>,
    Path(table): Path<String>,
    Json(body): Json<QueryRequest>,
) -> Result<Json<ScanResponse>, AppError> {
    let q = body.to_hybrid().map_err(AppError::BadRequest)?;
    let db = state.db.read().await;
    let hits = db.scan_values(&table, &q)?;
    let rows = hits
        .into_iter()
        .map(|(row_id, vals)| {
            let mut values = std::collections::HashMap::new();
            for (name, v) in vals {
                values.insert(name, v.into());
            }
            RowValueResponse { row_id, values }
        })
        .collect();
    Ok(Json(ScanResponse { rows }))
}

async fn query(
    State(state): State<AppState>,
    Path(table): Path<String>,
    Json(mut body): Json<QueryRequest>,
) -> Result<Json<QueryResponse>, AppError> {
    resolve_semantic(&state, &table, &mut body).await?;
    let q = body.to_hybrid().map_err(AppError::BadRequest)?;
    let db = state.db.read().await;
    let results = db
        .query(&table, &q)?
        .into_iter()
        .map(|(row_id, score)| Hit { row_id, score })
        .collect();
    Ok(Json(QueryResponse { results }))
}

async fn explain(
    State(state): State<AppState>,
    Path(table): Path<String>,
    Json(mut body): Json<QueryRequest>,
) -> Result<Json<ExplainResponse>, AppError> {
    resolve_semantic(&state, &table, &mut body).await?;
    let q = body.to_hybrid().map_err(AppError::BadRequest)?;
    let db = state.db.read().await;
    let plan = db.explain(&table, &q)?;
    Ok(Json(ExplainResponse { plan }))
}

async fn nl_query(
    State(state): State<AppState>,
    Path(table): Path<String>,
    Json(body): Json<NlRequest>,
) -> Result<Json<NlResponse>, AppError> {
    let llm = state
        .llm
        .clone()
        .ok_or_else(|| AppError::Unavailable("natural-language queries are not configured (no LLM backend)".into()))?;

    // Ground the model in the table's real schema.
    let cols = {
        let db = state.db.read().await;
        db.columns(&table).ok_or_else(|| AppError::NotFound(format!("no such table: {table}")))?
    };
    let semantic_enabled = state.embedder.is_some();
    let system = nl::system_prompt(&table, &cols, semantic_enabled);
    let schema = nl::query_tool_schema(semantic_enabled);

    // Compile NL → structured query (the model must call the tool; we get its input back).
    let compiled = llm
        .translate(system, body.query, schema)
        .await
        .map_err(AppError::Internal)?;
    let mut req: QueryRequest = serde_json::from_value(compiled.clone())
        .map_err(|e| AppError::Internal(format!("model produced an invalid query: {e}")))?;
    resolve_semantic(&state, &table, &mut req).await?;
    let q = req.to_hybrid().map_err(AppError::BadRequest)?;

    let db = state.db.read().await;
    let results = db
        .query(&table, &q)?
        .into_iter()
        .map(|(row_id, score)| Hit { row_id, score })
        .collect();
    Ok(Json(NlResponse { compiled, results }))
}

async fn schema(
    State(state): State<AppState>,
    Path(table): Path<String>,
) -> Result<Json<SchemaResponse>, AppError> {
    let db = state.db.read().await;
    let cols = db.columns(&table).ok_or_else(|| AppError::NotFound(format!("no such table: {table}")))?;
    let columns = cols
        .into_iter()
        .map(|(name, kind)| {
            let (label, dim) = nl::kind_label(kind);
            ColumnInfo { name, kind: label.to_string(), dim }
        })
        .collect();
    Ok(Json(SchemaResponse { table, columns }))
}

async fn flush(State(state): State<AppState>) -> Result<Json<serde_json::Value>, AppError> {
    state.db.write().await.flush()?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn compact(State(state): State<AppState>) -> Result<Json<serde_json::Value>, AppError> {
    state.db.write().await.compact()?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Owned `(name, Value)` pairs from a row request, so handlers can then borrow names for the
/// engine's `&[(&str, Value)]` signature.
fn to_value_pairs(body: RowRequest) -> Vec<(String, Value)> {
    body.values.into_iter().map(|(name, v)| (name, v.into())).collect()
}

/// `embed` would perform a network call, which is intentionally not valid for a conditional
/// predicate. Conditions must be exact, already-materialised values so their outcome is stable
/// and auditable.
fn condition_values(values: std::collections::HashMap<String, ApiValue>) -> Result<Vec<(String, Value)>, AppError> {
    values
        .into_iter()
        .map(|(name, value)| match value {
            ApiValue::Embed(_) => Err(AppError::BadRequest("transaction preconditions cannot use `embed` values".into())),
            other => Ok((name, other.into())),
        })
        .collect()
}
