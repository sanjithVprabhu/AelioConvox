//! # aelio-store — embedded Aelio database binding + test double (§24)
//!
//! The kernel depends on this trait, [`EmbeddedStore`] is the production implementation over the
//! database engine in this workspace, and [`MemoryStore`] is the deterministic test double.

use aelio_db_query::{
    ColumnKind, Database, HybridQuery, InsertIfAbsent as DbInsertIfAbsent, PredOp, UpdateIfVersion,
    Value as DbValue,
};
use aelio_sol::SolValue;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Compare-and-swap conflict or unknown Once outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    Conflict,
    /// §8.4: intent without result ⇒ fail-loud, never silent re-execute.
    UnknownOutcome,
    NotFound,
    Internal(String),
}

/// Per-key CAS row used by `states` / Once accounting.
#[derive(Debug, Clone)]
pub struct Versioned {
    pub value: SolValue,
    pub version: u64,
}

/// Minimal store surface for P0: Once intent/result + generic CAS.
pub trait Store: Send {
    fn get(&self, tenant: &str, table: &str, key: &str) -> Result<Option<Versioned>, StoreError>;

    /// Insert only if absent. Returns existing if present.
    fn put_if_absent(
        &mut self,
        tenant: &str,
        table: &str,
        key: &str,
        value: SolValue,
    ) -> Result<PutIfAbsent, StoreError>;

    /// CAS: succeed only when `expected_version` matches.
    fn cas(
        &mut self,
        tenant: &str,
        table: &str,
        key: &str,
        expected_version: u64,
        value: SolValue,
    ) -> Result<u64, StoreError>;

    /// Bounded, key-ordered scan used to hydrate append-only per-instance records.
    fn scan_prefix(
        &self,
        tenant: &str,
        table: &str,
        key_prefix: &str,
        limit: usize,
    ) -> Result<Vec<(String, Versioned)>, StoreError>;
}

#[derive(Debug, Clone)]
pub enum PutIfAbsent {
    Inserted { version: u64 },
    Existing(Versioned),
}

/// Once lifecycle over a store (§8.4).
#[derive(Debug, Clone, PartialEq)]
pub enum OnceState {
    /// First claim — caller must execute body then `complete`.
    Execute,
    /// Prior success — return recorded bag fragment / marker.
    Replay(SolValue),
}

const ONCE_TABLE: &str = "once_intents";

/// Begin an at-most-once region. `idem_key` is already fully formed (includes tenant/instance/nid).
pub fn once_begin(
    store: &mut dyn Store,
    tenant: &str,
    idem_key: &str,
) -> Result<OnceState, StoreError> {
    let intent = SolValue::map([
        ("status", SolValue::str("intent")),
        ("result", SolValue::Null),
    ]);
    match store.put_if_absent(tenant, ONCE_TABLE, idem_key, intent)? {
        PutIfAbsent::Inserted { .. } => Ok(OnceState::Execute),
        PutIfAbsent::Existing(row) => {
            let status = row
                .value
                .as_map()
                .and_then(|m| m.get("status"))
                .and_then(|s| match s {
                    SolValue::Str(x) => Some(x.as_str()),
                    _ => None,
                })
                .unwrap_or("");
            match status {
                "result" => {
                    let result = row
                        .value
                        .as_map()
                        .and_then(|m| m.get("result"))
                        .cloned()
                        .unwrap_or(SolValue::Null);
                    Ok(OnceState::Replay(result))
                }
                "intent" => Err(StoreError::UnknownOutcome),
                _ => Err(StoreError::Internal(format!("bad once status `{status}`"))),
            }
        }
    }
}

/// Complete a once region after successful body execution.
pub fn once_complete(
    store: &mut dyn Store,
    tenant: &str,
    idem_key: &str,
    result: SolValue,
) -> Result<(), StoreError> {
    let row = store
        .get(tenant, ONCE_TABLE, idem_key)?
        .ok_or(StoreError::NotFound)?;
    let next = SolValue::map([("status", SolValue::str("result")), ("result", result)]);
    store.cas(tenant, ONCE_TABLE, idem_key, row.version, next)?;
    Ok(())
}

// ── Embedded Aelio database implementation ───────────────────────────────────────────────────

#[derive(Clone)]
pub struct EmbeddedStore {
    db: Arc<Mutex<Database>>,
}

impl EmbeddedStore {
    /// Open or create the embedded database rooted at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        std::fs::create_dir_all(path.as_ref())
            .map_err(|error| StoreError::Internal(error.to_string()))?;
        let db = Database::open(path.as_ref())
            .map_err(|error| StoreError::Internal(error.to_string()))?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
        })
    }

    fn physical_table(tenant: &str, table: &str) -> Result<String, StoreError> {
        if tenant.is_empty()
            || table.is_empty()
            || !table
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(StoreError::Internal(
                "tenant and identifier-safe logical table are required".into(),
            ));
        }
        let tenant_hash = blake3::hash(tenant.as_bytes()).to_hex();
        Ok(format!("aelio_t_{}_{}", &tenant_hash[..16], table))
    }

    fn database(&self) -> Result<std::sync::MutexGuard<'_, Database>, StoreError> {
        self.db
            .lock()
            .map_err(|error| StoreError::Internal(error.to_string()))
    }

    fn ensure_table(db: &mut Database, table: &str) -> Result<(), StoreError> {
        if db.columns(table).is_none() {
            db.create_table(
                table,
                &[("key", ColumnKind::Utf8), ("payload", ColumnKind::Utf8)],
            )
            .map_err(|error| StoreError::Internal(error.to_string()))?;
        }
        Ok(())
    }

    fn row_for_key(
        db: &Database,
        table: &str,
        key: &str,
    ) -> Result<Option<(u64, Versioned)>, StoreError> {
        let query = HybridQuery::new(1).filter("key", PredOp::Eq, DbValue::Utf8(key.to_owned()));
        let Some((row_id, values)) = db
            .scan_values(table, &query)
            .map_err(|error| StoreError::Internal(error.to_string()))?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let payload = values
            .into_iter()
            .find_map(|(name, value)| match (name.as_str(), value) {
                ("payload", DbValue::Utf8(payload)) => Some(payload),
                _ => None,
            })
            .ok_or_else(|| StoreError::Internal("database row lacks payload".into()))?;
        let version = db
            .row_version(table, row_id)
            .map_err(|error| StoreError::Internal(error.to_string()))?
            .ok_or(StoreError::NotFound)?;
        Ok(Some((
            row_id,
            Versioned {
                value: parse_sol(&payload)?,
                version,
            },
        )))
    }
}

impl Store for EmbeddedStore {
    fn get(&self, tenant: &str, table: &str, key: &str) -> Result<Option<Versioned>, StoreError> {
        let physical = Self::physical_table(tenant, table)?;
        let mut db = self.database()?;
        Self::ensure_table(&mut db, &physical)?;
        Ok(Self::row_for_key(&db, &physical, key)?.map(|(_, row)| row))
    }

    fn put_if_absent(
        &mut self,
        tenant: &str,
        table: &str,
        key: &str,
        value: SolValue,
    ) -> Result<PutIfAbsent, StoreError> {
        if key.is_empty() {
            return Err(StoreError::Internal("store key must not be empty".into()));
        }
        let physical = Self::physical_table(tenant, table)?;
        let payload = aelio_sol::canonical_string(&value);
        let mut db = self.database()?;
        Self::ensure_table(&mut db, &physical)?;
        match db
            .insert_if_absent(
                &physical,
                &[("key", DbValue::Utf8(key.to_owned()))],
                &[
                    ("key", DbValue::Utf8(key.to_owned())),
                    ("payload", DbValue::Utf8(payload)),
                ],
            )
            .map_err(|error| StoreError::Internal(error.to_string()))?
        {
            DbInsertIfAbsent::Inserted { version, .. } => Ok(PutIfAbsent::Inserted { version }),
            DbInsertIfAbsent::Existing { .. } => {
                let existing = Self::row_for_key(&db, &physical, key)?
                    .map(|(_, row)| row)
                    .ok_or(StoreError::NotFound)?;
                Ok(PutIfAbsent::Existing(existing))
            }
        }
    }

    fn cas(
        &mut self,
        tenant: &str,
        table: &str,
        key: &str,
        expected_version: u64,
        value: SolValue,
    ) -> Result<u64, StoreError> {
        let physical = Self::physical_table(tenant, table)?;
        let payload = aelio_sol::canonical_string(&value);
        let mut db = self.database()?;
        Self::ensure_table(&mut db, &physical)?;
        let (row_id, _) = Self::row_for_key(&db, &physical, key)?.ok_or(StoreError::NotFound)?;
        match db
            .update_if_version(
                &physical,
                row_id,
                expected_version,
                &[("payload", DbValue::Utf8(payload))],
            )
            .map_err(|error| StoreError::Internal(error.to_string()))?
        {
            UpdateIfVersion::Updated { version } => Ok(version),
            UpdateIfVersion::Conflict { .. } => Err(StoreError::Conflict),
            UpdateIfVersion::NotFound => Err(StoreError::NotFound),
        }
    }

    fn scan_prefix(
        &self,
        tenant: &str,
        table: &str,
        key_prefix: &str,
        limit: usize,
    ) -> Result<Vec<(String, Versioned)>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let physical = Self::physical_table(tenant, table)?;
        let mut db = self.database()?;
        Self::ensure_table(&mut db, &physical)?;
        // UTF-8 keys beginning with `prefix` fall in [prefix, prefix + U+10FFFF).
        let upper = format!("{key_prefix}\u{10ffff}");
        let query = HybridQuery::new(limit)
            .filter("key", PredOp::Ge, DbValue::Utf8(key_prefix.to_owned()))
            .filter("key", PredOp::Lt, DbValue::Utf8(upper));
        let mut rows = Vec::new();
        for (row_id, values) in db
            .scan_values(&physical, &query)
            .map_err(|error| StoreError::Internal(error.to_string()))?
        {
            let mut key = None;
            let mut payload = None;
            for (name, value) in values {
                match (name.as_str(), value) {
                    ("key", DbValue::Utf8(value)) => key = Some(value),
                    ("payload", DbValue::Utf8(value)) => payload = Some(value),
                    _ => {}
                }
            }
            let key = key.ok_or_else(|| StoreError::Internal("database row lacks key".into()))?;
            let payload =
                payload.ok_or_else(|| StoreError::Internal("database row lacks payload".into()))?;
            let version = db
                .row_version(&physical, row_id)
                .map_err(|error| StoreError::Internal(error.to_string()))?
                .ok_or(StoreError::NotFound)?;
            rows.push((
                key,
                Versioned {
                    value: parse_sol(&payload)?,
                    version,
                },
            ));
        }
        rows.sort_by(|left, right| left.0.cmp(&right.0));
        rows.truncate(limit);
        Ok(rows)
    }
}

fn parse_sol(text: &str) -> Result<SolValue, StoreError> {
    let json: serde_json::Value =
        serde_json::from_str(text).map_err(|error| StoreError::Internal(error.to_string()))?;
    fn convert(value: &serde_json::Value) -> Result<SolValue, StoreError> {
        Ok(match value {
            serde_json::Value::Null => SolValue::Null,
            serde_json::Value::Bool(value) => SolValue::Bool(*value),
            serde_json::Value::Number(value) => {
                if let Some(value) = value.as_i64() {
                    SolValue::Int(value)
                } else {
                    SolValue::float(
                        value
                            .as_f64()
                            .ok_or_else(|| StoreError::Internal("number out of range".into()))?,
                    )
                    .map_err(|error| StoreError::Internal(error.to_string()))?
                }
            }
            serde_json::Value::String(value) => SolValue::str(value),
            serde_json::Value::Array(values) => {
                SolValue::List(values.iter().map(convert).collect::<Result<_, _>>()?)
            }
            serde_json::Value::Object(values) => SolValue::map(
                values
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), convert(value)?)))
                    .collect::<Result<Vec<_>, StoreError>>()?,
            ),
        })
    }
    let value = convert(&json)?;
    aelio_sol::Limits::default()
        .check(&value)
        .map_err(|error| StoreError::Internal(error.to_string()))?;
    Ok(value)
}

// ── In-memory double ──────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Inner {
    /// (tenant, table, key) → Versioned
    rows: BTreeMap<(String, String, String), Versioned>,
}

/// Thread-safe in-memory store (test double + P0 default).
#[derive(Clone, Default)]
pub struct MemoryStore {
    inner: Arc<Mutex<Inner>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Store for MemoryStore {
    fn get(&self, tenant: &str, table: &str, key: &str) -> Result<Option<Versioned>, StoreError> {
        let g = self
            .inner
            .lock()
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(g.rows
            .get(&(tenant.into(), table.into(), key.into()))
            .cloned())
    }

    fn put_if_absent(
        &mut self,
        tenant: &str,
        table: &str,
        key: &str,
        value: SolValue,
    ) -> Result<PutIfAbsent, StoreError> {
        let mut g = self
            .inner
            .lock()
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        let k = (tenant.into(), table.into(), key.into());
        if let Some(existing) = g.rows.get(&k) {
            return Ok(PutIfAbsent::Existing(existing.clone()));
        }
        g.rows.insert(k, Versioned { value, version: 1 });
        Ok(PutIfAbsent::Inserted { version: 1 })
    }

    fn cas(
        &mut self,
        tenant: &str,
        table: &str,
        key: &str,
        expected_version: u64,
        value: SolValue,
    ) -> Result<u64, StoreError> {
        let mut g = self
            .inner
            .lock()
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        let k = (tenant.into(), table.into(), key.into());
        let row = g.rows.get_mut(&k).ok_or(StoreError::NotFound)?;
        if row.version != expected_version {
            return Err(StoreError::Conflict);
        }
        row.version += 1;
        row.value = value;
        Ok(row.version)
    }

    fn scan_prefix(
        &self,
        tenant: &str,
        table: &str,
        key_prefix: &str,
        limit: usize,
    ) -> Result<Vec<(String, Versioned)>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let g = self
            .inner
            .lock()
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(g.rows
            .iter()
            .filter(|((row_tenant, row_table, key), _)| {
                row_tenant == tenant && row_table == table && key.starts_with(key_prefix)
            })
            .take(limit)
            .map(|((_, _, key), row)| (key.clone(), row.clone()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn once_is_at_most_once_and_fail_loud_on_orphan_intent() {
        let mut store = MemoryStore::new();
        let tenant = "t1";
        let key = "inst|n_send|abc";

        assert_eq!(
            once_begin(&mut store, tenant, key).unwrap(),
            OnceState::Execute
        );
        // Crash window: intent without result.
        assert_eq!(
            once_begin(&mut store, tenant, key).unwrap_err(),
            StoreError::UnknownOutcome
        );

        // Fresh key completes and replays.
        let key2 = "inst|n_send|def";
        assert_eq!(
            once_begin(&mut store, tenant, key2).unwrap(),
            OnceState::Execute
        );
        once_complete(
            &mut store,
            tenant,
            key2,
            SolValue::map([("sent", SolValue::Bool(true))]),
        )
        .unwrap();
        match once_begin(&mut store, tenant, key2).unwrap() {
            OnceState::Replay(v) => {
                assert_eq!(v.as_map().unwrap().get("sent"), Some(&SolValue::Bool(true)));
            }
            OnceState::Execute => panic!("must replay"),
        }
    }

    #[test]
    fn cas_conflicts_on_stale_version() {
        let mut store = MemoryStore::new();
        store
            .put_if_absent("t", "states", "k", SolValue::Int(1))
            .unwrap();
        assert!(store.cas("t", "states", "k", 1, SolValue::Int(2)).is_ok());
        assert_eq!(
            store
                .cas("t", "states", "k", 1, SolValue::Int(3))
                .unwrap_err(),
            StoreError::Conflict
        );
    }

    #[test]
    fn embedded_store_survives_restart_and_preserves_cas_and_prefix_scan() {
        let directory = tempfile::tempdir().unwrap();
        let first_version;
        {
            let mut store = EmbeddedStore::open(directory.path()).unwrap();
            let inserted = store
                .put_if_absent(
                    "tenant-a",
                    "ledger",
                    "instance-1:0001",
                    SolValue::map([("value", SolValue::Int(1))]),
                )
                .unwrap();
            first_version = match inserted {
                PutIfAbsent::Inserted { version } => version,
                PutIfAbsent::Existing(_) => panic!("fresh row must insert"),
            };
            store
                .put_if_absent(
                    "tenant-a",
                    "ledger",
                    "other:0001",
                    SolValue::map([("value", SolValue::Int(9))]),
                )
                .unwrap();
        }

        let mut reopened = EmbeddedStore::open(directory.path()).unwrap();
        let row = reopened
            .get("tenant-a", "ledger", "instance-1:0001")
            .unwrap()
            .unwrap();
        assert_eq!(
            row.value.as_map().unwrap().get("value"),
            Some(&SolValue::Int(1))
        );
        let next_version = reopened
            .cas(
                "tenant-a",
                "ledger",
                "instance-1:0001",
                first_version,
                SolValue::map([("value", SolValue::Int(2))]),
            )
            .unwrap();
        assert!(next_version > first_version);
        assert_eq!(
            reopened
                .cas(
                    "tenant-a",
                    "ledger",
                    "instance-1:0001",
                    first_version,
                    SolValue::Null,
                )
                .unwrap_err(),
            StoreError::Conflict
        );
        let rows = reopened
            .scan_prefix("tenant-a", "ledger", "instance-1:", 10)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "instance-1:0001");
        assert!(reopened
            .get("tenant-b", "ledger", "instance-1:0001")
            .unwrap()
            .is_none());
    }
}
