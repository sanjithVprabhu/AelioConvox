//! # aelio-store — trait boundary + in-memory double (§24, F-002/F-003)
//!
//! Sunjet is the production backend; this crate owns the **trait** so the kernel stays
//! Sunjet-agnostic. The in-memory double exists for tests and P0 only.

use aelio_sol::SolValue;
use std::collections::BTreeMap;
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
    let next = SolValue::map([
        ("status", SolValue::str("result")),
        ("result", result),
    ]);
    store.cas(tenant, ONCE_TABLE, idem_key, row.version, next)?;
    Ok(())
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
        let g = self.inner.lock().map_err(|e| StoreError::Internal(e.to_string()))?;
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
        let mut g = self.inner.lock().map_err(|e| StoreError::Internal(e.to_string()))?;
        let k = (tenant.into(), table.into(), key.into());
        if let Some(existing) = g.rows.get(&k) {
            return Ok(PutIfAbsent::Existing(existing.clone()));
        }
        g.rows.insert(
            k,
            Versioned {
                value,
                version: 1,
            },
        );
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
        let mut g = self.inner.lock().map_err(|e| StoreError::Internal(e.to_string()))?;
        let k = (tenant.into(), table.into(), key.into());
        let row = g.rows.get_mut(&k).ok_or(StoreError::NotFound)?;
        if row.version != expected_version {
            return Err(StoreError::Conflict);
        }
        row.version += 1;
        row.value = value;
        Ok(row.version)
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

        assert_eq!(once_begin(&mut store, tenant, key).unwrap(), OnceState::Execute);
        // Crash window: intent without result.
        assert_eq!(
            once_begin(&mut store, tenant, key).unwrap_err(),
            StoreError::UnknownOutcome
        );

        // Fresh key completes and replays.
        let key2 = "inst|n_send|def";
        assert_eq!(once_begin(&mut store, tenant, key2).unwrap(), OnceState::Execute);
        once_complete(&mut store, tenant, key2, SolValue::map([("sent", SolValue::Bool(true))])).unwrap();
        match once_begin(&mut store, tenant, key2).unwrap() {
            OnceState::Replay(v) => {
                assert_eq!(
                    v.as_map().unwrap().get("sent"),
                    Some(&SolValue::Bool(true))
                );
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
            store.cas("t", "states", "k", 1, SolValue::Int(3)).unwrap_err(),
            StoreError::Conflict
        );
    }
}
