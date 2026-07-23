//! Durable learning cold loop driven only by behavioral outcomes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::abilities::learn::attribute;
use crate::abilities::registry::{
    ProcedureEvidence, ProcedureProvenance, ProcedureSpec, ProcedureStatus, SituationFilter,
};
use crate::contract::{AbilityContract, AbilityPath};
use crate::storage::{
    AelioStore, CompareSwap, LogicalTable, PutIfAbsent, RecordEnvelope, StoredRecord,
};
use crate::tenant::FlowSpec;
use crate::types::TenantMode;
use crate::{AelioError, AelioResult, ReasonCode};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BehavioralSignalKind {
    TurnCompleted,
    ToolSuccess,
    ToolError,
    UserRepair,
    FlowTerminal,
    Abandonment,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehavioralOutcomeSignal {
    pub id: String,
    pub turn_id: String,
    pub proposal_id: Option<String>,
    pub kind: BehavioralSignalKind,
    pub step_ids: Vec<String>,
    pub failed_step: Option<String>,
    pub latency_ms: u64,
    pub token_cost: u64,
    pub call_cost_microunits: u64,
    pub observed_at_ms: i64,
}

impl BehavioralOutcomeSignal {
    pub fn score(&self) -> f64 {
        match self.kind {
            BehavioralSignalKind::TurnCompleted => 0.75,
            BehavioralSignalKind::ToolSuccess => 0.75,
            BehavioralSignalKind::FlowTerminal => 1.0,
            BehavioralSignalKind::ToolError => 0.0,
            BehavioralSignalKind::UserRepair => -1.0,
            BehavioralSignalKind::Abandonment => -0.75,
        }
    }

    pub fn successful(&self) -> bool {
        matches!(
            self.kind,
            BehavioralSignalKind::TurnCompleted
                | BehavioralSignalKind::ToolSuccess
                | BehavioralSignalKind::FlowTerminal
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColdProposalState {
    Accumulating,
    Eligible,
    Promoted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencySnapshot {
    pub tool_versions: BTreeMap<String, String>,
    pub prompt_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalEvidence {
    pub observations: u64,
    pub successes: u64,
    pub score_sum: f64,
    pub latency_sum_ms: u64,
    pub token_cost_sum: u64,
    pub call_cost_sum_microunits: u64,
    pub step_credit_sum: BTreeMap<String, f64>,
}

impl Default for ProposalEvidence {
    fn default() -> Self {
        Self {
            observations: 0,
            successes: 0,
            score_sum: 0.0,
            latency_sum_ms: 0,
            token_cost_sum: 0,
            call_cost_sum_microunits: 0,
            step_credit_sum: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColdProposal {
    pub id: String,
    pub situation_hash: String,
    pub situation_filter: SituationFilter,
    pub path: AbilityPath,
    pub contract: AbilityContract,
    pub effectful: bool,
    pub dependencies: DependencySnapshot,
    pub evidence: ProposalEvidence,
    pub state: ColdProposalState,
    pub promoted_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantApproval {
    pub proposal_id: String,
    pub approved_by: String,
    pub approved_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionGate {
    pub min_observations: u64,
    pub min_success_rate: f64,
    pub min_mean_score: f64,
}

impl Default for PromotionGate {
    fn default() -> Self {
        Self {
            min_observations: 5,
            min_success_rate: 0.8,
            min_mean_score: 0.6,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotedProcedureVersion {
    pub procedure_id: String,
    pub version: String,
    pub situation_hash: String,
    pub path: AbilityPath,
    pub effectful: bool,
    pub dependencies: DependencySnapshot,
    pub evidence: ProposalEvidence,
    pub promoted_at_ms: i64,
    pub approved_by: Option<String>,
    /// Complete executable specification loaded into the tier registry after restart.
    pub spec: ProcedureSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyInvalidation {
    pub procedure_version: String,
    pub reason: String,
    pub invalidated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionResult {
    Promoted { version: String },
    AlreadyPromoted { version: String },
}

#[derive(Debug, Clone)]
pub struct ExplorationBudget {
    pub max_trials: u32,
    pub used_trials: u32,
}

/// Operator-owned live exploration envelope. Disabled is the safe default; enabling it requires
/// both deterministic sampling and a durable per-window cap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplorationPolicy {
    pub enabled: bool,
    /// Basis points in [0, 10_000]. Selection is a stable hash of tenant + turn, never RNG.
    pub sample_rate_bps: u16,
    pub max_trials_per_window: u32,
    pub window_ms: i64,
    pub max_candidate_steps: usize,
}

impl Default for ExplorationPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            sample_rate_bps: 0,
            max_trials_per_window: 0,
            window_ms: 3_600_000,
            max_candidate_steps: 16,
        }
    }
}

impl ExplorationPolicy {
    pub fn validate(&self) -> AelioResult<()> {
        if self.sample_rate_bps > 10_000
            || self.window_ms <= 0
            || self.max_candidate_steps == 0
            || (self.enabled && (self.sample_rate_bps == 0 || self.max_trials_per_window == 0))
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "exploration requires a valid sample rate, durable window cap, and step bound",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplorationWindow {
    pub start_ms: i64,
    pub window_ms: i64,
    pub used_trials: u32,
    pub max_trials: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplorationComparison {
    pub turn_id: String,
    pub primary_procedure: String,
    pub candidate_procedure: String,
    pub primary_success: bool,
    pub candidate_success: bool,
    pub candidate_evidence_hash: Option<String>,
    pub candidate_latency_ms: u64,
    pub invoked_tools: Vec<String>,
    pub failure_code: Option<ReasonCode>,
    pub observed_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowCandidateState {
    PendingReview,
    Approved,
    Activating,
    Active,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowCandidate {
    pub id: String,
    pub version: String,
    pub flow: FlowSpec,
    pub source_procedure_versions: Vec<String>,
    pub state: FlowCandidateState,
    pub proposed_at_ms: i64,
    pub reviewed_at_ms: Option<i64>,
    pub reviewed_by: Option<String>,
    pub review_reason: Option<String>,
}

#[derive(Clone)]
pub struct LearningColdLoop {
    tenant_id: String,
    mode: TenantMode,
    store: AelioStore,
    embedder: std::sync::Arc<dyn crate::embedding::Embedder>,
}

impl LearningColdLoop {
    pub fn new(tenant_id: impl Into<String>, mode: TenantMode, store: AelioStore) -> Self {
        let dimension = store.embedding_dimension();
        Self {
            tenant_id: tenant_id.into(),
            mode,
            store,
            embedder: std::sync::Arc::new(
                crate::embedding::HashEmbedder::new(dimension)
                    .expect("persisted embedding dimension is positive"),
            ),
        }
    }

    pub fn with_embedder(
        tenant_id: impl Into<String>,
        mode: TenantMode,
        store: AelioStore,
        embedder: std::sync::Arc<dyn crate::embedding::Embedder>,
    ) -> AelioResult<Self> {
        if embedder.dimension() != store.embedding_dimension() {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "cold-loop embedder dimension does not match the tenant store",
            ));
        }
        Ok(Self {
            tenant_id: tenant_id.into(),
            mode,
            store,
            embedder,
        })
    }

    pub fn submit_proposal(
        &mut self,
        proposal: ColdProposal,
        now_ms: i64,
    ) -> AelioResult<PutIfAbsent> {
        if proposal.id.trim().is_empty() || proposal.path.steps.is_empty() {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "proposal id and non-empty path are required",
            ));
        }
        self.store.put_if_absent(
            &self.tenant_id,
            LogicalTable::Proposals,
            &proposal_envelope(proposal, now_ms, now_ms),
            None,
        )
    }

    pub fn record_signal(&mut self, signal: BehavioralOutcomeSignal) -> AelioResult<PutIfAbsent> {
        let now_ms = signal.observed_at_ms;
        let inserted = self.store.put_if_absent(
            &self.tenant_id,
            LogicalTable::OutcomeSignals,
            &RecordEnvelope {
                key: signal.id.clone(),
                kind: "behavioral_outcome".into(),
                status: "recorded".into(),
                owner: signal.turn_id.clone(),
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
                expires_at_ms: None,
                value: signal.clone(),
            },
            None,
        )?;
        if matches!(inserted, PutIfAbsent::Inserted { .. }) {
            if let Some(proposal_id) = &signal.proposal_id {
                self.accumulate(proposal_id, &signal)?;
            }
        }
        Ok(inserted)
    }

    pub fn approve(
        &mut self,
        proposal_id: &str,
        approved_by: &str,
        now_ms: i64,
    ) -> AelioResult<PutIfAbsent> {
        if approved_by.trim().is_empty() {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "approval identity is required",
            ));
        }
        let proposal: StoredRecord<ColdProposal> = self
            .store
            .get(&self.tenant_id, LogicalTable::Proposals, proposal_id)?
            .ok_or_else(|| AelioError::new(ReasonCode::NotFound, "proposal not found"))?;
        if proposal.envelope.value.state == ColdProposalState::Rejected {
            return Err(AelioError::new(
                ReasonCode::Conflict,
                "rejected proposal cannot be approved",
            ));
        }
        let approval = TenantApproval {
            proposal_id: proposal_id.into(),
            approved_by: approved_by.into(),
            approved_at_ms: now_ms,
        };
        self.store.put_if_absent(
            &self.tenant_id,
            LogicalTable::Approvals,
            &RecordEnvelope {
                key: proposal_id.into(),
                kind: "effectful_promotion_approval".into(),
                status: "approved".into(),
                owner: approved_by.into(),
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
                expires_at_ms: None,
                value: approval,
            },
            None,
        )
    }

    pub fn promote(
        &mut self,
        proposal_id: &str,
        gate: &PromotionGate,
        now_ms: i64,
    ) -> AelioResult<PromotionResult> {
        if gate.min_observations == 0
            || !(0.0..=1.0).contains(&gate.min_success_rate)
            || !(-1.0..=1.0).contains(&gate.min_mean_score)
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "promotion gate observations and rates are outside safe bounds",
            ));
        }
        let row: StoredRecord<ColdProposal> = self
            .store
            .get(&self.tenant_id, LogicalTable::Proposals, proposal_id)?
            .ok_or_else(|| AelioError::new(ReasonCode::NotFound, "proposal not found"))?;
        let previous_version = row.envelope.value.promoted_version.clone();
        if let Some(version) = previous_version.as_ref() {
            self.finalize_procedure(proposal_id, version, now_ms)?;
            let previous: StoredRecord<PromotedProcedureVersion> = self
                .store
                .get(
                    &self.tenant_id,
                    LogicalTable::Procedures,
                    &format!("{proposal_id}@{version}"),
                )?
                .ok_or_else(|| {
                    AelioError::new(
                        ReasonCode::Conflict,
                        "promoted proposal has no immutable procedure version",
                    )
                })?;
            if row.envelope.value.evidence.observations
                < previous
                    .envelope
                    .value
                    .evidence
                    .observations
                    .saturating_add(gate.min_observations)
            {
                return Ok(PromotionResult::AlreadyPromoted {
                    version: version.clone(),
                });
            }
        }
        enforce_gate(&row.envelope.value, gate)?;
        let approval: Option<StoredRecord<TenantApproval>> =
            self.store
                .get(&self.tenant_id, LogicalTable::Approvals, proposal_id)?;
        if row.envelope.value.effectful && approval.is_none() {
            return Err(AelioError::new(
                ReasonCode::GateNotMet,
                "effectful promotion requires explicit tenant approval",
            ));
        }
        let version = deterministic_version(&row.envelope.value);
        let approved_by = approval.map(|record| record.envelope.value.approved_by);
        let success_rate = row.envelope.value.evidence.successes as f64
            / row.envelope.value.evidence.observations.max(1) as f64;
        let mean_cost = row.envelope.value.evidence.call_cost_sum_microunits as f64
            / row.envelope.value.evidence.observations.max(1) as f64;
        let mean_latency_ms = row.envelope.value.evidence.latency_sum_ms as f64
            / row.envelope.value.evidence.observations.max(1) as f64;
        let mut contract = row.envelope.value.contract.clone();
        contract.id = format!("{proposal_id}@{version}");
        contract.prompt_hash = Some(row.envelope.value.dependencies.prompt_hash.clone());
        let situation_filter = row.envelope.value.situation_filter.clone();
        let situation_embedding = crate::abilities::learn::embed_situation_filter(
            self.embedder.as_ref(),
            &situation_filter,
        )
        .unwrap_or_default();
        let spec = ProcedureSpec {
            id: proposal_id.into(),
            version: version.clone(),
            tenant_id: self.tenant_id.clone(),
            situation_hash: row.envelope.value.situation_hash.clone(),
            situation_filter,
            situation_embedding,
            path: row.envelope.value.path.clone(),
            contract,
            tool_deps: row
                .envelope
                .value
                .dependencies
                .tool_versions
                .keys()
                .cloned()
                .collect(),
            prompt_deps: vec![row.envelope.value.dependencies.prompt_hash.clone()],
            evidence: ProcedureEvidence {
                observations: row.envelope.value.evidence.observations,
                success_rate,
                mean_cost,
                mean_latency_ms,
            },
            status: ProcedureStatus::Promoted,
            provenance: ProcedureProvenance {
                origin: "durable_cold_loop".into(),
                proposed_by: "cold_loop".into(),
                approved_by: approved_by.clone(),
            },
            supersedes: previous_version.clone(),
        };
        let promoted = PromotedProcedureVersion {
            procedure_id: proposal_id.into(),
            version: version.clone(),
            situation_hash: row.envelope.value.situation_hash.clone(),
            path: row.envelope.value.path.clone(),
            effectful: row.envelope.value.effectful,
            dependencies: row.envelope.value.dependencies.clone(),
            evidence: row.envelope.value.evidence.clone(),
            promoted_at_ms: now_ms,
            approved_by,
            spec,
        };
        let immutable_key = format!("{proposal_id}@{version}");
        let _ = self.store.put_if_absent(
            &self.tenant_id,
            LogicalTable::Procedures,
            &RecordEnvelope {
                key: immutable_key,
                kind: "promoted_procedure_version".into(),
                status: "staging".into(),
                owner: proposal_id.into(),
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
                expires_at_ms: None,
                value: promoted,
            },
            None,
        )?;

        let mut proposal = row.envelope.value;
        proposal.state = ColdProposalState::Promoted;
        proposal.promoted_version = Some(version.clone());
        let next = proposal_envelope(proposal, row.envelope.created_at_ms, now_ms);
        match self.store.compare_swap(
            &self.tenant_id,
            LogicalTable::Proposals,
            row.row_id,
            row.version,
            &next,
            None,
        )? {
            CompareSwap::Updated { .. } => {
                self.finalize_procedure(proposal_id, &version, now_ms)?;
                Ok(PromotionResult::Promoted { version })
            }
            CompareSwap::Conflict { .. } => {
                let winner: StoredRecord<ColdProposal> = self
                    .store
                    .get(&self.tenant_id, LogicalTable::Proposals, proposal_id)?
                    .ok_or_else(|| AelioError::new(ReasonCode::Conflict, "promotion race lost"))?;
                let version =
                    winner.envelope.value.promoted_version.ok_or_else(|| {
                        AelioError::new(ReasonCode::Conflict, "promotion race lost")
                    })?;
                self.finalize_procedure(proposal_id, &version, now_ms)?;
                Ok(PromotionResult::AlreadyPromoted { version })
            }
            CompareSwap::NotFound => Err(AelioError::new(
                ReasonCode::Conflict,
                "proposal disappeared during promotion",
            )),
        }
    }

    fn finalize_procedure(
        &mut self,
        proposal_id: &str,
        version: &str,
        now_ms: i64,
    ) -> AelioResult<()> {
        let key = format!("{proposal_id}@{version}");
        let row: StoredRecord<PromotedProcedureVersion> = self
            .store
            .get(&self.tenant_id, LogicalTable::Procedures, &key)?
            .ok_or_else(|| {
                AelioError::new(
                    ReasonCode::Conflict,
                    "promoted proposal has no immutable procedure version",
                )
            })?;
        if row.envelope.status == "promoted" {
            return Ok(());
        }
        let mut next = row.envelope.clone();
        next.status = "promoted".into();
        next.updated_at_ms = now_ms;
        match self.store.compare_swap(
            &self.tenant_id,
            LogicalTable::Procedures,
            row.row_id,
            row.version,
            &next,
            None,
        )? {
            CompareSwap::Updated { .. } => Ok(()),
            CompareSwap::Conflict { .. } => {
                let current: StoredRecord<PromotedProcedureVersion> = self
                    .store
                    .get(&self.tenant_id, LogicalTable::Procedures, &key)?
                    .ok_or_else(|| {
                        AelioError::new(ReasonCode::Conflict, "procedure finalization race lost")
                    })?;
                if current.envelope.status == "promoted" {
                    Ok(())
                } else {
                    Err(AelioError::new(
                        ReasonCode::Conflict,
                        "procedure finalization race lost",
                    ))
                }
            }
            CompareSwap::NotFound => Err(AelioError::new(
                ReasonCode::Conflict,
                "procedure disappeared during finalization",
            )),
        }
    }

    /// Exploration is always bounded and can never perform an effect unless it is a dry run.
    pub fn claim_exploration(
        &self,
        budget: &mut ExplorationBudget,
        effectful: bool,
        dry_run: bool,
    ) -> AelioResult<()> {
        if budget.used_trials >= budget.max_trials {
            return Err(AelioError::new(
                ReasonCode::BudgetExceeded,
                "exploration budget exhausted",
            ));
        }
        if effectful && !dry_run {
            return Err(AelioError::new(
                ReasonCode::PolicyDenied,
                "exploration is restricted to read-only or dry-run execution",
            ));
        }
        if matches!(self.mode, TenantMode::Live) && effectful {
            return Err(AelioError::new(
                ReasonCode::PolicyDenied,
                "live mode does not explore effectful paths",
            ));
        }
        budget.used_trials += 1;
        Ok(())
    }

    /// Deterministically sample and durably claim one live shadow trial. Concurrent workers share
    /// the same CAS-protected window, so the operator cap cannot be exceeded by a race.
    pub fn claim_live_exploration(
        &mut self,
        turn_id: &str,
        policy: &ExplorationPolicy,
        now_ms: i64,
    ) -> AelioResult<bool> {
        policy.validate()?;
        if !policy.enabled {
            return Ok(false);
        }
        let digest = Sha256::digest(format!("{}\u{1f}{turn_id}", self.tenant_id).as_bytes());
        let bucket = u16::from_le_bytes([digest[0], digest[1]]) % 10_000;
        if bucket >= policy.sample_rate_bps {
            return Ok(false);
        }
        let start_ms = now_ms.div_euclid(policy.window_ms) * policy.window_ms;
        let key = format!("window:{start_ms}");
        for _ in 0..8 {
            let current: Option<StoredRecord<ExplorationWindow>> =
                self.store
                    .get(&self.tenant_id, LogicalTable::Explorations, &key)?;
            if let Some(row) = current {
                let effective_max = row
                    .envelope
                    .value
                    .max_trials
                    .min(policy.max_trials_per_window);
                if row.envelope.value.used_trials >= effective_max {
                    return Ok(false);
                }
                let mut next = row.envelope;
                next.value.used_trials = next.value.used_trials.saturating_add(1);
                next.value.max_trials = effective_max;
                next.updated_at_ms = now_ms;
                match self.store.compare_swap(
                    &self.tenant_id,
                    LogicalTable::Explorations,
                    row.row_id,
                    row.version,
                    &next,
                    None,
                )? {
                    CompareSwap::Updated { .. } => return Ok(true),
                    CompareSwap::Conflict { .. } | CompareSwap::NotFound => continue,
                }
            } else {
                let window = ExplorationWindow {
                    start_ms,
                    window_ms: policy.window_ms,
                    used_trials: 1,
                    max_trials: policy.max_trials_per_window,
                };
                match self.store.put_if_absent(
                    &self.tenant_id,
                    LogicalTable::Explorations,
                    &RecordEnvelope {
                        key: key.clone(),
                        kind: "exploration_window".into(),
                        status: "active".into(),
                        owner: "operator_budget".into(),
                        created_at_ms: now_ms,
                        updated_at_ms: now_ms,
                        expires_at_ms: Some(start_ms.saturating_add(policy.window_ms)),
                        value: window,
                    },
                    None,
                )? {
                    PutIfAbsent::Inserted { .. } => return Ok(true),
                    PutIfAbsent::Existing { .. } => continue,
                }
            }
        }
        Err(AelioError::new(
            ReasonCode::Conflict,
            "exploration budget CAS contention exceeded",
        ))
    }

    /// Select the strongest valid immutable alternative for the exact situation. The primary is
    /// excluded, as are effectful, invalidated, oversized, and unfinalized versions.
    pub fn runner_up(
        &self,
        situation_hash: &str,
        primary_procedure_id: &str,
        max_steps: usize,
    ) -> AelioResult<Option<PromotedProcedureVersion>> {
        let procedures: Vec<StoredRecord<PromotedProcedureVersion>> = self.store.list(
            &self.tenant_id,
            LogicalTable::Procedures,
            Some("promoted"),
            10_001,
        )?;
        if procedures.len() > 10_000 {
            return Err(AelioError::new(
                ReasonCode::BudgetExceeded,
                "procedure catalog exceeds bounded runner-up scan",
            ));
        }
        let mut candidates = Vec::new();
        for row in procedures {
            let value = row.envelope.value;
            if value.situation_hash != situation_hash
                || value.procedure_id == primary_procedure_id
                || value.effectful
                || value.path.steps.is_empty()
                || value.path.steps.len() > max_steps
                || !self.is_valid(&row.envelope.key)?
            {
                continue;
            }
            candidates.push(value);
        }
        candidates.sort_by(|left, right| {
            procedure_mean_score(&right.evidence)
                .total_cmp(&procedure_mean_score(&left.evidence))
                .then(right.evidence.observations.cmp(&left.evidence.observations))
                .then(left.procedure_id.cmp(&right.procedure_id))
        });
        Ok(candidates.into_iter().next())
    }

    pub fn record_exploration_comparison(
        &mut self,
        comparison: ExplorationComparison,
    ) -> AelioResult<PutIfAbsent> {
        self.store.put_if_absent(
            &self.tenant_id,
            LogicalTable::Explorations,
            &RecordEnvelope {
                key: format!(
                    "comparison:{}:{}",
                    comparison.turn_id, comparison.candidate_procedure
                ),
                kind: "exploration_comparison".into(),
                status: "recorded".into(),
                owner: comparison.turn_id.clone(),
                created_at_ms: comparison.observed_at_ms,
                updated_at_ms: comparison.observed_at_ms,
                expires_at_ms: None,
                value: comparison,
            },
            None,
        )
    }

    /// Create a review artifact only. The learned sequence is frozen (`learnable=false`) and is
    /// never added to the active catalog by this operation.
    pub fn propose_candidate_flow(
        &mut self,
        mut flow: FlowSpec,
        mut source_procedure_versions: Vec<String>,
        protected_flow_ids: &[String],
        now_ms: i64,
    ) -> AelioResult<PutIfAbsent> {
        validate_flow_candidate_shape(&flow)?;
        if protected_flow_ids.iter().any(|id| id == &flow.id) {
            return Err(AelioError::new(
                ReasonCode::PolicyDenied,
                "learning cannot replace a protected authored flow",
            ));
        }
        source_procedure_versions.sort();
        source_procedure_versions.dedup();
        if source_procedure_versions.is_empty() || source_procedure_versions.len() > 32 {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "candidate flow requires 1..=32 immutable source procedures",
            ));
        }
        let mut allowed = std::collections::BTreeSet::new();
        for key in &source_procedure_versions {
            let row: StoredRecord<PromotedProcedureVersion> = self
                .store
                .get(&self.tenant_id, LogicalTable::Procedures, key)?
                .ok_or_else(|| {
                    AelioError::new(ReasonCode::NotFound, "source procedure was not found")
                })?;
            if row.envelope.status != "promoted" || !self.is_valid(key)? {
                return Err(AelioError::new(
                    ReasonCode::GateNotMet,
                    "candidate flow source is not a valid promoted procedure",
                ));
            }
            let evidence = &row.envelope.value.evidence;
            if evidence.observations < PromotionGate::default().min_observations
                || procedure_success_rate(evidence) < PromotionGate::default().min_success_rate
            {
                return Err(AelioError::new(
                    ReasonCode::GateNotMet,
                    "candidate flow source has insufficient stable evidence",
                ));
            }
            allowed.extend(
                row.envelope
                    .value
                    .path
                    .steps
                    .iter()
                    .map(|step| step.ability_id.clone()),
            );
        }
        if flow
            .steps
            .iter()
            .flat_map(|step| &step.admissible)
            .any(|identifier| !allowed.contains(identifier))
        {
            return Err(AelioError::new(
                ReasonCode::Denied,
                "candidate flow references an ability outside its source procedures",
            ));
        }
        flow.learnable = false;
        let bytes = serde_json::to_vec(&(flow.clone(), &source_procedure_versions))
            .map_err(|error| AelioError::new(ReasonCode::Validation, error.to_string()))?;
        let version = hex::encode(&Sha256::digest(bytes)[..12]);
        let candidate = FlowCandidate {
            id: flow.id.clone(),
            version: version.clone(),
            flow,
            source_procedure_versions,
            state: FlowCandidateState::PendingReview,
            proposed_at_ms: now_ms,
            reviewed_at_ms: None,
            reviewed_by: None,
            review_reason: None,
        };
        self.store.put_if_absent(
            &self.tenant_id,
            LogicalTable::FlowCandidates,
            &RecordEnvelope {
                key: format!("{}@{version}", candidate.id),
                kind: "candidate_flow".into(),
                status: "pending_review".into(),
                owner: "learning_cold_loop".into(),
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
                expires_at_ms: None,
                value: candidate,
            },
            None,
        )
    }

    pub fn review_candidate_flow(
        &mut self,
        key: &str,
        approved: bool,
        reviewed_by: &str,
        reason: Option<String>,
        now_ms: i64,
    ) -> AelioResult<()> {
        if reviewed_by.trim().is_empty() {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "candidate flow review identity is required",
            ));
        }
        for _ in 0..8 {
            let row: StoredRecord<FlowCandidate> = self
                .store
                .get(&self.tenant_id, LogicalTable::FlowCandidates, key)?
                .ok_or_else(|| AelioError::new(ReasonCode::NotFound, "candidate flow not found"))?;
            if row.envelope.value.state != FlowCandidateState::PendingReview {
                return Err(AelioError::new(
                    ReasonCode::Conflict,
                    "candidate flow was already reviewed",
                ));
            }
            let mut next = row.envelope;
            next.value.state = if approved {
                FlowCandidateState::Approved
            } else {
                FlowCandidateState::Rejected
            };
            next.value.reviewed_at_ms = Some(now_ms);
            next.value.reviewed_by = Some(reviewed_by.into());
            next.value.review_reason = reason.clone();
            next.status = if approved { "approved" } else { "rejected" }.into();
            next.owner = reviewed_by.into();
            next.updated_at_ms = now_ms;
            match self.store.compare_swap(
                &self.tenant_id,
                LogicalTable::FlowCandidates,
                row.row_id,
                row.version,
                &next,
                None,
            )? {
                CompareSwap::Updated { .. } => return Ok(()),
                CompareSwap::Conflict { .. } => continue,
                CompareSwap::NotFound => {
                    return Err(AelioError::new(
                        ReasonCode::NotFound,
                        "candidate flow disappeared during review",
                    ));
                }
            }
        }
        Err(AelioError::new(
            ReasonCode::Conflict,
            "candidate flow review CAS contention exceeded",
        ))
    }

    pub fn may_serve_promoted(&self) -> bool {
        matches!(self.mode, TenantMode::Live)
    }

    /// Append invalidation records; promoted procedure payloads remain immutable.
    pub fn invalidate_dependencies(
        &mut self,
        current_tools: &BTreeMap<String, String>,
        registry: &crate::abilities::registry::Registry,
        now_ms: i64,
    ) -> AelioResult<Vec<DependencyInvalidation>> {
        let procedures: Vec<StoredRecord<PromotedProcedureVersion>> = self.store.list(
            &self.tenant_id,
            LogicalTable::Procedures,
            Some("promoted"),
            10_000,
        )?;
        let mut invalidations = Vec::new();
        for procedure in procedures {
            let key = procedure.envelope.key.clone();
            let reason = procedure
                .envelope
                .value
                .dependencies
                .tool_versions
                .iter()
                .find_map(|(tool, version)| {
                    (current_tools.get(tool) != Some(version))
                        .then(|| format!("tool {tool} version changed"))
                })
                .or_else(|| {
                    let current_prompt_fingerprint =
                        crate::abilities::learn::path_prompt_dependency_fingerprint(
                            registry,
                            &procedure.envelope.value.path,
                        );
                    (procedure.envelope.value.dependencies.prompt_hash
                        != current_prompt_fingerprint)
                        .then(|| "prompt dependency fingerprint changed".to_string())
                });
            let Some(reason) = reason else {
                continue;
            };
            let invalidation = DependencyInvalidation {
                procedure_version: key.clone(),
                reason,
                invalidated_at_ms: now_ms,
            };
            let inserted = self.store.put_if_absent(
                &self.tenant_id,
                LogicalTable::Audit,
                &RecordEnvelope {
                    key: format!("invalidation:{key}"),
                    kind: "dependency_invalidation".into(),
                    status: "active".into(),
                    owner: "cold_loop".into(),
                    created_at_ms: now_ms,
                    updated_at_ms: now_ms,
                    expires_at_ms: None,
                    value: invalidation.clone(),
                },
                None,
            )?;
            if matches!(inserted, PutIfAbsent::Inserted { .. }) {
                invalidations.push(invalidation);
            }
        }
        Ok(invalidations)
    }

    pub fn is_valid(&self, procedure_version: &str) -> AelioResult<bool> {
        let invalidation: Option<StoredRecord<DependencyInvalidation>> = self.store.get(
            &self.tenant_id,
            LogicalTable::Audit,
            &format!("invalidation:{procedure_version}"),
        )?;
        Ok(invalidation.is_none())
    }

    pub fn suspend_procedure(
        &mut self,
        procedure_id: &str,
        reason: &str,
        now_ms: i64,
    ) -> AelioResult<usize> {
        let procedures: Vec<StoredRecord<PromotedProcedureVersion>> = self.store.list(
            &self.tenant_id,
            LogicalTable::Procedures,
            Some("promoted"),
            10_000,
        )?;
        let mut suspended = 0;
        for procedure in procedures
            .into_iter()
            .filter(|row| row.envelope.value.procedure_id == procedure_id)
        {
            let key = procedure.envelope.key;
            let invalidation = DependencyInvalidation {
                procedure_version: key.clone(),
                reason: reason.into(),
                invalidated_at_ms: now_ms,
            };
            if matches!(
                self.store.put_if_absent(
                    &self.tenant_id,
                    LogicalTable::Audit,
                    &RecordEnvelope {
                        key: format!("invalidation:{key}"),
                        kind: "behavioral_invalidation".into(),
                        status: "active".into(),
                        owner: "cold_loop".into(),
                        created_at_ms: now_ms,
                        updated_at_ms: now_ms,
                        expires_at_ms: None,
                        value: invalidation,
                    },
                    None,
                )?,
                PutIfAbsent::Inserted { .. }
            ) {
                suspended += 1;
            }
        }
        Ok(suspended)
    }

    fn accumulate(
        &mut self,
        proposal_id: &str,
        signal: &BehavioralOutcomeSignal,
    ) -> AelioResult<()> {
        for _ in 0..8 {
            let row: StoredRecord<ColdProposal> = self
                .store
                .get(&self.tenant_id, LogicalTable::Proposals, proposal_id)?
                .ok_or_else(|| AelioError::new(ReasonCode::NotFound, "proposal not found"))?;
            let mut proposal = row.envelope.value;
            let score = signal.score();
            proposal.evidence.observations += 1;
            proposal.evidence.successes += u64::from(signal.successful());
            proposal.evidence.score_sum += score;
            proposal.evidence.latency_sum_ms = proposal
                .evidence
                .latency_sum_ms
                .saturating_add(signal.latency_ms);
            proposal.evidence.token_cost_sum = proposal
                .evidence
                .token_cost_sum
                .saturating_add(signal.token_cost);
            proposal.evidence.call_cost_sum_microunits = proposal
                .evidence
                .call_cost_sum_microunits
                .saturating_add(signal.call_cost_microunits);
            for credit in attribute(&signal.step_ids, signal.failed_step.as_deref(), score) {
                *proposal
                    .evidence
                    .step_credit_sum
                    .entry(credit.step_id)
                    .or_default() += credit.credit;
            }
            let next =
                proposal_envelope(proposal, row.envelope.created_at_ms, signal.observed_at_ms);
            match self.store.compare_swap(
                &self.tenant_id,
                LogicalTable::Proposals,
                row.row_id,
                row.version,
                &next,
                None,
            )? {
                CompareSwap::Updated { .. } => return Ok(()),
                CompareSwap::Conflict { .. } => continue,
                CompareSwap::NotFound => break,
            }
        }
        Err(AelioError::new(
            ReasonCode::Conflict,
            "proposal evidence CAS contention exceeded",
        ))
    }
}

fn enforce_gate(proposal: &ColdProposal, gate: &PromotionGate) -> AelioResult<()> {
    let evidence = &proposal.evidence;
    let success_rate = if evidence.observations == 0 {
        0.0
    } else {
        evidence.successes as f64 / evidence.observations as f64
    };
    let mean_score = if evidence.observations == 0 {
        0.0
    } else {
        evidence.score_sum / evidence.observations as f64
    };
    if evidence.observations < gate.min_observations
        || success_rate < gate.min_success_rate
        || mean_score < gate.min_mean_score
    {
        return Err(AelioError::new(
            ReasonCode::GateNotMet,
            format!(
                "promotion gate not met: observations={}, success_rate={success_rate:.3}, mean_score={mean_score:.3}",
                evidence.observations
            ),
        ));
    }
    Ok(())
}

fn procedure_success_rate(evidence: &ProposalEvidence) -> f64 {
    evidence.successes as f64 / evidence.observations.max(1) as f64
}

fn procedure_mean_score(evidence: &ProposalEvidence) -> f64 {
    evidence.score_sum / evidence.observations.max(1) as f64
}

fn validate_flow_candidate_shape(flow: &FlowSpec) -> AelioResult<()> {
    let mut step_ids = std::collections::BTreeSet::new();
    if flow.id.trim().is_empty()
        || flow.id.len() > 128
        || flow.version.trim().is_empty()
        || flow.version.len() > 128
        || flow.name.trim().is_empty()
        || flow.name.len() > 256
        || flow.steps.is_empty()
        || flow.steps.len() > 32
        || flow.max_attempts == 0
        || flow.max_attempts > 100
        || flow.terminal_states.is_empty()
        || flow.terminal_states.len() > 16
        || flow.activation.trigger_surface.is_empty()
        || flow.activation.trigger_surface.len() > 64
        || flow
            .activation
            .trigger_surface
            .iter()
            .any(|trigger| trigger.trim().is_empty() || trigger.len() > 256)
        || !(0.0..=1.0).contains(&flow.activation.margin_threshold)
        || flow
            .ttl_secs
            .is_some_and(|ttl| ttl == 0 || ttl > 31_536_000)
        || flow.steps.iter().any(|step| {
            step.id.trim().is_empty()
                || step.id.len() > 128
                || step.intent.trim().is_empty()
                || step.intent.len() > 256
                || step.admissible.is_empty()
                || step.admissible.len() > 16
                || step
                    .admissible
                    .iter()
                    .any(|item| item.trim().is_empty() || item.len() > 256)
                || !step_ids.insert(step.id.clone())
        })
    {
        return Err(AelioError::new(
            ReasonCode::Validation,
            "candidate flow shape, bounds, triggers, terminals, or step identities are invalid",
        ));
    }
    Ok(())
}

fn deterministic_version(proposal: &ColdProposal) -> String {
    let mut canonical = proposal.clone();
    canonical.state = ColdProposalState::Eligible;
    canonical.promoted_version = None;
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    hex::encode(&Sha256::digest(bytes)[..12])
}

fn proposal_envelope(
    proposal: ColdProposal,
    created_at_ms: i64,
    updated_at_ms: i64,
) -> RecordEnvelope<ColdProposal> {
    RecordEnvelope {
        key: proposal.id.clone(),
        kind: "learning_proposal".into(),
        status: match proposal.state {
            ColdProposalState::Accumulating => "accumulating",
            ColdProposalState::Eligible => "eligible",
            ColdProposalState::Promoted => "promoted",
            ColdProposalState::Rejected => "rejected",
        }
        .into(),
        owner: "cold_loop".into(),
        created_at_ms,
        updated_at_ms,
        expires_at_ms: None,
        value: proposal,
    }
}
