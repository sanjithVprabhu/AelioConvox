//! Crash reconciliation for processing turns and durable side-effect intents.

use serde::{Deserialize, Serialize};

use crate::runtime::durable::DurableTurnValue;
use crate::storage::{
    AelioStore, CompareSwap, LogicalTable, PutIfAbsent, RecordEnvelope, StoredRecord,
};
use crate::{AelioError, AelioResult, ReasonCode};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentState {
    Pending,
    RetryReady,
    Succeeded,
    Failed,
    ManualReview,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SideEffectIntent {
    pub id: String,
    pub turn_id: String,
    pub tool_id: String,
    pub tool_version: String,
    pub idempotency_key: Option<String>,
    pub idempotent: bool,
    pub effectful: bool,
    pub dry_run: bool,
    pub state: IntentState,
    pub attempts: u32,
    pub max_attempts: u32,
    pub terminal_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome {
    Success,
    Error,
    UnknownAfterCrash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DurableCallRecord {
    pub call_id: String,
    pub intent_id: String,
    pub tool_id: String,
    pub tool_version: String,
    pub started_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub outcome: CallOutcome,
    pub reason_code: Option<ReasonCode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReconcileOutcome {
    IntentConfirmed { intent_id: String },
    IdempotentRetryReady { intent_id: String },
    NonIdempotentManualReview { intent_id: String },
    IntentRetryExhausted { intent_id: String },
    TurnCompleted { turn_id: String },
    TurnAwaitingSafeRetry { turn_id: String },
    TurnManualReview { turn_id: String },
}

#[derive(Clone)]
pub struct SagaReconciler {
    tenant_id: String,
    store: AelioStore,
}

impl SagaReconciler {
    pub fn new(tenant_id: impl Into<String>, store: AelioStore) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            store,
        }
    }

    pub fn record_intent(
        &mut self,
        intent: SideEffectIntent,
        now_ms: i64,
    ) -> AelioResult<PutIfAbsent> {
        if intent.id.trim().is_empty()
            || intent.turn_id.trim().is_empty()
            || intent.max_attempts == 0
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "intent id, turn id, and positive max_attempts are required",
            ));
        }
        self.store.put_if_absent(
            &self.tenant_id,
            LogicalTable::EffectIntents,
            &RecordEnvelope {
                key: intent.id.clone(),
                kind: "side_effect_intent".into(),
                status: intent_status(&intent.state).into(),
                owner: intent.turn_id.clone(),
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
                expires_at_ms: None,
                value: intent,
            },
            None,
        )
    }

    pub fn record_call(
        &mut self,
        call: DurableCallRecord,
        now_ms: i64,
    ) -> AelioResult<PutIfAbsent> {
        self.store.put_if_absent(
            &self.tenant_id,
            LogicalTable::CallRecords,
            &RecordEnvelope {
                key: call.call_id.clone(),
                kind: "tool_call".into(),
                status: match call.outcome {
                    CallOutcome::Success => "success",
                    CallOutcome::Error => "error",
                    CallOutcome::UnknownAfterCrash => "unknown",
                }
                .into(),
                owner: call.intent_id.clone(),
                created_at_ms: now_ms,
                updated_at_ms: call.completed_at_ms.unwrap_or(now_ms),
                expires_at_ms: None,
                value: call,
            },
            None,
        )
    }

    /// Reconcile a bounded batch. This method changes durable state but never invokes a tool.
    pub fn reconcile(&mut self, now_ms: i64, limit: usize) -> AelioResult<Vec<ReconcileOutcome>> {
        let mut outcomes = self.reconcile_intents(now_ms, limit)?;
        if outcomes.len() < limit {
            outcomes.extend(self.reconcile_turns(now_ms, limit - outcomes.len())?);
        }
        Ok(outcomes)
    }

    fn reconcile_intents(
        &mut self,
        now_ms: i64,
        limit: usize,
    ) -> AelioResult<Vec<ReconcileOutcome>> {
        let rows: Vec<StoredRecord<SideEffectIntent>> = self.store.list(
            &self.tenant_id,
            LogicalTable::EffectIntents,
            Some("pending"),
            limit,
        )?;
        let calls: Vec<StoredRecord<DurableCallRecord>> = self.store.list_kind(
            &self.tenant_id,
            LogicalTable::CallRecords,
            "tool_call",
            None,
            limit.saturating_mul(4),
        )?;
        let mut outcomes = Vec::new();
        for row in rows {
            let mut intent = row.envelope.value.clone();
            let receipt = calls
                .iter()
                .filter(|call| call.envelope.value.intent_id == intent.id)
                .max_by_key(|call| call.envelope.value.started_at_ms);
            let outcome = if let Some(call) = receipt {
                match call.envelope.value.outcome {
                    CallOutcome::Success => {
                        intent.state = IntentState::Succeeded;
                        ReconcileOutcome::IntentConfirmed {
                            intent_id: intent.id.clone(),
                        }
                    }
                    CallOutcome::Error => {
                        intent.state = IntentState::Failed;
                        intent.terminal_reason = Some("tool call returned an error".into());
                        ReconcileOutcome::IntentRetryExhausted {
                            intent_id: intent.id.clone(),
                        }
                    }
                    CallOutcome::UnknownAfterCrash => classify_unknown(&mut intent),
                }
            } else {
                classify_unknown(&mut intent)
            };
            let next = intent_envelope(&row.envelope, intent, now_ms);
            if matches!(
                self.store.compare_swap(
                    &self.tenant_id,
                    LogicalTable::EffectIntents,
                    row.row_id,
                    row.version,
                    &next,
                    None,
                )?,
                CompareSwap::Updated { .. }
            ) {
                outcomes.push(outcome);
            }
        }
        Ok(outcomes)
    }

    fn reconcile_turns(&mut self, now_ms: i64, limit: usize) -> AelioResult<Vec<ReconcileOutcome>> {
        let turns: Vec<StoredRecord<DurableTurnValue>> = self.store.list(
            &self.tenant_id,
            LogicalTable::Turns,
            Some("processing"),
            limit,
        )?;
        let intents: Vec<StoredRecord<SideEffectIntent>> = self.store.list(
            &self.tenant_id,
            LogicalTable::EffectIntents,
            None,
            limit.saturating_mul(8),
        )?;
        let mut outcomes = Vec::new();
        for row in turns {
            let related: Vec<_> = intents
                .iter()
                .filter(|intent| intent.envelope.value.turn_id == row.envelope.key)
                .collect();
            let (status, outcome) = if related
                .iter()
                .any(|intent| intent.envelope.value.state == IntentState::ManualReview)
            {
                (
                    "manual_review",
                    ReconcileOutcome::TurnManualReview {
                        turn_id: row.envelope.key.clone(),
                    },
                )
            } else if related
                .iter()
                .any(|intent| intent.envelope.value.state == IntentState::RetryReady)
                || related.is_empty()
            {
                (
                    "retry_ready",
                    ReconcileOutcome::TurnAwaitingSafeRetry {
                        turn_id: row.envelope.key.clone(),
                    },
                )
            } else if related
                .iter()
                .all(|intent| intent.envelope.value.state == IntentState::Succeeded)
            {
                (
                    "effects_reconciled",
                    ReconcileOutcome::TurnCompleted {
                        turn_id: row.envelope.key.clone(),
                    },
                )
            } else {
                (
                    "manual_review",
                    ReconcileOutcome::TurnManualReview {
                        turn_id: row.envelope.key.clone(),
                    },
                )
            };
            let mut next = row.envelope.clone();
            next.status = status.into();
            next.updated_at_ms = now_ms;
            if matches!(
                self.store.compare_swap(
                    &self.tenant_id,
                    LogicalTable::Turns,
                    row.row_id,
                    row.version,
                    &next,
                    None,
                )?,
                CompareSwap::Updated { .. }
            ) {
                outcomes.push(outcome);
            }
        }
        Ok(outcomes)
    }
}

fn classify_unknown(intent: &mut SideEffectIntent) -> ReconcileOutcome {
    if intent.idempotent
        && intent.idempotency_key.is_some()
        && intent.attempts < intent.max_attempts
    {
        intent.state = IntentState::RetryReady;
        ReconcileOutcome::IdempotentRetryReady {
            intent_id: intent.id.clone(),
        }
    } else if intent.idempotent && intent.attempts >= intent.max_attempts {
        intent.state = IntentState::Failed;
        intent.terminal_reason = Some("reconciliation retry budget exhausted".into());
        ReconcileOutcome::IntentRetryExhausted {
            intent_id: intent.id.clone(),
        }
    } else {
        intent.state = IntentState::ManualReview;
        intent.terminal_reason =
            Some("non-idempotent effect has no conclusive durable receipt".into());
        ReconcileOutcome::NonIdempotentManualReview {
            intent_id: intent.id.clone(),
        }
    }
}

fn intent_envelope(
    previous: &RecordEnvelope<SideEffectIntent>,
    intent: SideEffectIntent,
    now_ms: i64,
) -> RecordEnvelope<SideEffectIntent> {
    RecordEnvelope {
        key: previous.key.clone(),
        kind: previous.kind.clone(),
        status: intent_status(&intent.state).into(),
        owner: previous.owner.clone(),
        created_at_ms: previous.created_at_ms,
        updated_at_ms: now_ms,
        expires_at_ms: None,
        value: intent,
    }
}

fn intent_status(state: &IntentState) -> &'static str {
    match state {
        IntentState::Pending => "pending",
        IntentState::RetryReady => "retry_ready",
        IntentState::Succeeded => "succeeded",
        IntentState::Failed => "failed",
        IntentState::ManualReview => "manual_review",
    }
}
