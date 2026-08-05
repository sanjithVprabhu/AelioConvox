//! Durable persistence for process trees (Phase 2.5).
//!
//! Tables (tenant-scoped via [`aelio_store::Store`]):
//! - [`INSTANCE_TABLE`] — `InstanceRecordV1` JSON envelopes
//! - [`JOIN_TABLE`] — `ChildJoinV1` JSON envelopes
//!
//! Keys:
//! - instances: `{subject_id}/{instance_id}`
//! - joins: `{subject_id}/{join_id}`
//!
//! Writes use put-if-absent on create and CAS on update so concurrent workers
//! fail closed on version conflicts.

use crate::os_contract::{ChildJoinV1, InstanceRecordV1};
use crate::process_tree::{InstanceBudgetV1, ProcessTree, WaitPredicate};
use aelio_sol::SolValue;
use aelio_store::{PutIfAbsent, Store, StoreError};
use std::collections::BTreeMap;

pub const INSTANCE_TABLE: &str = "process_instances_v1";
pub const JOIN_TABLE: &str = "process_joins_v1";
pub const WAIT_TABLE: &str = "process_waits_v1";
pub const BUDGET_TABLE: &str = "process_budgets_v1";

fn encode_json(kind: &'static str, value: &impl serde::Serialize) -> Result<SolValue, StoreError> {
    let json = serde_json::to_string(value)
        .map_err(|e| StoreError::Internal(format!("serialize {kind}: {e}")))?;
    Ok(SolValue::map([
        ("kind", SolValue::str(kind)),
        ("json", SolValue::str(json)),
    ]))
}

fn decode_json<T: serde::de::DeserializeOwned>(
    kind: &str,
    value: &SolValue,
) -> Result<T, StoreError> {
    let map = value
        .as_map()
        .ok_or_else(|| StoreError::Internal(format!("{kind} envelope must be a map")))?;
    let got_kind = map
        .get("kind")
        .and_then(|v| match v {
            SolValue::Str(s) => Some(s.as_str()),
            _ => None,
        })
        .unwrap_or("");
    if got_kind != kind {
        return Err(StoreError::Internal(format!(
            "expected kind `{kind}`, got `{got_kind}`"
        )));
    }
    let json = map
        .get("json")
        .and_then(|v| match v {
            SolValue::Str(s) => Some(s.as_str()),
            _ => None,
        })
        .ok_or_else(|| StoreError::Internal(format!("{kind} missing json field")))?;
    serde_json::from_str(json).map_err(|e| StoreError::Internal(format!("decode {kind}: {e}")))
}

fn instance_key(subject_id: &str, instance_id: &str) -> String {
    format!("{subject_id}/{instance_id}")
}

fn join_key(subject_id: &str, join_id: &str) -> String {
    format!("{subject_id}/{join_id}")
}

/// Repository over a store for one tenant.
pub struct ProcessRepository<'a> {
    store: &'a mut dyn Store,
    tenant_id: String,
    /// Local CAS versions keyed by table/key.
    versions: BTreeMap<(String, String), u64>,
}

impl<'a> ProcessRepository<'a> {
    pub fn new(store: &'a mut dyn Store, tenant_id: impl Into<String>) -> Self {
        Self {
            store,
            tenant_id: tenant_id.into(),
            versions: BTreeMap::new(),
        }
    }

    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    pub(crate) fn remember(&mut self, table: &str, key: &str, version: u64) {
        self.versions
            .insert((table.into(), key.into()), version);
    }

    fn known_version(&self, table: &str, key: &str) -> Option<u64> {
        self.versions
            .get(&(table.into(), key.into()))
            .copied()
    }

    /// Insert or CAS-update an instance record.
    pub fn put_instance(&mut self, record: &InstanceRecordV1) -> Result<u64, StoreError> {
        record
            .validate()
            .map_err(|e| StoreError::Internal(e))?;
        let key = instance_key(&record.subject_id, &record.instance_id);
        let value = encode_json("instance_record_v1", record)?;
        self.upsert(INSTANCE_TABLE, &key, value)
    }

    pub fn get_instance(
        &mut self,
        subject_id: &str,
        instance_id: &str,
    ) -> Result<Option<InstanceRecordV1>, StoreError> {
        let key = instance_key(subject_id, instance_id);
        match self.store.get(&self.tenant_id, INSTANCE_TABLE, &key)? {
            None => Ok(None),
            Some(row) => {
                self.remember(INSTANCE_TABLE, &key, row.version);
                Ok(Some(decode_json("instance_record_v1", &row.value)?))
            }
        }
    }

    pub fn put_join(&mut self, join: &ChildJoinV1) -> Result<u64, StoreError> {
        join.validate().map_err(|e| StoreError::Internal(e))?;
        // subject is recovered from parent instance when available; join key uses parent scope.
        let subject = self
            .get_instance_subject_for_parent(&join.parent_instance_id)?
            .unwrap_or_else(|| "unknown".into());
        let key = join_key(&subject, &join.join_id);
        let value = encode_json("child_join_v1", join)?;
        self.upsert(JOIN_TABLE, &key, value)
    }

    pub fn put_join_for_subject(
        &mut self,
        subject_id: &str,
        join: &ChildJoinV1,
    ) -> Result<u64, StoreError> {
        join.validate().map_err(|e| StoreError::Internal(e))?;
        let key = join_key(subject_id, &join.join_id);
        let value = encode_json("child_join_v1", join)?;
        self.upsert(JOIN_TABLE, &key, value)
    }

    fn get_instance_subject_for_parent(
        &mut self,
        parent_instance_id: &str,
    ) -> Result<Option<String>, StoreError> {
        // Scan is expensive; prefer caller using put_join_for_subject.
        // Best-effort: try common lookup via scan is not available without subject.
        let _ = parent_instance_id;
        Ok(None)
    }

    pub fn get_join(
        &mut self,
        subject_id: &str,
        join_id: &str,
    ) -> Result<Option<ChildJoinV1>, StoreError> {
        let key = join_key(subject_id, join_id);
        match self.store.get(&self.tenant_id, JOIN_TABLE, &key)? {
            None => Ok(None),
            Some(row) => {
                self.remember(JOIN_TABLE, &key, row.version);
                Ok(Some(decode_json("child_join_v1", &row.value)?))
            }
        }
    }

    pub fn put_wait(
        &mut self,
        subject_id: &str,
        instance_id: &str,
        pred: &WaitPredicate,
    ) -> Result<u64, StoreError> {
        let key = format!("{subject_id}/{instance_id}");
        let value = encode_json("wait_predicate_v1", pred)?;
        self.upsert(WAIT_TABLE, &key, value)
    }

    pub fn put_budget(
        &mut self,
        subject_id: &str,
        instance_id: &str,
        budget: &InstanceBudgetV1,
    ) -> Result<u64, StoreError> {
        let key = format!("{subject_id}/{instance_id}");
        let value = encode_json("instance_budget_v1", budget)?;
        self.upsert(BUDGET_TABLE, &key, value)
    }

    /// Persist every instance, join, wait, and budget in a live [`ProcessTree`].
    pub fn persist_tree(&mut self, tree: &ProcessTree) -> Result<usize, StoreError> {
        let mut n = 0;
        for rec in tree.instances_snapshot() {
            self.put_instance(&rec)?;
            n += 1;
        }
        for join in tree.joins_snapshot() {
            self.put_join_for_subject(&tree.subject_id, &join)?;
            n += 1;
        }
        for (instance_id, pred) in tree.waits_snapshot() {
            self.put_wait(&tree.subject_id, &instance_id, &pred)?;
            n += 1;
        }
        for (instance_id, budget) in tree.budgets_snapshot() {
            self.put_budget(&tree.subject_id, &instance_id, &budget)?;
            n += 1;
        }
        Ok(n)
    }

    /// Load all instances/joins/waits/budgets for a subject into a new tree (restart recovery).
    pub fn hydrate_tree(
        &mut self,
        subject_id: &str,
    ) -> Result<ProcessTree, StoreError> {
        let mut tree = ProcessTree::new(&self.tenant_id, subject_id);
        let inst_prefix = format!("{subject_id}/");
        let inst_rows = self.store.scan_prefix(
            &self.tenant_id,
            INSTANCE_TABLE,
            &inst_prefix,
            4_096,
        )?;
        for (key, row) in inst_rows {
            self.remember(INSTANCE_TABLE, &key, row.version);
            let rec: InstanceRecordV1 = decode_json("instance_record_v1", &row.value)?;
            tree.restore_instance(rec)
                .map_err(|e| StoreError::Internal(e.detail))?;
        }
        let join_rows =
            self.store
                .scan_prefix(&self.tenant_id, JOIN_TABLE, &inst_prefix, 4_096)?;
        for (key, row) in join_rows {
            self.remember(JOIN_TABLE, &key, row.version);
            let join: ChildJoinV1 = decode_json("child_join_v1", &row.value)?;
            tree.restore_join(join)
                .map_err(|e| StoreError::Internal(e.detail))?;
        }
        let wait_rows =
            self.store
                .scan_prefix(&self.tenant_id, WAIT_TABLE, &inst_prefix, 4_096)?;
        for (key, row) in wait_rows {
            self.remember(WAIT_TABLE, &key, row.version);
            let pred: WaitPredicate = decode_json("wait_predicate_v1", &row.value)?;
            // key = subject/instance_id
            let instance_id = key
                .split_once('/')
                .map(|(_, id)| id)
                .unwrap_or(key.as_str());
            tree.restore_wait(instance_id, pred)
                .map_err(|e| StoreError::Internal(e.detail))?;
        }
        let budget_rows =
            self.store
                .scan_prefix(&self.tenant_id, BUDGET_TABLE, &inst_prefix, 4_096)?;
        for (key, row) in budget_rows {
            self.remember(BUDGET_TABLE, &key, row.version);
            let budget: InstanceBudgetV1 = decode_json("instance_budget_v1", &row.value)?;
            let instance_id = key
                .split_once('/')
                .map(|(_, id)| id)
                .unwrap_or(key.as_str());
            tree.restore_budget(instance_id, budget)
                .map_err(|e| StoreError::Internal(e.detail))?;
        }
        // Restore next_seq past any restored ids (best-effort monotonic).
        tree.reseed_seq_after_hydrate();
        Ok(tree)
    }

    fn upsert(&mut self, table: &str, key: &str, value: SolValue) -> Result<u64, StoreError> {
        if let Some(expected) = self.known_version(table, key) {
            let new_v = self
                .store
                .cas(&self.tenant_id, table, key, expected, value)?;
            self.remember(table, key, new_v);
            return Ok(new_v);
        }
        // First write path: try put_if_absent; if exists, fetch and CAS.
        match self
            .store
            .put_if_absent(&self.tenant_id, table, key, value.clone())?
        {
            PutIfAbsent::Inserted { version } => {
                self.remember(table, key, version);
                Ok(version)
            }
            PutIfAbsent::Existing(existing) => {
                let new_v = self.store.cas(
                    &self.tenant_id,
                    table,
                    key,
                    existing.version,
                    value,
                )?;
                self.remember(table, key, new_v);
                Ok(new_v)
            }
        }
    }
}

/// Helper for tests/tools: put then get round-trip.
pub fn round_trip_instance(
    store: &mut dyn Store,
    tenant: &str,
    record: &InstanceRecordV1,
) -> Result<InstanceRecordV1, StoreError> {
    let mut repo = ProcessRepository::new(store, tenant);
    repo.put_instance(record)?;
    repo.get_instance(&record.subject_id, &record.instance_id)?
        .ok_or(StoreError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os_contract::{
        InstanceStateV1, JoinPolicyV1, ResultEnvelopeV1, VersionPinV1,
    };
    use crate::process_tree::ProcessTree;
    use aelio_store::MemoryStore;
    use serde_json::json;

    fn pin(id: &str) -> VersionPinV1 {
        VersionPinV1 {
            id: id.into(),
            version: "1.0.0".into(),
            artifact_hash: None,
        }
    }

    #[test]
    fn persist_and_hydrate_tree_survives_restart() {
        let mut store = MemoryStore::new();
        let mut tree = ProcessTree::new("tenant-a", "user-1");
        let root = tree.spawn_root(pin("workflow.average"), Some("corr-1".into())).unwrap();
        let (join_id, kids) = tree
            .spawn_many_all(
                &root,
                vec![
                    ("sum".into(), pin("math.sum")),
                    ("count".into(), pin("collection.count")),
                ],
            )
            .unwrap();
        tree.complete_ok(&kids[0], json!(12), None, None).unwrap();
        tree.complete_ok(&kids[1], json!(3), None, None).unwrap();
        tree.complete_ok(&root, json!(4), None, Some("hash-avg".into()))
            .unwrap();

        {
            let mut repo = ProcessRepository::new(&mut store, "tenant-a");
            let n = repo.persist_tree(&tree).unwrap();
            assert_eq!(n, 4); // 3 instances + 1 join
        }

        // Fresh repository = process restart.
        let mut repo2 = ProcessRepository::new(&mut store, "tenant-a");
        let hydrated = repo2.hydrate_tree("user-1").unwrap();
        assert_eq!(hydrated.instance_count(), 3);
        assert_eq!(
            hydrated.get(&root).unwrap().state,
            InstanceStateV1::Completed
        );
        assert_eq!(
            hydrated.get(&root).unwrap().bag_hash.as_deref(),
            Some("hash-avg")
        );
        assert_eq!(
            hydrated.get(&kids[0]).unwrap().parent_instance_id.as_deref(),
            Some(root.as_str())
        );
        let join = hydrated.join(&join_id).unwrap();
        assert!(matches!(join.policy, JoinPolicyV1::All));
        assert!(join.terminal.is_some());
        match join.terminal.as_ref().unwrap() {
            ResultEnvelopeV1::Ok { output, .. } => {
                let arr = output.as_array().unwrap();
                assert_eq!(arr[0]["nid"], "sum");
                assert_eq!(arr[0]["output"], 12);
            }
            other => panic!("unexpected terminal {other:?}"),
        }
    }

    #[test]
    fn persist_and_hydrate_waits_and_budgets() {
        use crate::process_tree::{InstanceBudgetV1, WaitPredicate};
        use crate::os_contract::InstanceStateV1;

        let mut store = MemoryStore::new();
        let mut tree = ProcessTree::new("tenant-w", "user-w");
        let parent = tree.spawn_root(pin("parent"), None).unwrap();
        tree.set_budget(
            &parent,
            InstanceBudgetV1 {
                calls_remaining: Some(10),
                steps_remaining: Some(20),
                wall_ms_remaining: Some(5000),
                depth_remaining: Some(4),
                fanout_remaining: Some(4),
            },
        )
        .unwrap();
        let child = tree.spawn_child(&parent, "ask", pin("wait_for_user")).unwrap();
        tree.park(
            &child,
            WaitPredicate::UserEvent {
                event_key: "user.reply".into(),
            },
            "cont-1",
        )
        .unwrap();
        tree.park(
            &parent,
            WaitPredicate::ChildTerminal {
                child_id: child.clone(),
            },
            "cont-p",
        )
        .unwrap();

        {
            let mut repo = ProcessRepository::new(&mut store, "tenant-w");
            let n = repo.persist_tree(&tree).unwrap();
            // 2 instances + 2 waits + 2 budgets (parent + subdivided child)
            assert!(n >= 6, "persisted {n} rows");
        }

        let mut repo2 = ProcessRepository::new(&mut store, "tenant-w");
        let hydrated = repo2.hydrate_tree("user-w").unwrap();
        assert_eq!(hydrated.get(&child).unwrap().state, InstanceStateV1::Waiting);
        assert_eq!(hydrated.get(&parent).unwrap().state, InstanceStateV1::Waiting);
        assert!(matches!(
            hydrated.wait_of(&child),
            Some(WaitPredicate::UserEvent { event_key }) if event_key == "user.reply"
        ));
        assert!(matches!(
            hydrated.wait_of(&parent),
            Some(WaitPredicate::ChildTerminal { child_id }) if child_id == &child
        ));
        assert_eq!(
            hydrated.budget_of(&parent).and_then(|b| b.calls_remaining),
            Some(10)
        );
        // Child got subdivided budget.
        assert!(hydrated.budget_of(&child).is_some());
    }

    #[test]
    fn cas_conflict_on_stale_version() {
        let mut store = MemoryStore::new();
        let mut tree = ProcessTree::new("t", "u");
        let root = tree.spawn_root(pin("p"), None).unwrap();
        let rec = tree.get(&root).unwrap().clone();

        {
            let mut repo_a = ProcessRepository::new(&mut store, "t");
            repo_a.put_instance(&rec).unwrap();
            let mut rec2 = rec.clone();
            rec2.state = InstanceStateV1::Completed;
            rec2.result = Some(ResultEnvelopeV1::Ok {
                output: json!({}),
                output_imprint: None,
            });
            // version becomes 2
            repo_a.put_instance(&rec2).unwrap();
        }

        // Second writer with stale version cache (thinks version is still 1).
        let mut repo_b = ProcessRepository::new(&mut store, "t");
        repo_b.remember(INSTANCE_TABLE, &instance_key("u", &root), 1);
        let mut rec3 = rec.clone();
        rec3.state = InstanceStateV1::Failed;
        rec3.result = Some(ResultEnvelopeV1::Err {
            code: "x".into(),
            message: "y".into(),
            detail: None,
        });
        let err = repo_b.put_instance(&rec3).unwrap_err();
        assert_eq!(err, StoreError::Conflict);
    }
}
