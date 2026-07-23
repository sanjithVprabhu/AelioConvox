//! Deterministic proactive evaluation before any model invocation.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::runtime::{DurableScheduler, JobState, ScheduledJob};
use crate::storage::{
    AelioStore, CompareSwap, LogicalTable, PutIfAbsent, RecordEnvelope, StoredRecord,
};
use crate::{AelioError, AelioResult, ReasonCode};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProactivePolicy {
    pub min_cadence_ms: i64,
    pub max_enqueues_per_day: u32,
    pub suppression_ms: i64,
    pub max_job_attempts: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProactiveCandidate {
    pub user_id: String,
    pub fingerprint: String,
    pub payload: serde_json::Value,
    pub opted_in: bool,
    pub deterministic_gate: bool,
    pub confidence_millis: u16,
    pub min_confidence_millis: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProactiveDecision {
    Enqueued { job_id: String },
    AlreadyEnqueued { job_id: String },
    Suppressed { reason: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProactiveState {
    last_evaluated_at_ms: Option<i64>,
    last_enqueued_at_ms: Option<i64>,
    day_number: i64,
    daily_enqueues: u32,
    suppressed_until_ms: Option<i64>,
}

#[derive(Clone)]
pub struct ProactiveLoop {
    tenant_id: String,
    store: AelioStore,
}

impl ProactiveLoop {
    pub fn new(tenant_id: impl Into<String>, store: AelioStore) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            store,
        }
    }

    pub fn evaluate_and_enqueue(
        &mut self,
        candidate: ProactiveCandidate,
        policy: &ProactivePolicy,
        now_ms: i64,
    ) -> AelioResult<ProactiveDecision> {
        validate(&candidate, policy)?;
        let current: Option<StoredRecord<ProactiveState>> = self.store.get(
            &self.tenant_id,
            LogicalTable::ProactiveRuns,
            &candidate.user_id,
        )?;
        let mut state = current
            .as_ref()
            .map(|record| record.envelope.value.clone())
            .unwrap_or_default();
        state.last_evaluated_at_ms = Some(now_ms);
        let day = now_ms.div_euclid(86_400_000);
        if state.day_number != day {
            state.day_number = day;
            state.daily_enqueues = 0;
        }

        let suppression = gate_reason(&candidate, policy, &state, now_ms);
        if let Some(reason) = suppression {
            self.persist_state(current, &candidate.user_id, state, now_ms)?;
            return Ok(ProactiveDecision::Suppressed {
                reason: reason.into(),
            });
        }

        let job_id = idempotency_key(
            &self.tenant_id,
            &candidate.user_id,
            &candidate.fingerprint,
            now_ms.div_euclid(policy.min_cadence_ms),
        );
        // Reserve cadence and daily budget before publishing the job. This is deliberately
        // fail-closed: a crash can consume one send allowance, but concurrent workers can never
        // publish two jobs from the same stale counter and exceed the tenant's outbound cap.
        state.daily_enqueues = state.daily_enqueues.saturating_add(1);
        state.last_enqueued_at_ms = Some(now_ms);
        state.suppressed_until_ms = Some(now_ms.saturating_add(policy.suppression_ms));
        self.persist_state(current, &candidate.user_id, state, now_ms)?;

        let mut scheduler = DurableScheduler::new(&self.tenant_id, self.store.clone());
        let result = scheduler.schedule(
            ScheduledJob {
                id: job_id.clone(),
                kind: "proactive_delivery".into(),
                payload: candidate.payload,
                state: JobState::Scheduled,
                scheduled_at_ms: now_ms,
                owner: None,
                lease_expires_at_ms: None,
                attempts: 0,
                max_attempts: policy.max_job_attempts,
                last_error: None,
                terminal: None,
            },
            now_ms,
        )?;
        Ok(match result {
            PutIfAbsent::Inserted { .. } => ProactiveDecision::Enqueued { job_id },
            PutIfAbsent::Existing { .. } => ProactiveDecision::AlreadyEnqueued { job_id },
        })
    }

    fn persist_state(
        &mut self,
        current: Option<StoredRecord<ProactiveState>>,
        user_id: &str,
        state: ProactiveState,
        now_ms: i64,
    ) -> AelioResult<()> {
        let key = current
            .as_ref()
            .map_or_else(|| user_id.to_owned(), |record| record.envelope.key.clone());
        let envelope = RecordEnvelope {
            key,
            kind: "proactive_state".into(),
            status: "active".into(),
            owner: "runtime".into(),
            created_at_ms: current
                .as_ref()
                .map_or(now_ms, |record| record.envelope.created_at_ms),
            updated_at_ms: now_ms,
            expires_at_ms: None,
            value: state,
        };
        if let Some(current) = current {
            match self.store.compare_swap(
                &self.tenant_id,
                LogicalTable::ProactiveRuns,
                current.row_id,
                current.version,
                &envelope,
                None,
            )? {
                CompareSwap::Updated { .. } => Ok(()),
                CompareSwap::Conflict { .. } | CompareSwap::NotFound => Err(AelioError::new(
                    ReasonCode::Conflict,
                    "concurrent proactive evaluation",
                )),
            }
        } else {
            match self.store.put_if_absent(
                &self.tenant_id,
                LogicalTable::ProactiveRuns,
                &envelope,
                None,
            )? {
                PutIfAbsent::Inserted { .. } => Ok(()),
                PutIfAbsent::Existing { .. } => Err(AelioError::new(
                    ReasonCode::Conflict,
                    "concurrent proactive state creation",
                )),
            }
        }
    }
}

fn gate_reason(
    candidate: &ProactiveCandidate,
    policy: &ProactivePolicy,
    state: &ProactiveState,
    now_ms: i64,
) -> Option<&'static str> {
    if !candidate.opted_in {
        return Some("not_opted_in");
    }
    if !candidate.deterministic_gate {
        return Some("deterministic_gate_failed");
    }
    if candidate.confidence_millis < candidate.min_confidence_millis {
        return Some("confidence_below_threshold");
    }
    if state
        .suppressed_until_ms
        .is_some_and(|until| now_ms < until)
    {
        return Some("suppression_window");
    }
    if state
        .last_enqueued_at_ms
        .is_some_and(|last| now_ms.saturating_sub(last) < policy.min_cadence_ms)
    {
        return Some("cadence");
    }
    if state.daily_enqueues >= policy.max_enqueues_per_day {
        return Some("daily_budget");
    }
    None
}

fn validate(candidate: &ProactiveCandidate, policy: &ProactivePolicy) -> AelioResult<()> {
    if candidate.user_id.trim().is_empty()
        || candidate.fingerprint.trim().is_empty()
        || policy.min_cadence_ms <= 0
        || policy.max_enqueues_per_day == 0
        || policy.suppression_ms < 0
        || policy.max_job_attempts == 0
        || candidate.confidence_millis > 1_000
        || candidate.min_confidence_millis > 1_000
    {
        return Err(AelioError::new(
            ReasonCode::Validation,
            "invalid proactive candidate or policy",
        ));
    }
    Ok(())
}

fn idempotency_key(tenant: &str, user: &str, fingerprint: &str, cadence_bucket: i64) -> String {
    let digest =
        Sha256::digest(format!("{tenant}\0{user}\0{fingerprint}\0{cadence_bucket}").as_bytes());
    format!("proactive:{}", hex::encode(&digest[..16]))
}
