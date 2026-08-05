//! In-memory process tree for Conductor/Harness OS (Phase 2 spine).
//!
//! Owns [`InstanceRecordV1`] / [`ChildJoinV1`] transitions:
//! spawn → complete/fail/cancel → join evaluation.
//!
//! Durability (store/Aelio DB) is a later step; this module is the
//! authoritative transition logic those adapters must call.

use crate::error::{ErrV1, ReasonCode};
use crate::os_contract::{
    ChildJoinV1, InstanceRecordV1, InstanceStateV1, JoinPolicyV1, ResultEnvelopeV1, VersionPinV1,
};
use aelio_sol::SolValue;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

/// Errors from illegal process-tree transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessError {
    pub code: ReasonCode,
    pub detail: String,
}

impl ProcessError {
    fn shape(detail: impl Into<String>) -> Self {
        Self {
            code: ReasonCode::Shape,
            detail: detail.into(),
        }
    }
    fn missing(detail: impl Into<String>) -> Self {
        Self {
            code: ReasonCode::Missing,
            detail: detail.into(),
        }
    }
    fn policy(detail: impl Into<String>) -> Self {
        Self {
            code: ReasonCode::Policy,
            detail: detail.into(),
        }
    }

    pub fn into_errv1(self, nid: &str) -> ErrV1 {
        ErrV1::new(self.code, nid, self.detail)
    }
}

/// Why an instance is in [`InstanceStateV1::Waiting`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WaitPredicate {
    /// Parent (or peer waiter) waits until `child_id` is terminal.
    ChildTerminal { child_id: String },
    /// Wait for an external/user event key (Park-style).
    UserEvent { event_key: String },
}

/// One in-memory process tree for a single (tenant, subject) session scope.
#[derive(Debug, Default)]
pub struct ProcessTree {
    pub tenant_id: String,
    pub subject_id: String,
    instances: BTreeMap<String, InstanceRecordV1>,
    joins: BTreeMap<String, ChildJoinV1>,
    /// Optional bag snapshots keyed by instance_id (for pure-program demos).
    bags: BTreeMap<String, SolValue>,
    /// Active wait predicates for Waiting instances.
    waits: BTreeMap<String, WaitPredicate>,
    /// Per-instance residual budgets (Phase 2.8).
    budgets: BTreeMap<String, InstanceBudgetV1>,
    next_seq: u64,
}

/// Residual budget envelope for one process instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct InstanceBudgetV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls_remaining: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps_remaining: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_ms_remaining: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth_remaining: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fanout_remaining: Option<u64>,
}

impl InstanceBudgetV1 {
    pub fn charge_call(&mut self) -> Result<(), ProcessError> {
        if let Some(n) = self.calls_remaining.as_mut() {
            if *n == 0 {
                return Err(ProcessError {
                    code: ReasonCode::BudgetCalls,
                    detail: "calls budget exhausted".into(),
                });
            }
            *n -= 1;
        }
        Ok(())
    }

    pub fn charge_step(&mut self) -> Result<(), ProcessError> {
        if let Some(n) = self.steps_remaining.as_mut() {
            if *n == 0 {
                return Err(ProcessError {
                    code: ReasonCode::BudgetSize,
                    detail: "steps budget exhausted".into(),
                });
            }
            *n -= 1;
        }
        Ok(())
    }

    /// Split a sub-budget for a child (deterministic floor-half style).
    pub fn subdivide_for_child(&self) -> InstanceBudgetV1 {
        InstanceBudgetV1 {
            calls_remaining: self.calls_remaining.map(|n| n / 2),
            steps_remaining: self.steps_remaining.map(|n| n / 2),
            wall_ms_remaining: self.wall_ms_remaining.map(|n| n / 2),
            depth_remaining: self.depth_remaining.map(|n| n.saturating_sub(1)),
            fanout_remaining: self.fanout_remaining,
        }
    }
}

impl ProcessTree {
    pub fn new(tenant_id: impl Into<String>, subject_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            subject_id: subject_id.into(),
            instances: BTreeMap::new(),
            joins: BTreeMap::new(),
            bags: BTreeMap::new(),
            waits: BTreeMap::new(),
            budgets: BTreeMap::new(),
            next_seq: 1,
        }
    }

    pub fn set_budget(&mut self, instance_id: &str, budget: InstanceBudgetV1) -> Result<(), ProcessError> {
        if !self.instances.contains_key(instance_id) {
            return Err(ProcessError::missing(format!(
                "unknown instance `{instance_id}`"
            )));
        }
        self.budgets.insert(instance_id.into(), budget);
        Ok(())
    }

    pub fn budget_of(&self, instance_id: &str) -> Option<&InstanceBudgetV1> {
        self.budgets.get(instance_id)
    }

    pub fn charge_call(&mut self, instance_id: &str) -> Result<(), ProcessError> {
        if let Some(b) = self.budgets.get_mut(instance_id) {
            b.charge_call()?;
        }
        Ok(())
    }

    pub fn charge_step(&mut self, instance_id: &str) -> Result<(), ProcessError> {
        if let Some(b) = self.budgets.get_mut(instance_id) {
            b.charge_step()?;
        }
        Ok(())
    }

    pub fn budgets_snapshot(&self) -> Vec<(String, InstanceBudgetV1)> {
        self.budgets
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    pub fn restore_budget(
        &mut self,
        instance_id: &str,
        budget: InstanceBudgetV1,
    ) -> Result<(), ProcessError> {
        if !self.instances.contains_key(instance_id) {
            return Err(ProcessError::missing(format!(
                "unknown instance `{instance_id}`"
            )));
        }
        self.budgets.insert(instance_id.into(), budget);
        Ok(())
    }

    pub fn wait_of(&self, instance_id: &str) -> Option<&WaitPredicate> {
        self.waits.get(instance_id)
    }

    /// Snapshot of active waits for durable persist: (instance_id, predicate).
    pub fn waits_snapshot(&self) -> Vec<(String, WaitPredicate)> {
        self.waits
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Restore a wait predicate after hydrate (instance must already exist and be Waiting).
    pub fn restore_wait(
        &mut self,
        instance_id: &str,
        predicate: WaitPredicate,
    ) -> Result<(), ProcessError> {
        let rec = self
            .instances
            .get(instance_id)
            .ok_or_else(|| ProcessError::missing(format!("unknown instance `{instance_id}`")))?;
        if rec.state != InstanceStateV1::Waiting {
            return Err(ProcessError::policy(format!(
                "restore_wait requires Waiting state for `{instance_id}`"
            )));
        }
        self.waits.insert(instance_id.into(), predicate);
        Ok(())
    }

    /// True if instance is Cancelled / DeadLettered (no new effects allowed).
    pub fn is_cancelled(&self, instance_id: &str) -> bool {
        self.instances.get(instance_id).is_some_and(|r| {
            matches!(
                r.state,
                InstanceStateV1::Cancelled | InstanceStateV1::DeadLettered
            )
        })
    }

    /// Fail closed before effectful dispatch when the instance (or an ancestor) is cancelled.
    pub fn assert_may_dispatch(&self, instance_id: &str) -> Result<(), ProcessError> {
        let mut cur = Some(instance_id.to_string());
        while let Some(id) = cur {
            let rec = self
                .instances
                .get(&id)
                .ok_or_else(|| ProcessError::missing(format!("unknown instance `{id}`")))?;
            if matches!(
                rec.state,
                InstanceStateV1::Cancelled | InstanceStateV1::DeadLettered
            ) {
                return Err(ProcessError::policy(format!(
                    "instance `{id}` cancelled: no post-cancel dispatch"
                )));
            }
            cur = rec.parent_instance_id.clone();
        }
        Ok(())
    }

    pub fn get(&self, instance_id: &str) -> Option<&InstanceRecordV1> {
        self.instances.get(instance_id)
    }

    pub fn bag(&self, instance_id: &str) -> Option<&SolValue> {
        self.bags.get(instance_id)
    }

    pub fn join(&self, join_id: &str) -> Option<&ChildJoinV1> {
        self.joins.get(join_id)
    }

    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    pub fn join_count(&self) -> usize {
        self.joins.len()
    }

    /// Snapshot of all instance records (for durable persist).
    pub fn instances_snapshot(&self) -> Vec<InstanceRecordV1> {
        self.instances.values().cloned().collect()
    }

    /// Snapshot of all join records (for durable persist).
    pub fn joins_snapshot(&self) -> Vec<ChildJoinV1> {
        self.joins.values().cloned().collect()
    }

    /// Restore an instance from durable storage (restart recovery). Does not allocate seq.
    pub fn restore_instance(&mut self, record: InstanceRecordV1) -> Result<(), ProcessError> {
        record.validate().map_err(ProcessError::shape)?;
        if record.tenant_id != self.tenant_id {
            return Err(ProcessError::policy("tenant mismatch on restore"));
        }
        if record.subject_id != self.subject_id {
            return Err(ProcessError::policy("subject mismatch on restore"));
        }
        self.instances.insert(record.instance_id.clone(), record);
        Ok(())
    }

    /// Restore a join from durable storage.
    pub fn restore_join(&mut self, join: ChildJoinV1) -> Result<(), ProcessError> {
        join.validate().map_err(ProcessError::shape)?;
        self.joins.insert(join.join_id.clone(), join);
        Ok(())
    }

    /// After hydrate, advance `next_seq` past any numeric suffixes found in instance ids.
    pub fn reseed_seq_after_hydrate(&mut self) {
        let mut max_seq = self.next_seq;
        for id in self.instances.keys() {
            // Patterns: inst-N  or  parent:nid:N
            if let Some(n) = id
                .rsplit(['-', ':'])
                .next()
                .and_then(|s| s.parse::<u64>().ok())
            {
                max_seq = max_seq.max(n + 1);
            }
        }
        for id in self.joins.keys() {
            if let Some(n) = id
                .rsplit('-')
                .next()
                .and_then(|s| s.parse::<u64>().ok())
            {
                max_seq = max_seq.max(n + 1);
            }
        }
        self.next_seq = max_seq;
    }

    fn alloc_id(&mut self, prefix: &str) -> String {
        let n = self.next_seq;
        self.next_seq += 1;
        format!("{prefix}-{}", n)
    }

    /// Spawn a root instance (no parent). State = Running.
    pub fn spawn_root(
        &mut self,
        artifact_pin: VersionPinV1,
        correlation_id: Option<String>,
    ) -> Result<String, ProcessError> {
        artifact_pin
            .validate()
            .map_err(ProcessError::shape)?;
        let instance_id = self.alloc_id("inst");
        let record = InstanceRecordV1 {
            instance_id: instance_id.clone(),
            root_instance_id: instance_id.clone(),
            parent_instance_id: None,
            tenant_id: self.tenant_id.clone(),
            subject_id: self.subject_id.clone(),
            artifact_pin,
            state: InstanceStateV1::Running,
            child_ids: vec![],
            join_policy: JoinPolicyV1::All,
            bag_hash: None,
            continuation_pin: None,
            result: None,
            correlation_id,
            causation_id: None,
            ledger_head: None,
        };
        record.validate().map_err(ProcessError::shape)?;
        self.instances.insert(instance_id.clone(), record);
        Ok(instance_id)
    }

    /// Spawn a child under `parent_id`. Deterministic id from parent + nid + seq.
    pub fn spawn_child(
        &mut self,
        parent_id: &str,
        child_nid: &str,
        artifact_pin: VersionPinV1,
    ) -> Result<String, ProcessError> {
        if child_nid.trim().is_empty() {
            return Err(ProcessError::shape("child_nid must be non-empty"));
        }
        artifact_pin
            .validate()
            .map_err(ProcessError::shape)?;
        let parent = self
            .instances
            .get(parent_id)
            .ok_or_else(|| ProcessError::missing(format!("unknown parent `{parent_id}`")))?
            .clone();
        if matches!(
            parent.state,
            InstanceStateV1::Completed
                | InstanceStateV1::Failed
                | InstanceStateV1::Cancelled
                | InstanceStateV1::DeadLettered
        ) {
            return Err(ProcessError::policy(
                "cannot spawn child under terminal parent",
            ));
        }
        if parent.child_ids.len() >= 64 {
            return Err(ProcessError::policy("fanout exceeds 64 children"));
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        // Fanout / depth budget checks on parent.
        if let Some(pb) = self.budgets.get_mut(parent_id) {
            if let Some(f) = pb.fanout_remaining.as_mut() {
                if *f == 0 {
                    return Err(ProcessError {
                        code: ReasonCode::BudgetSize,
                        detail: "fanout budget exhausted".into(),
                    });
                }
                *f -= 1;
            }
            if pb.depth_remaining == Some(0) {
                return Err(ProcessError {
                    code: ReasonCode::BudgetSize,
                    detail: "depth budget exhausted".into(),
                });
            }
        }
        let child_budget = self.budgets.get(parent_id).map(|b| b.subdivide_for_child());
        let instance_id = format!("{parent_id}:{child_nid}:{seq}");
        let record = InstanceRecordV1 {
            instance_id: instance_id.clone(),
            root_instance_id: parent.root_instance_id.clone(),
            parent_instance_id: Some(parent_id.into()),
            tenant_id: self.tenant_id.clone(),
            subject_id: self.subject_id.clone(),
            artifact_pin,
            state: InstanceStateV1::Running,
            child_ids: vec![],
            join_policy: JoinPolicyV1::All,
            bag_hash: None,
            continuation_pin: None,
            result: None,
            correlation_id: parent.correlation_id.clone(),
            causation_id: Some(parent_id.into()),
            ledger_head: None,
        };
        record.validate().map_err(ProcessError::shape)?;
        self.instances.insert(instance_id.clone(), record);
        if let Some(b) = child_budget {
            self.budgets.insert(instance_id.clone(), b);
        }
        let parent_mut = self.instances.get_mut(parent_id).expect("parent exists");
        parent_mut.child_ids.push(instance_id.clone());
        Ok(instance_id)
    }

    /// Open a join edge for ordered children under a parent.
    pub fn open_join(
        &mut self,
        parent_id: &str,
        policy: JoinPolicyV1,
        child_nids: Vec<String>,
        child_instance_ids: Vec<String>,
    ) -> Result<String, ProcessError> {
        policy.validate().map_err(ProcessError::shape)?;
        if child_nids.len() != child_instance_ids.len() {
            return Err(ProcessError::shape(
                "child_nids and child_instance_ids length mismatch",
            ));
        }
        if child_nids.is_empty() {
            return Err(ProcessError::shape("join requires at least one child"));
        }
        if !self.instances.contains_key(parent_id) {
            return Err(ProcessError::missing(format!(
                "unknown parent `{parent_id}`"
            )));
        }
        for cid in &child_instance_ids {
            let child = self
                .instances
                .get(cid)
                .ok_or_else(|| ProcessError::missing(format!("unknown child `{cid}`")))?;
            if child.parent_instance_id.as_deref() != Some(parent_id) {
                return Err(ProcessError::policy(format!(
                    "child `{cid}` is not owned by parent `{parent_id}`"
                )));
            }
        }
        let join_id = self.alloc_id("join");
        let join = ChildJoinV1 {
            parent_instance_id: parent_id.into(),
            join_id: join_id.clone(),
            policy,
            child_nids,
            child_instance_ids,
            completed: BTreeMap::new(),
            cancelled: vec![],
            terminal: None,
        };
        join.validate().map_err(ProcessError::shape)?;
        self.joins.insert(join_id.clone(), join);
        Ok(join_id)
    }

    /// Park an instance (nested Park / wait-for-child / wait-for-user).
    ///
    /// Sets state to Waiting, stores `continuation_pin` and wait predicate.
    /// Does not complete the instance. If waiting on an already-terminal child,
    /// transitions to Ready immediately.
    pub fn park(
        &mut self,
        instance_id: &str,
        predicate: WaitPredicate,
        continuation_pin: impl Into<String>,
    ) -> Result<(), ProcessError> {
        let pin = continuation_pin.into();
        let parent_state = self
            .instances
            .get(instance_id)
            .ok_or_else(|| ProcessError::missing(format!("unknown instance `{instance_id}`")))?
            .state;
        if matches!(
            parent_state,
            InstanceStateV1::Completed
                | InstanceStateV1::Failed
                | InstanceStateV1::Cancelled
                | InstanceStateV1::DeadLettered
        ) {
            return Err(ProcessError::policy(format!(
                "cannot park terminal instance `{instance_id}`"
            )));
        }

        let mut immediate_ready = false;
        if let WaitPredicate::ChildTerminal { child_id } = &predicate {
            let child = self.instances.get(child_id).ok_or_else(|| {
                ProcessError::missing(format!("unknown child `{child_id}`"))
            })?;
            if child.parent_instance_id.as_deref() != Some(instance_id) {
                return Err(ProcessError::policy(
                    "ChildTerminal wait requires direct child ownership",
                ));
            }
            immediate_ready = matches!(
                child.state,
                InstanceStateV1::Completed
                    | InstanceStateV1::Failed
                    | InstanceStateV1::Cancelled
                    | InstanceStateV1::DeadLettered
            );
        }

        let rec = self.instances.get_mut(instance_id).unwrap();
        rec.continuation_pin = Some(pin);
        if immediate_ready {
            rec.state = InstanceStateV1::Ready;
            self.waits.remove(instance_id);
        } else {
            rec.state = InstanceStateV1::Waiting;
            self.waits.insert(instance_id.into(), predicate);
        }
        Ok(())
    }

    /// Resume a Waiting instance with a wake payload (user/event).
    ///
    /// For `UserEvent`, `event_key` must match. For `ChildTerminal`, prefer
    /// automatic wake from [`Self::complete`]; explicit resume also allowed if child is terminal.
    pub fn resume(
        &mut self,
        instance_id: &str,
        event_key: Option<&str>,
    ) -> Result<(), ProcessError> {
        let rec = self
            .instances
            .get(instance_id)
            .ok_or_else(|| ProcessError::missing(format!("unknown instance `{instance_id}`")))?
            .clone();
        if rec.state != InstanceStateV1::Waiting {
            return Err(ProcessError::policy(format!(
                "instance `{instance_id}` is not Waiting (state={:?})",
                rec.state
            )));
        }
        let pred = self
            .waits
            .get(instance_id)
            .cloned()
            .ok_or_else(|| ProcessError::missing("missing wait predicate"))?;
        match pred {
            WaitPredicate::UserEvent { event_key: expected } => {
                let got = event_key.unwrap_or("");
                if got != expected {
                    return Err(ProcessError::policy(format!(
                        "wake key mismatch: expected `{expected}`, got `{got}`"
                    )));
                }
            }
            WaitPredicate::ChildTerminal { child_id } => {
                let child = self.instances.get(&child_id).ok_or_else(|| {
                    ProcessError::missing(format!("unknown child `{child_id}`"))
                })?;
                if !matches!(
                    child.state,
                    InstanceStateV1::Completed
                        | InstanceStateV1::Failed
                        | InstanceStateV1::Cancelled
                        | InstanceStateV1::DeadLettered
                ) {
                    return Err(ProcessError::policy(
                        "cannot resume ChildTerminal wait: child still active",
                    ));
                }
            }
        }
        let rec = self.instances.get_mut(instance_id).unwrap();
        rec.state = InstanceStateV1::Running;
        self.waits.remove(instance_id);
        Ok(())
    }

    /// Mark instance completed successfully; re-evaluate open joins that include it.
    pub fn complete_ok(
        &mut self,
        instance_id: &str,
        output: serde_json::Value,
        bag: Option<SolValue>,
        bag_hash: Option<String>,
    ) -> Result<ResultEnvelopeV1, ProcessError> {
        self.complete(
            instance_id,
            ResultEnvelopeV1::Ok {
                output,
                output_imprint: None,
            },
            InstanceStateV1::Completed,
            bag,
            bag_hash,
        )
    }

    pub fn complete_err(
        &mut self,
        instance_id: &str,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<ResultEnvelopeV1, ProcessError> {
        self.complete(
            instance_id,
            ResultEnvelopeV1::Err {
                code: code.into(),
                message: message.into(),
                detail: None,
            },
            InstanceStateV1::Failed,
            None,
            None,
        )
    }

    pub fn cancel(
        &mut self,
        instance_id: &str,
        reason: impl Into<String>,
    ) -> Result<ResultEnvelopeV1, ProcessError> {
        let reason = reason.into();
        // Cancel descendants first (cooperative subtree cancel).
        let children: Vec<String> = self
            .instances
            .get(instance_id)
            .map(|r| r.child_ids.clone())
            .unwrap_or_default();
        for child in children {
            if let Some(rec) = self.instances.get(&child) {
                if !matches!(
                    rec.state,
                    InstanceStateV1::Completed
                        | InstanceStateV1::Failed
                        | InstanceStateV1::Cancelled
                        | InstanceStateV1::DeadLettered
                ) {
                    let _ = self.cancel(&child, format!("parent cancelled: {reason}"));
                }
            }
        }
        self.complete(
            instance_id,
            ResultEnvelopeV1::Cancelled { reason },
            InstanceStateV1::Cancelled,
            None,
            None,
        )
    }

    fn complete(
        &mut self,
        instance_id: &str,
        result: ResultEnvelopeV1,
        state: InstanceStateV1,
        bag: Option<SolValue>,
        bag_hash: Option<String>,
    ) -> Result<ResultEnvelopeV1, ProcessError> {
        result.validate().map_err(ProcessError::shape)?;
        let rec = self
            .instances
            .get_mut(instance_id)
            .ok_or_else(|| ProcessError::missing(format!("unknown instance `{instance_id}`")))?;
        if matches!(
            rec.state,
            InstanceStateV1::Completed
                | InstanceStateV1::Failed
                | InstanceStateV1::Cancelled
                | InstanceStateV1::DeadLettered
        ) {
            return Err(ProcessError::policy(format!(
                "instance `{instance_id}` already terminal"
            )));
        }
        rec.state = state;
        rec.result = Some(result.clone());
        rec.bag_hash = bag_hash;
        self.waits.remove(instance_id);
        if let Some(b) = bag {
            self.bags.insert(instance_id.into(), b);
        }
        // Feed open joins.
        let join_ids: Vec<String> = self
            .joins
            .iter()
            .filter(|(_, j)| j.child_instance_ids.iter().any(|c| c == instance_id))
            .map(|(id, _)| id.clone())
            .collect();
        for jid in join_ids {
            self.feed_join(&jid, instance_id, result.clone())?;
        }
        // Nested park: wake any parent Waiting on this child's terminal state.
        self.wake_child_terminal_waiters(instance_id)?;
        Ok(result)
    }

    /// When a child becomes terminal, Ready any parents parked on `ChildTerminal { child_id }`.
    fn wake_child_terminal_waiters(&mut self, child_id: &str) -> Result<(), ProcessError> {
        let waiters: Vec<String> = self
            .waits
            .iter()
            .filter_map(|(waiter, pred)| match pred {
                WaitPredicate::ChildTerminal { child_id: c } if c == child_id => {
                    Some(waiter.clone())
                }
                _ => None,
            })
            .collect();
        for waiter in waiters {
            if let Some(rec) = self.instances.get_mut(&waiter) {
                if rec.state == InstanceStateV1::Waiting {
                    rec.state = InstanceStateV1::Ready;
                }
            }
            self.waits.remove(&waiter);
        }
        Ok(())
    }

    fn feed_join(
        &mut self,
        join_id: &str,
        child_instance_id: &str,
        result: ResultEnvelopeV1,
    ) -> Result<(), ProcessError> {
        let join = self
            .joins
            .get_mut(join_id)
            .ok_or_else(|| ProcessError::missing(format!("unknown join `{join_id}`")))?;
        if join.terminal.is_some() {
            return Ok(());
        }
        // Map instance → nid for ordered result keys.
        let nid = join
            .child_instance_ids
            .iter()
            .zip(join.child_nids.iter())
            .find(|(cid, _)| *cid == child_instance_id)
            .map(|(_, nid)| nid.clone())
            .ok_or_else(|| {
                ProcessError::shape(format!(
                    "child `{child_instance_id}` not in join `{join_id}`"
                ))
            })?;
        join.completed.insert(nid, result.clone());
        if matches!(result, ResultEnvelopeV1::Cancelled { .. }) {
            join.cancelled.push(child_instance_id.into());
        }
        self.try_finalize_join(join_id)
    }

    fn try_finalize_join(&mut self, join_id: &str) -> Result<(), ProcessError> {
        let join = self
            .joins
            .get(join_id)
            .ok_or_else(|| ProcessError::missing(format!("unknown join `{join_id}`")))?
            .clone();
        if join.terminal.is_some() {
            return Ok(());
        }
        let terminal = match &join.policy {
            JoinPolicyV1::All => finalize_join_all(&join),
            JoinPolicyV1::AllSettled => finalize_join_all_settled(&join),
            JoinPolicyV1::Any => finalize_join_any(&join),
            JoinPolicyV1::Race => finalize_join_race(&join),
            JoinPolicyV1::Quorum { n } => finalize_join_quorum(&join, *n),
            JoinPolicyV1::Supervise => None, // parent continues; no auto-terminal
        };
        if let Some(term) = terminal {
            let join_mut = self.joins.get_mut(join_id).expect("join exists");
            join_mut.terminal = Some(term);
        }
        Ok(())
    }

    /// Spawn ordered children, open join=all, return join_id + child instance ids.
    pub fn spawn_many_all(
        &mut self,
        parent_id: &str,
        children: Vec<(String, VersionPinV1)>,
    ) -> Result<(String, Vec<String>), ProcessError> {
        let mut nids = Vec::new();
        let mut ids = Vec::new();
        for (nid, pin) in children {
            let id = self.spawn_child(parent_id, &nid, pin)?;
            nids.push(nid);
            ids.push(id);
        }
        let join_id = self.open_join(parent_id, JoinPolicyV1::All, nids, ids.clone())?;
        Ok((join_id, ids))
    }
}

fn finalize_join_all(join: &ChildJoinV1) -> Option<ResultEnvelopeV1> {
    if join.completed.len() < join.child_nids.len() {
        return None;
    }
    // Fail if any child failed or cancelled.
    for nid in &join.child_nids {
        match join.completed.get(nid) {
            Some(ResultEnvelopeV1::Ok { .. }) => {}
            Some(ResultEnvelopeV1::Err { code, message, .. }) => {
                return Some(ResultEnvelopeV1::Err {
                    code: code.clone(),
                    message: format!("join_all child `{nid}` failed: {message}"),
                    detail: Some(json!({"child_nid": nid})),
                });
            }
            Some(ResultEnvelopeV1::Cancelled { reason }) => {
                return Some(ResultEnvelopeV1::Err {
                    code: "Join.Cancelled".into(),
                    message: format!("join_all child `{nid}` cancelled: {reason}"),
                    detail: Some(json!({"child_nid": nid})),
                });
            }
            None => return None,
        }
    }
    // Deterministic ordered array by child_nids (JSON objects do not preserve key order).
    let mut outputs = Vec::new();
    for nid in &join.child_nids {
        if let Some(ResultEnvelopeV1::Ok { output, .. }) = join.completed.get(nid) {
            outputs.push(json!({ "nid": nid, "output": output }));
        }
    }
    Some(ResultEnvelopeV1::Ok {
        output: serde_json::Value::Array(outputs),
        output_imprint: None,
    })
}

fn finalize_join_all_settled(join: &ChildJoinV1) -> Option<ResultEnvelopeV1> {
    if join.completed.len() < join.child_nids.len() {
        return None;
    }
    let mut outputs = serde_json::Map::new();
    for nid in &join.child_nids {
        if let Some(env) = join.completed.get(nid) {
            outputs.insert(
                nid.clone(),
                serde_json::to_value(env).unwrap_or(serde_json::Value::Null),
            );
        }
    }
    Some(ResultEnvelopeV1::Ok {
        output: serde_json::Value::Object(outputs),
        output_imprint: None,
    })
}

fn finalize_join_any(join: &ChildJoinV1) -> Option<ResultEnvelopeV1> {
    for nid in &join.child_nids {
        if let Some(ResultEnvelopeV1::Ok { output, output_imprint }) = join.completed.get(nid) {
            return Some(ResultEnvelopeV1::Ok {
                output: json!({ "winner": nid, "output": output }),
                output_imprint: output_imprint.clone(),
            });
        }
    }
    if join.completed.len() >= join.child_nids.len() {
        return Some(ResultEnvelopeV1::Err {
            code: "Join.AllFailed".into(),
            message: "join_any: no successful child".into(),
            detail: None,
        });
    }
    None
}

fn finalize_join_race(join: &ChildJoinV1) -> Option<ResultEnvelopeV1> {
    // First terminal in child_nids order among completed (deterministic; true wall-race later).
    for nid in &join.child_nids {
        if let Some(env) = join.completed.get(nid) {
            return Some(match env {
                ResultEnvelopeV1::Ok { output, output_imprint } => ResultEnvelopeV1::Ok {
                    output: json!({ "winner": nid, "output": output }),
                    output_imprint: output_imprint.clone(),
                },
                other => other.clone(),
            });
        }
    }
    None
}

fn finalize_join_quorum(join: &ChildJoinV1, n: u32) -> Option<ResultEnvelopeV1> {
    let oks: Vec<_> = join
        .child_nids
        .iter()
        .filter_map(|nid| match join.completed.get(nid) {
            Some(ResultEnvelopeV1::Ok { output, .. }) => Some((nid.clone(), output.clone())),
            _ => None,
        })
        .collect();
    if oks.len() as u32 >= n {
        let mut outputs = serde_json::Map::new();
        for (nid, out) in oks.into_iter().take(n as usize) {
            outputs.insert(nid, out);
        }
        return Some(ResultEnvelopeV1::Ok {
            output: serde_json::Value::Object(outputs),
            output_imprint: None,
        });
    }
    let failed = join
        .completed
        .values()
        .filter(|e| !matches!(e, ResultEnvelopeV1::Ok { .. }))
        .count();
    let remaining = join.child_nids.len() - join.completed.len();
    if oks.len() + remaining < n as usize {
        return Some(ResultEnvelopeV1::Err {
            code: "Join.QuorumImpossible".into(),
            message: format!("quorum {n} impossible ({failed} failed, {remaining} remaining)"),
            detail: None,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(id: &str) -> VersionPinV1 {
        VersionPinV1 {
            id: id.into(),
            version: "1.0.0".into(),
            artifact_hash: None,
        }
    }

    #[test]
    fn spawn_children_join_all_ordered() {
        let mut tree = ProcessTree::new("tenant", "user-1");
        let root = tree.spawn_root(pin("workflow.average"), None).unwrap();
        let (join_id, kids) = tree
            .spawn_many_all(
                &root,
                vec![
                    ("sum".into(), pin("math.sum")),
                    ("count".into(), pin("collection.count")),
                ],
            )
            .unwrap();
        assert_eq!(kids.len(), 2);
        assert_eq!(tree.get(&root).unwrap().child_ids.len(), 2);
        assert_eq!(
            tree.get(&kids[0]).unwrap().parent_instance_id.as_deref(),
            Some(root.as_str())
        );
        assert_eq!(
            tree.get(&kids[0]).unwrap().root_instance_id,
            root
        );

        tree.complete_ok(&kids[0], json!(12), None, None).unwrap();
        assert!(tree.join(&join_id).unwrap().terminal.is_none());
        tree.complete_ok(&kids[1], json!(3), None, None).unwrap();
        let term = tree.join(&join_id).unwrap().terminal.clone().unwrap();
        match term {
            ResultEnvelopeV1::Ok { output, .. } => {
                let arr = output.as_array().expect("ordered join array");
                assert_eq!(arr.len(), 2);
                assert_eq!(arr[0]["nid"], "sum");
                assert_eq!(arr[0]["output"], 12);
                assert_eq!(arr[1]["nid"], "count");
                assert_eq!(arr[1]["output"], 3);
            }
            other => panic!("expected Ok join, got {other:?}"),
        }
    }

    #[test]
    fn join_all_fails_on_child_error() {
        let mut tree = ProcessTree::new("t", "u");
        let root = tree.spawn_root(pin("parent"), None).unwrap();
        let (join_id, kids) = tree
            .spawn_many_all(
                &root,
                vec![("a".into(), pin("a")), ("b".into(), pin("b"))],
            )
            .unwrap();
        tree.complete_ok(&kids[0], json!(1), None, None).unwrap();
        tree.complete_err(&kids[1], "Math.EmptyInput", "empty")
            .unwrap();
        match tree.join(&join_id).unwrap().terminal.as_ref().unwrap() {
            ResultEnvelopeV1::Err { code, .. } => assert_eq!(code, "Math.EmptyInput"),
            _ => panic!("expected join err"),
        }
    }

    #[test]
    fn cancel_propagates_to_children() {
        let mut tree = ProcessTree::new("t", "u");
        let root = tree.spawn_root(pin("parent"), None).unwrap();
        let child = tree.spawn_child(&root, "c1", pin("child")).unwrap();
        tree.cancel(&root, "user exit").unwrap();
        assert_eq!(tree.get(&root).unwrap().state, InstanceStateV1::Cancelled);
        assert_eq!(
            tree.get(&child).unwrap().state,
            InstanceStateV1::Cancelled
        );
    }

    #[test]
    fn cannot_spawn_under_terminal_parent() {
        let mut tree = ProcessTree::new("t", "u");
        let root = tree.spawn_root(pin("p"), None).unwrap();
        tree.complete_ok(&root, json!({}), None, None).unwrap();
        let err = tree.spawn_child(&root, "x", pin("c")).unwrap_err();
        assert_eq!(err.code, ReasonCode::Policy);
    }

    #[test]
    fn nested_park_child_wakes_parent_on_complete() {
        let mut tree = ProcessTree::new("t", "u");
        let parent = tree.spawn_root(pin("parent"), None).unwrap();
        let child = tree.spawn_child(&parent, "ask", pin("wait_for_user")).unwrap();
        tree.park(
            &child,
            WaitPredicate::UserEvent {
                event_key: "user.reply".into(),
            },
            "cont-child-1",
        )
        .unwrap();
        assert_eq!(tree.get(&child).unwrap().state, InstanceStateV1::Waiting);
        tree.park(
            &parent,
            WaitPredicate::ChildTerminal {
                child_id: child.clone(),
            },
            "cont-parent-1",
        )
        .unwrap();
        assert_eq!(tree.get(&parent).unwrap().state, InstanceStateV1::Waiting);

        // Wrong wake key refused.
        assert!(tree.resume(&child, Some("wrong")).is_err());
        // Correct wake → child Running.
        tree.resume(&child, Some("user.reply")).unwrap();
        assert_eq!(tree.get(&child).unwrap().state, InstanceStateV1::Running);
        // Parent still waiting until child terminals.
        assert_eq!(tree.get(&parent).unwrap().state, InstanceStateV1::Waiting);

        tree.complete_ok(&child, json!({"answer": "42"}), None, None)
            .unwrap();
        // Auto-wake parent.
        assert_eq!(tree.get(&parent).unwrap().state, InstanceStateV1::Ready);
        assert!(tree.wait_of(&parent).is_none());
        tree.complete_ok(&parent, json!({"done": true}), None, None)
            .unwrap();
        assert_eq!(tree.get(&parent).unwrap().state, InstanceStateV1::Completed);
    }

    #[test]
    fn cancel_blocks_dispatch_assertion() {
        let mut tree = ProcessTree::new("t", "u");
        let root = tree.spawn_root(pin("p"), None).unwrap();
        let child = tree.spawn_child(&root, "c", pin("c")).unwrap();
        tree.assert_may_dispatch(&child).unwrap();
        tree.cancel(&root, "user exit").unwrap();
        assert!(tree.is_cancelled(&root));
        assert!(tree.is_cancelled(&child));
        let err = tree.assert_may_dispatch(&child).unwrap_err();
        assert_eq!(err.code, ReasonCode::Policy);
        assert!(err.detail.contains("post-cancel"));
    }

    #[test]
    fn budget_charges_and_subdivides() {
        let mut tree = ProcessTree::new("t", "u");
        let root = tree.spawn_root(pin("p"), None).unwrap();
        tree.set_budget(
            &root,
            InstanceBudgetV1 {
                calls_remaining: Some(2),
                steps_remaining: Some(4),
                wall_ms_remaining: None,
                depth_remaining: Some(3),
                fanout_remaining: Some(2),
            },
        )
        .unwrap();
        let child = tree.spawn_child(&root, "c", pin("c")).unwrap();
        // Child inherits floor(2/2)=1 calls.
        assert_eq!(
            tree.budget_of(&child).unwrap().calls_remaining,
            Some(1)
        );
        assert_eq!(
            tree.budget_of(&root).unwrap().fanout_remaining,
            Some(1)
        );
        tree.charge_call(&root).unwrap();
        tree.charge_call(&root).unwrap();
        let err = tree.charge_call(&root).unwrap_err();
        assert_eq!(err.code, ReasonCode::BudgetCalls);
    }
}
