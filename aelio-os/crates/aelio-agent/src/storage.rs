//! Durable, tenant-scoped Aelio repositories over the embedded Aelio DB database.
//!
//! Every repository operation requires a tenant and a closed logical table. Callers never
//! provide a physical table name, which prevents accidental cross-tenant queries. Tables are
//! logical isolation boundaries over Aelio DB's shared WAL and segments; authorization still
//! belongs to the Aelio server boundary.

use std::io;
use std::sync::{Arc, Mutex, MutexGuard};

use aelio_db_query::{
    ColumnKind, Database, HybridQuery, InsertIfAbsent, PredOp, UpdateIfVersion, Value as DbValue,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::types::{AelioError, AelioResult, ReasonCode};

const SCHEMA_VERSION: i64 = 5;

/// Closed tenant-scoped storage namespaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogicalTable {
    Turns,
    StepAttempts,
    Idempotency,
    Jobs,
    EffectIntents,
    CallRecords,
    OutcomeSignals,
    Approvals,
    Catalogs,
    States,
    FlowInstances,
    Procedures,
    Proposals,
    Signatures,
    Documents,
    DocumentChunks,
    Memories,
    ProactiveRuns,
    Explorations,
    FlowCandidates,
    SdkDeliveryEvents,
    Audit,
    Migrations,
}

impl LogicalTable {
    fn name(self) -> &'static str {
        match self {
            Self::Turns => "turns",
            Self::StepAttempts => "step_attempts",
            Self::Idempotency => "idempotency",
            Self::Jobs => "jobs",
            Self::EffectIntents => "effect_intents",
            Self::CallRecords => "call_records",
            Self::OutcomeSignals => "outcome_signals",
            Self::Approvals => "approvals",
            Self::Catalogs => "catalogs",
            Self::States => "states",
            Self::FlowInstances => "flow_instances",
            Self::Procedures => "procedures",
            Self::Proposals => "proposals",
            Self::Signatures => "signatures",
            Self::Documents => "documents",
            Self::DocumentChunks => "document_chunks",
            Self::Memories => "memories",
            Self::ProactiveRuns => "proactive_runs",
            Self::Explorations => "explorations",
            Self::FlowCandidates => "flow_candidates",
            Self::SdkDeliveryEvents => "sdk_delivery_events",
            Self::Audit => "audit",
            Self::Migrations => "migrations",
        }
    }

    fn has_embedding(self) -> bool {
        matches!(
            self,
            Self::Procedures | Self::Documents | Self::DocumentChunks | Self::Memories
        )
    }

    const ALL: [Self; 23] = [
        Self::Turns,
        Self::StepAttempts,
        Self::Idempotency,
        Self::Jobs,
        Self::EffectIntents,
        Self::CallRecords,
        Self::OutcomeSignals,
        Self::Approvals,
        Self::Catalogs,
        Self::States,
        Self::FlowInstances,
        Self::Procedures,
        Self::Proposals,
        Self::Signatures,
        Self::Documents,
        Self::DocumentChunks,
        Self::Memories,
        Self::ProactiveRuns,
        Self::Explorations,
        Self::FlowCandidates,
        Self::SdkDeliveryEvents,
        Self::Audit,
        Self::Migrations,
    ];
}

/// Resolves a tenant and closed logical name to a physical Aelio DB table.
#[derive(Debug, Clone)]
pub struct TableResolver {
    prefix: String,
}

impl Default for TableResolver {
    fn default() -> Self {
        Self {
            prefix: "aelio".into(),
        }
    }
}

impl TableResolver {
    pub fn new(prefix: impl Into<String>) -> AelioResult<Self> {
        let prefix = prefix.into();
        if prefix.is_empty()
            || !prefix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "table prefix must contain only ASCII letters, numbers, or underscores",
            ));
        }
        Ok(Self { prefix })
    }

    pub fn resolve(&self, tenant_id: &str, logical: LogicalTable) -> AelioResult<String> {
        if tenant_id.trim().is_empty() {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "tenant id is required",
            ));
        }
        let digest = Sha256::digest(tenant_id.as_bytes());
        let tenant_key = hex::encode(&digest[..8]);
        Ok(format!(
            "{}_t_{}_{}",
            self.prefix,
            tenant_key,
            logical.name()
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordEnvelope<T> {
    pub key: String,
    pub kind: String,
    pub status: String,
    pub owner: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub expires_at_ms: Option<i64>,
    pub value: T,
}

#[derive(Debug, Clone)]
pub struct StoredRecord<T> {
    pub row_id: u64,
    pub version: u64,
    pub envelope: RecordEnvelope<T>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PutIfAbsent {
    Inserted { row_id: u64, version: u64 },
    Existing { row_id: u64, version: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareSwap {
    Updated { version: u64 },
    Conflict { current_version: u64 },
    NotFound,
}

#[derive(Debug, Clone, Copy)]
pub struct RecallFilter<'a> {
    pub status: Option<&'a str>,
    pub owner: Option<&'a str>,
    pub now_ms: i64,
    pub k: usize,
}

/// Embedded durable storage used by the Rust runtime.
#[derive(Clone)]
pub struct AelioStore {
    db: Arc<Mutex<Database>>,
    resolver: TableResolver,
    embedding_dim: u16,
}

impl AelioStore {
    pub fn new(db: Database, embedding_dim: u16) -> AelioResult<Self> {
        if embedding_dim == 0 {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "embedding dimension must be greater than zero",
            ));
        }
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            resolver: TableResolver::default(),
            embedding_dim,
        })
    }

    pub fn with_resolver(
        db: Database,
        embedding_dim: u16,
        resolver: TableResolver,
    ) -> AelioResult<Self> {
        let mut store = Self::new(db, embedding_dim)?;
        store.resolver = resolver;
        Ok(store)
    }

    pub fn from_shared(
        db: Arc<Mutex<Database>>,
        embedding_dim: u16,
        resolver: TableResolver,
    ) -> AelioResult<Self> {
        if embedding_dim == 0 {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "embedding dimension must be greater than zero",
            ));
        }
        Ok(Self {
            db,
            resolver,
            embedding_dim,
        })
    }

    pub fn shared_database(&self) -> Arc<Mutex<Database>> {
        Arc::clone(&self.db)
    }

    pub fn embedding_dimension(&self) -> usize {
        usize::from(self.embedding_dim)
    }

    /// Create every tenant table and atomically record the schema version.
    pub fn migrate_tenant(&mut self, tenant_id: &str, now_ms: i64) -> AelioResult<()> {
        {
            let mut db = self.database()?;
            for logical in LogicalTable::ALL {
                let table = self.resolver.resolve(tenant_id, logical)?;
                if db.columns(&table).is_none() {
                    let mut columns = common_columns();
                    if logical.has_embedding() {
                        columns.push(("embedding", ColumnKind::Vector(self.embedding_dim)));
                    }
                    db.create_table(&table, &columns).map_err(storage_error)?;
                }
            }
        }

        let migration = RecordEnvelope {
            key: "schema".into(),
            kind: "schema_version".into(),
            status: "active".into(),
            owner: "system".into(),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            expires_at_ms: None,
            value: SCHEMA_VERSION,
        };
        let migration_result =
            self.put_if_absent(tenant_id, LogicalTable::Migrations, &migration, None)?;
        if matches!(migration_result, PutIfAbsent::Existing { .. }) {
            if let Some(current) = self.get::<i64>(tenant_id, LogicalTable::Migrations, "schema")? {
                if current.envelope.value < SCHEMA_VERSION {
                    let mut next = migration;
                    next.created_at_ms = current.envelope.created_at_ms;
                    let _ = self.compare_swap(
                        tenant_id,
                        LogicalTable::Migrations,
                        current.row_id,
                        current.version,
                        &next,
                        None,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Atomically create a logical record by key.
    pub fn put_if_absent<T: Serialize>(
        &mut self,
        tenant_id: &str,
        logical: LogicalTable,
        envelope: &RecordEnvelope<T>,
        embedding: Option<&[f32]>,
    ) -> AelioResult<PutIfAbsent> {
        self.put_if_absent_with_links(tenant_id, logical, envelope, embedding, &[])
    }

    pub fn put_if_absent_with_links<T: Serialize>(
        &mut self,
        tenant_id: &str,
        logical: LogicalTable,
        envelope: &RecordEnvelope<T>,
        embedding: Option<&[f32]>,
        links: &[u64],
    ) -> AelioResult<PutIfAbsent> {
        let table = self.resolver.resolve(tenant_id, logical)?;
        self.ensure_embedding(logical, embedding)?;
        let payload = serde_json::to_string(envelope)
            .map_err(|e| AelioError::new(ReasonCode::Validation, e.to_string()))?;
        let mut values = record_values(envelope, payload);
        if let Some(vector) = embedding {
            values.push(("embedding", DbValue::Vector(vector.to_vec())));
        }
        if !links.is_empty() {
            values.push(("links", DbValue::Edges(links.to_vec())));
        }
        let conditions = [("key", DbValue::Utf8(envelope.key.clone()))];
        let condition_refs = db_refs(&conditions);
        let value_refs = db_refs(&values);
        let mut db = self.database()?;
        match db
            .insert_if_absent(&table, &condition_refs, &value_refs)
            .map_err(storage_error)?
        {
            InsertIfAbsent::Inserted { row_id, version } => {
                Ok(PutIfAbsent::Inserted { row_id, version })
            }
            InsertIfAbsent::Existing { row_id, version } => {
                Ok(PutIfAbsent::Existing { row_id, version })
            }
        }
    }

    pub fn get<T: DeserializeOwned>(
        &self,
        tenant_id: &str,
        logical: LogicalTable,
        key: &str,
    ) -> AelioResult<Option<StoredRecord<T>>> {
        let table = self.resolver.resolve(tenant_id, logical)?;
        let query = HybridQuery::new(2).filter("key", PredOp::Eq, DbValue::Utf8(key.into()));
        let db = self.database()?;
        let Some((row_id, _)) = self.database_query(&db, &table, &query)?.into_iter().next() else {
            return Ok(None);
        };
        Self::get_by_row_id(&db, &table, row_id)
    }

    /// Bounded deterministic scan used by background workers. Workers must still claim each
    /// returned row with `compare_swap`; a scan is discovery, never ownership.
    pub fn list<T: DeserializeOwned>(
        &self,
        tenant_id: &str,
        logical: LogicalTable,
        status: Option<&str>,
        limit: usize,
    ) -> AelioResult<Vec<StoredRecord<T>>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let table = self.resolver.resolve(tenant_id, logical)?;
        let mut query = HybridQuery::new(limit);
        if let Some(status) = status {
            query = query.filter("status", PredOp::Eq, DbValue::Utf8(status.into()));
        }
        let db = self.database()?;
        let mut records = Vec::new();
        for (row_id, _) in self.database_query(&db, &table, &query)? {
            if let Some(record) = Self::get_by_row_id(&db, &table, row_id)? {
                records.push(record);
            }
        }
        records.sort_by(|left, right| left.envelope.key.cmp(&right.envelope.key));
        Ok(records)
    }

    /// Bounded deterministic scan restricted to one envelope kind. Use this for logical tables
    /// that intentionally contain multiple closed record payloads.
    pub fn list_kind<T: DeserializeOwned>(
        &self,
        tenant_id: &str,
        logical: LogicalTable,
        kind: &str,
        status: Option<&str>,
        limit: usize,
    ) -> AelioResult<Vec<StoredRecord<T>>> {
        if kind.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let table = self.resolver.resolve(tenant_id, logical)?;
        let mut query =
            HybridQuery::new(limit).filter("kind", PredOp::Eq, DbValue::Utf8(kind.into()));
        if let Some(status) = status {
            query = query.filter("status", PredOp::Eq, DbValue::Utf8(status.into()));
        }
        let db = self.database()?;
        let mut records = Vec::new();
        for (row_id, _) in self.database_query(&db, &table, &query)? {
            if let Some(record) = Self::get_by_row_id(&db, &table, row_id)? {
                records.push(record);
            }
        }
        records.sort_by(|left, right| left.envelope.key.cmp(&right.envelope.key));
        Ok(records)
    }

    /// Compare-and-set a complete envelope. The key is immutable.
    pub fn compare_swap<T: Serialize>(
        &mut self,
        tenant_id: &str,
        logical: LogicalTable,
        row_id: u64,
        expected_version: u64,
        envelope: &RecordEnvelope<T>,
        embedding: Option<&[f32]>,
    ) -> AelioResult<CompareSwap> {
        let table = self.resolver.resolve(tenant_id, logical)?;
        self.ensure_embedding(logical, embedding)?;
        let mut db = self.database()?;
        let current: Option<StoredRecord<serde_json::Value>> =
            Self::get_by_row_id(&db, &table, row_id)?;
        let Some(current) = current else {
            return Ok(CompareSwap::NotFound);
        };
        if current.envelope.key != envelope.key {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "record keys cannot be changed by compare_swap",
            ));
        }
        let payload = serde_json::to_string(envelope)
            .map_err(|e| AelioError::new(ReasonCode::Validation, e.to_string()))?;
        let mut values = record_values(envelope, payload);
        if let Some(vector) = embedding {
            values.push(("embedding", DbValue::Vector(vector.to_vec())));
        }
        let value_refs = db_refs(&values);
        Ok(
            match db
                .update_if_version(&table, row_id, expected_version, &value_refs)
                .map_err(storage_error)?
            {
                UpdateIfVersion::Updated { version } => CompareSwap::Updated { version },
                UpdateIfVersion::Conflict { current_version } => {
                    CompareSwap::Conflict { current_version }
                }
                UpdateIfVersion::NotFound => CompareSwap::NotFound,
            },
        )
    }

    /// Retrieve nearest durable records in a tenant table. Payloads are hydrated and invalid
    /// or expired rows can be rejected by the caller before execution.
    pub fn semantic_search<T: DeserializeOwned>(
        &self,
        tenant_id: &str,
        logical: LogicalTable,
        query_vector: &[f32],
        status: Option<&str>,
        k: usize,
    ) -> AelioResult<Vec<(StoredRecord<T>, f32)>> {
        if !logical.has_embedding() {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "semantic search requires an embedding-enabled logical table",
            ));
        }
        if query_vector.len() != self.embedding_dim as usize {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "query vector dimension does not match the tenant schema",
            ));
        }
        let table = self.resolver.resolve(tenant_id, logical)?;
        let mut query = HybridQuery::new(k).vector("embedding", query_vector.to_vec());
        if let Some(status) = status {
            query = query.filter("status", PredOp::Eq, DbValue::Utf8(status.into()));
        }
        let db = self.database()?;
        let hits = db.query(&table, &query).map_err(storage_error)?;
        let mut records = Vec::with_capacity(hits.len());
        for (row_id, score) in hits {
            if let Some(record) = Self::get_by_row_id(&db, &table, row_id)? {
                records.push((record, score));
            }
        }
        Ok(records)
    }

    /// BM25 search with tenant-table, status, and expiry predicates applied in Aelio DB.
    pub fn lexical_search<T: DeserializeOwned>(
        &self,
        tenant_id: &str,
        logical: LogicalTable,
        text: &str,
        filter: RecallFilter<'_>,
    ) -> AelioResult<Vec<(StoredRecord<T>, f32)>> {
        self.search_live(tenant_id, logical, SearchAnchor::Text(text), filter)
    }

    /// Vector search with native tenant-table, status, and expiry predicates.
    pub fn semantic_search_live<T: DeserializeOwned>(
        &self,
        tenant_id: &str,
        logical: LogicalTable,
        vector: &[f32],
        filter: RecallFilter<'_>,
    ) -> AelioResult<Vec<(StoredRecord<T>, f32)>> {
        if vector.len() != self.embedding_dim as usize || !logical.has_embedding() {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "semantic query does not match an embedding-enabled tenant table",
            ));
        }
        self.search_live(tenant_id, logical, SearchAnchor::Vector(vector), filter)
    }

    /// Tenant-scoped graph traversal. Intermediate hops are table-scoped by aelio-db-query.
    pub fn graph_search<T: DeserializeOwned>(
        &self,
        tenant_id: &str,
        logical: LogicalTable,
        seeds: &[u64],
        depth: usize,
        budget: aelio_db_query::GraphBudget,
        filter: RecallFilter<'_>,
    ) -> AelioResult<Vec<(StoredRecord<T>, f32)>> {
        let table = self.resolver.resolve(tenant_id, logical)?;
        let db = self.database()?;
        let mut merged = std::collections::HashMap::<u64, f32>::new();
        for expiry in [
            (PredOp::Eq, DbValue::Null),
            (PredOp::Gt, DbValue::I64(filter.now_ms)),
        ] {
            let mut query = HybridQuery::new(filter.k)
                .graph("links", seeds.to_vec(), depth)
                .graph_budget(budget)
                .graph_scope_filters()
                .filter("expires_at", expiry.0, expiry.1);
            if let Some(status) = filter.status {
                query = query.filter("status", PredOp::Eq, DbValue::Utf8(status.into()));
            }
            if let Some(owner) = filter.owner {
                query = query.filter("owner", PredOp::Eq, DbValue::Utf8(owner.into()));
            }
            for (row_id, score) in db.query_checked(&table, &query).map_err(query_error)? {
                merged.insert(row_id, score);
            }
        }
        let mut hits: Vec<_> = merged.into_iter().collect();
        hits.sort_by(|left, right| left.0.cmp(&right.0));
        hits.truncate(filter.k);
        self.hydrate_hits(&db, &table, hits)
    }

    fn search_live<T: DeserializeOwned>(
        &self,
        tenant_id: &str,
        logical: LogicalTable,
        anchor: SearchAnchor<'_>,
        filter: RecallFilter<'_>,
    ) -> AelioResult<Vec<(StoredRecord<T>, f32)>> {
        if filter.k == 0 {
            return Ok(Vec::new());
        }
        let table = self.resolver.resolve(tenant_id, logical)?;
        let mut merged = std::collections::HashMap::<u64, f32>::new();
        let db = self.database()?;
        // Null expiry and future expiry are two native plans because the predicate language
        // deliberately has no implicit OR.
        for expiry in [
            (PredOp::Eq, DbValue::Null),
            (PredOp::Gt, DbValue::I64(filter.now_ms)),
        ] {
            let mut query = HybridQuery::new(filter.k.saturating_mul(2).max(filter.k));
            query = match anchor {
                SearchAnchor::Text(text) => query.text("payload", text),
                SearchAnchor::Vector(vector) => query.vector("embedding", vector.to_vec()),
            };
            query = query.filter("expires_at", expiry.0, expiry.1);
            if let Some(status) = filter.status {
                query = query.filter("status", PredOp::Eq, DbValue::Utf8(status.into()));
            }
            if let Some(owner) = filter.owner {
                query = query.filter("owner", PredOp::Eq, DbValue::Utf8(owner.into()));
            }
            for (row_id, score) in db.query(&table, &query).map_err(storage_error)? {
                merged
                    .entry(row_id)
                    .and_modify(|current| *current = current.max(score))
                    .or_insert(score);
            }
        }
        let mut hits: Vec<_> = merged.into_iter().collect();
        hits.sort_by(|left, right| right.1.total_cmp(&left.1).then(left.0.cmp(&right.0)));
        hits.truncate(filter.k);
        self.hydrate_hits(&db, &table, hits)
    }

    fn hydrate_hits<T: DeserializeOwned>(
        &self,
        db: &Database,
        table: &str,
        hits: Vec<(u64, f32)>,
    ) -> AelioResult<Vec<(StoredRecord<T>, f32)>> {
        let mut records = Vec::with_capacity(hits.len());
        for (row_id, score) in hits {
            if let Some(record) = Self::get_by_row_id(db, table, row_id)? {
                records.push((record, score));
            }
        }
        Ok(records)
    }

    pub fn flush(&mut self) -> AelioResult<()> {
        self.database()?.flush().map_err(storage_error)
    }

    fn ensure_embedding(
        &self,
        logical: LogicalTable,
        embedding: Option<&[f32]>,
    ) -> AelioResult<()> {
        if let Some(vector) = embedding {
            if !logical.has_embedding() {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    "logical table does not support embeddings",
                ));
            }
            if vector.len() != self.embedding_dim as usize {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    "embedding dimension does not match the tenant schema",
                ));
            }
        }
        Ok(())
    }

    fn get_by_row_id<T: DeserializeOwned>(
        db: &Database,
        table: &str,
        row_id: u64,
    ) -> AelioResult<Option<StoredRecord<T>>> {
        let Some(values) = db.get_row_values(table, row_id).map_err(storage_error)? else {
            return Ok(None);
        };
        let version = db
            .row_version(table, row_id)
            .map_err(storage_error)?
            .ok_or_else(|| AelioError::new(ReasonCode::NotFound, "row version is unavailable"))?;
        let payload = values
            .iter()
            .find_map(|(name, value)| match (name.as_str(), value) {
                ("payload", DbValue::Utf8(payload)) => Some(payload),
                _ => None,
            })
            .ok_or_else(|| AelioError::new(ReasonCode::Internal, "stored record has no payload"))?;
        let envelope = serde_json::from_str(payload)
            .map_err(|e| AelioError::new(ReasonCode::Internal, e.to_string()))?;
        Ok(Some(StoredRecord {
            row_id,
            version,
            envelope,
        }))
    }

    fn database(&self) -> AelioResult<MutexGuard<'_, Database>> {
        self.db
            .lock()
            .map_err(|_| AelioError::new(ReasonCode::Internal, "database lock poisoned"))
    }

    fn database_query(
        &self,
        db: &Database,
        table: &str,
        query: &HybridQuery,
    ) -> AelioResult<Vec<(u64, f32)>> {
        db.query(table, query).map_err(storage_error)
    }
}

fn common_columns() -> Vec<(&'static str, ColumnKind)> {
    vec![
        ("key", ColumnKind::Utf8),
        ("kind", ColumnKind::Utf8),
        ("status", ColumnKind::Utf8),
        ("owner", ColumnKind::Utf8),
        ("payload", ColumnKind::Text),
        ("created_at", ColumnKind::Timestamp),
        ("updated_at", ColumnKind::Timestamp),
        ("expires_at", ColumnKind::Timestamp),
        ("links", ColumnKind::Edge),
    ]
}

fn record_values<T>(envelope: &RecordEnvelope<T>, payload: String) -> Vec<(&'static str, DbValue)> {
    vec![
        ("key", DbValue::Utf8(envelope.key.clone())),
        ("kind", DbValue::Utf8(envelope.kind.clone())),
        ("status", DbValue::Utf8(envelope.status.clone())),
        ("owner", DbValue::Utf8(envelope.owner.clone())),
        ("payload", DbValue::Utf8(payload)),
        ("created_at", DbValue::I64(envelope.created_at_ms)),
        ("updated_at", DbValue::I64(envelope.updated_at_ms)),
        (
            "expires_at",
            envelope.expires_at_ms.map_or(DbValue::Null, DbValue::I64),
        ),
    ]
}

fn db_refs<'a>(values: &'a [(&'a str, DbValue)]) -> Vec<(&'a str, DbValue)> {
    values
        .iter()
        .map(|(name, value)| (*name, value.clone()))
        .collect()
}

fn storage_error(error: io::Error) -> AelioError {
    AelioError::new(ReasonCode::Internal, format!("storage: {error}"))
}

#[derive(Clone, Copy)]
enum SearchAnchor<'a> {
    Text(&'a str),
    Vector(&'a [f32]),
}

fn query_error(error: aelio_db_query::DatabaseQueryError) -> AelioError {
    match error {
        aelio_db_query::DatabaseQueryError::Execution(
            aelio_db_query::QueryError::GraphBudgetExceeded(exceeded),
        ) => {
            let message = format!(
                "graph {:?} budget exceeded: observed {}, limit {}",
                exceeded.kind, exceeded.observed, exceeded.limit
            );
            AelioError::new(ReasonCode::BudgetExceeded, message).with_detail(crate::Value::Map(
                indexmap::indexmap! {
                    "kind".into() => crate::Value::str(format!("{:?}", exceeded.kind).to_lowercase()),
                    "limit".into() => crate::Value::str(exceeded.limit.to_string()),
                    "observed".into() => crate::Value::str(exceeded.observed.to_string()),
                },
            ))
        }
        aelio_db_query::DatabaseQueryError::Planning(error) => storage_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("aelio_store_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn envelope(key: &str, value: &str, now: i64) -> RecordEnvelope<String> {
        RecordEnvelope {
            key: key.into(),
            kind: "test".into(),
            status: "active".into(),
            owner: "runtime".into(),
            created_at_ms: now,
            updated_at_ms: now,
            expires_at_ms: None,
            value: value.into(),
        }
    }

    #[test]
    fn tenant_tables_are_distinct_and_not_user_named() {
        let resolver = TableResolver::default();
        let a = resolver.resolve("tenant-a", LogicalTable::Turns).unwrap();
        let b = resolver.resolve("tenant-b", LogicalTable::Turns).unwrap();
        assert_ne!(a, b);
        assert!(!a.contains("tenant-a"));
    }

    #[test]
    fn durable_put_and_compare_swap_survive_flush() {
        let path = temp_dir("cas");
        let db = Database::create(&path).unwrap();
        let mut store = AelioStore::new(db, 3).unwrap();
        store.migrate_tenant("t1", 1).unwrap();

        let first = store
            .put_if_absent(
                "t1",
                LogicalTable::FlowInstances,
                &envelope("flow:u1", "v1", 1),
                None,
            )
            .unwrap();
        let PutIfAbsent::Inserted { row_id, version } = first else {
            panic!("first insert must win");
        };
        assert!(matches!(
            store
                .put_if_absent(
                    "t1",
                    LogicalTable::FlowInstances,
                    &envelope("flow:u1", "duplicate", 1),
                    None,
                )
                .unwrap(),
            PutIfAbsent::Existing { .. }
        ));
        store.flush().unwrap();

        let mut next = envelope("flow:u1", "v2", 2);
        next.status = "parked".into();
        let updated = store
            .compare_swap(
                "t1",
                LogicalTable::FlowInstances,
                row_id,
                version,
                &next,
                None,
            )
            .unwrap();
        let CompareSwap::Updated {
            version: next_version,
        } = updated
        else {
            panic!("matching version must update");
        };
        assert!(next_version > version);
        assert_eq!(
            store
                .compare_swap(
                    "t1",
                    LogicalTable::FlowInstances,
                    row_id,
                    version,
                    &envelope("flow:u1", "stale", 3),
                    None,
                )
                .unwrap(),
            CompareSwap::Conflict {
                current_version: next_version
            }
        );

        let loaded: StoredRecord<String> = store
            .get("t1", LogicalTable::FlowInstances, "flow:u1")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.envelope.value, "v2");
    }

    #[test]
    fn procedure_vectors_are_tenant_scoped() {
        let path = temp_dir("vectors");
        let db = Database::create(&path).unwrap();
        let mut store = AelioStore::new(db, 3).unwrap();
        store.migrate_tenant("a", 1).unwrap();
        store.migrate_tenant("b", 1).unwrap();
        store
            .put_if_absent(
                "a",
                LogicalTable::Procedures,
                &envelope("p1", "tenant-a", 1),
                Some(&[1.0, 0.0, 0.0]),
            )
            .unwrap();
        store
            .put_if_absent(
                "b",
                LogicalTable::Procedures,
                &envelope("p1", "tenant-b", 1),
                Some(&[1.0, 0.0, 0.0]),
            )
            .unwrap();

        let hits: Vec<(StoredRecord<String>, f32)> = store
            .semantic_search(
                "a",
                LogicalTable::Procedures,
                &[1.0, 0.0, 0.0],
                Some("active"),
                5,
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0.envelope.value, "tenant-a");
    }
}
