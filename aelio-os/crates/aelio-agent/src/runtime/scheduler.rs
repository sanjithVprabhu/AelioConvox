//! Durable scheduled jobs with compare-and-swap leasing.

use serde::{Deserialize, Serialize};

use crate::storage::{
    AelioStore, CompareSwap, LogicalTable, PutIfAbsent, RecordEnvelope, StoredRecord,
};
use crate::{AelioError, AelioResult, ReasonCode};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Scheduled,
    Leased,
    RetryWaiting,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobTerminalOutcome {
    Completed,
    RetryExhausted { reason: String },
    Cancelled { reason: String },
    ReconciliationRequired { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledJob {
    pub id: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub state: JobState,
    pub scheduled_at_ms: i64,
    pub owner: Option<String>,
    pub lease_expires_at_ms: Option<i64>,
    pub attempts: u32,
    pub max_attempts: u32,
    pub last_error: Option<String>,
    pub terminal: Option<JobTerminalOutcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobLease {
    pub row_id: u64,
    pub version: u64,
    pub job: ScheduledJob,
}

#[derive(Clone)]
pub struct DurableScheduler {
    tenant_id: String,
    store: AelioStore,
}

impl DurableScheduler {
    pub fn new(tenant_id: impl Into<String>, store: AelioStore) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            store,
        }
    }

    pub fn schedule(&mut self, job: ScheduledJob, now_ms: i64) -> AelioResult<PutIfAbsent> {
        if job.id.trim().is_empty() || job.max_attempts == 0 {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "job id and positive max_attempts are required",
            ));
        }
        let status = status(&job.state);
        self.store.put_if_absent(
            &self.tenant_id,
            LogicalTable::Jobs,
            &RecordEnvelope {
                key: job.id.clone(),
                kind: job.kind.clone(),
                status: status.into(),
                owner: String::new(),
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
                expires_at_ms: job.lease_expires_at_ms,
                value: job,
            },
            None,
        )
    }

    /// Discover due or expired jobs and atomically claim them. Contending workers may discover
    /// the same row, but only one matching version can acquire it.
    pub fn lease_due(
        &mut self,
        owner: &str,
        now_ms: i64,
        lease_ms: i64,
        limit: usize,
    ) -> AelioResult<Vec<JobLease>> {
        self.lease_due_kind(owner, now_ms, lease_ms, limit, None)
    }

    pub fn lease_due_kind(
        &mut self,
        owner: &str,
        now_ms: i64,
        lease_ms: i64,
        limit: usize,
        kind: Option<&str>,
    ) -> AelioResult<Vec<JobLease>> {
        if owner.trim().is_empty() || lease_ms <= 0 || limit == 0 || limit > 256 {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "lease owner, positive duration, and a limit from 1 through 256 are required",
            ));
        }
        let rows: Vec<StoredRecord<ScheduledJob>> = self.store.list(
            &self.tenant_id,
            LogicalTable::Jobs,
            None,
            limit.saturating_mul(16).max(256),
        )?;
        let mut leased = Vec::new();
        for row in rows {
            if leased.len() == limit {
                break;
            }
            if kind.is_some_and(|expected| row.envelope.value.kind != expected) {
                continue;
            }
            let due = matches!(
                row.envelope.value.state,
                JobState::Scheduled | JobState::RetryWaiting
            ) && row.envelope.value.scheduled_at_ms <= now_ms;
            let expired = row.envelope.value.state == JobState::Leased
                && row
                    .envelope
                    .value
                    .lease_expires_at_ms
                    .is_some_and(|expiry| expiry <= now_ms);
            if !due && !expired {
                continue;
            }
            let mut job = row.envelope.value.clone();
            if job.attempts >= job.max_attempts {
                self.terminalize_exhausted(&row, now_ms)?;
                continue;
            }
            job.state = JobState::Leased;
            job.owner = Some(owner.into());
            job.lease_expires_at_ms = Some(now_ms.saturating_add(lease_ms));
            job.attempts += 1;
            let next = job_envelope(&row.envelope, job.clone(), now_ms);
            if let CompareSwap::Updated { version } = self.store.compare_swap(
                &self.tenant_id,
                LogicalTable::Jobs,
                row.row_id,
                row.version,
                &next,
                None,
            )? {
                leased.push(JobLease {
                    row_id: row.row_id,
                    version,
                    job,
                });
            }
        }
        Ok(leased)
    }

    pub fn complete(&mut self, lease: &JobLease, owner: &str, now_ms: i64) -> AelioResult<()> {
        self.finish(lease, owner, now_ms, JobTerminalOutcome::Completed)
    }

    pub fn complete_owned(&mut self, job_id: &str, owner: &str, now_ms: i64) -> AelioResult<()> {
        let lease = self.active_lease(job_id)?;
        self.complete(&lease, owner, now_ms)
    }

    pub fn fail(
        &mut self,
        lease: &JobLease,
        owner: &str,
        now_ms: i64,
        retry_at_ms: i64,
        reason: impl Into<String>,
    ) -> AelioResult<JobState> {
        require_owner(lease, owner, now_ms)?;
        let reason = reason.into();
        let mut job = lease.job.clone();
        job.owner = None;
        job.lease_expires_at_ms = None;
        job.last_error = Some(reason.clone());
        if job.attempts >= job.max_attempts {
            job.state = JobState::Terminal;
            job.terminal = Some(JobTerminalOutcome::RetryExhausted { reason });
        } else {
            job.state = JobState::RetryWaiting;
            job.scheduled_at_ms = retry_at_ms.max(now_ms);
        }
        let state = job.state.clone();
        self.cas_lease(lease, job, now_ms)?;
        Ok(state)
    }

    pub fn fail_owned(
        &mut self,
        job_id: &str,
        owner: &str,
        now_ms: i64,
        retry_at_ms: i64,
        reason: impl Into<String>,
    ) -> AelioResult<JobState> {
        let lease = self.active_lease(job_id)?;
        self.fail(&lease, owner, now_ms, retry_at_ms, reason)
    }

    pub fn cancel(
        &mut self,
        lease: &JobLease,
        owner: &str,
        now_ms: i64,
        reason: impl Into<String>,
    ) -> AelioResult<()> {
        self.finish(
            lease,
            owner,
            now_ms,
            JobTerminalOutcome::Cancelled {
                reason: reason.into(),
            },
        )
    }

    fn finish(
        &mut self,
        lease: &JobLease,
        owner: &str,
        now_ms: i64,
        outcome: JobTerminalOutcome,
    ) -> AelioResult<()> {
        require_owner(lease, owner, now_ms)?;
        let mut job = lease.job.clone();
        job.state = JobState::Terminal;
        job.owner = None;
        job.lease_expires_at_ms = None;
        job.terminal = Some(outcome);
        self.cas_lease(lease, job, now_ms)
    }

    fn active_lease(&self, job_id: &str) -> AelioResult<JobLease> {
        if job_id.trim().is_empty() {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "job id is required",
            ));
        }
        let row: StoredRecord<ScheduledJob> = self
            .store
            .get(&self.tenant_id, LogicalTable::Jobs, job_id)?
            .ok_or_else(|| AelioError::new(ReasonCode::NotFound, "leased job not found"))?;
        Ok(JobLease {
            row_id: row.row_id,
            version: row.version,
            job: row.envelope.value,
        })
    }

    fn cas_lease(&mut self, lease: &JobLease, job: ScheduledJob, now_ms: i64) -> AelioResult<()> {
        let current: StoredRecord<ScheduledJob> = self
            .store
            .get(&self.tenant_id, LogicalTable::Jobs, &job.id)?
            .ok_or_else(|| AelioError::new(ReasonCode::NotFound, "leased job disappeared"))?;
        let next = job_envelope(&current.envelope, job, now_ms);
        match self.store.compare_swap(
            &self.tenant_id,
            LogicalTable::Jobs,
            lease.row_id,
            lease.version,
            &next,
            None,
        )? {
            CompareSwap::Updated { .. } => Ok(()),
            CompareSwap::Conflict { .. } | CompareSwap::NotFound => {
                Err(AelioError::new(ReasonCode::Conflict, "job lease was lost"))
            }
        }
    }

    fn terminalize_exhausted(
        &mut self,
        row: &StoredRecord<ScheduledJob>,
        now_ms: i64,
    ) -> AelioResult<()> {
        let mut job = row.envelope.value.clone();
        job.state = JobState::Terminal;
        job.owner = None;
        job.lease_expires_at_ms = None;
        job.terminal = Some(JobTerminalOutcome::RetryExhausted {
            reason: job
                .last_error
                .clone()
                .unwrap_or_else(|| "attempt budget exhausted".into()),
        });
        let next = job_envelope(&row.envelope, job, now_ms);
        let _ = self.store.compare_swap(
            &self.tenant_id,
            LogicalTable::Jobs,
            row.row_id,
            row.version,
            &next,
            None,
        )?;
        Ok(())
    }
}

fn require_owner(lease: &JobLease, owner: &str, now_ms: i64) -> AelioResult<()> {
    if lease.job.owner.as_deref() != Some(owner)
        || lease
            .job
            .lease_expires_at_ms
            .is_none_or(|expiry| expiry <= now_ms)
    {
        return Err(AelioError::new(
            ReasonCode::Conflict,
            "worker does not own an unexpired job lease",
        ));
    }
    Ok(())
}

fn job_envelope(
    previous: &RecordEnvelope<ScheduledJob>,
    job: ScheduledJob,
    now_ms: i64,
) -> RecordEnvelope<ScheduledJob> {
    RecordEnvelope {
        key: previous.key.clone(),
        kind: previous.kind.clone(),
        status: status(&job.state).into(),
        owner: job.owner.clone().unwrap_or_default(),
        created_at_ms: previous.created_at_ms,
        updated_at_ms: now_ms,
        expires_at_ms: job.lease_expires_at_ms,
        value: job,
    }
}

fn status(state: &JobState) -> &'static str {
    match state {
        JobState::Scheduled => "scheduled",
        JobState::Leased => "leased",
        JobState::RetryWaiting => "retry_waiting",
        JobState::Terminal => "terminal",
    }
}
