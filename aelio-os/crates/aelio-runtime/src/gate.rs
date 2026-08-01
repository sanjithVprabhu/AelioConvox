//! Generic evidence-backed admission gate over immutable artifacts.

use crate::artifact::HarnessBody;
use crate::sandbox::HarnessSandboxRunner;
use crate::{
    artifact_error, artifact_from_flow_push, ArtifactActor, ArtifactEvidence,
    ArtifactEvidenceDraft, ArtifactEvidenceRepository, ArtifactRecord, ArtifactRepository,
    ArtifactStatus, ArtifactTier, ArtifactTransition, ArtifactTrigger, EvidencePhase,
    EvidenceSummary, FlowPush, PromotionProposal, PromotionProposalRepository,
    PromotionProposalStatus, RuntimeError, SandboxCase, SandboxLimits, SandboxReport,
    SandboxRunner,
};
use aelio_prompt::{PromptArtifact, TemplateRegistry};
use aelio_sol::value_hash;
use aelio_store::EmbeddedStore;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize)]
pub struct GateResult {
    pub record: ArtifactRecord,
    pub report: SandboxReport,
    pub evidence: EvidenceSummary,
}

#[derive(Debug, Clone, Serialize)]
pub struct CanaryResult {
    pub record: ArtifactRecord,
    pub evidence: EvidenceSummary,
    pub promotion_proposal: Option<PromotionProposal>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCaseKind {
    #[default]
    Positive,
    Negative,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromptGateCase {
    pub slots: BTreeMap<String, Json>,
    pub expected: BTreeMap<String, Json>,
    #[serde(default)]
    pub kind: PromptCaseKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct PromptGateCaseReport {
    pub input_hash: String,
    pub prompt_hash: String,
    pub agreed: bool,
    pub detail: Option<String>,
    pub ledger_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PromptGateReport {
    pub artifact: String,
    pub cases: Vec<PromptGateCaseReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PromptGateResult {
    pub record: ArtifactRecord,
    pub report: PromptGateReport,
    pub evidence: EvidenceSummary,
}

/// Model boundary used by prompt admission. Implementations receive only the immutable admitted
/// prompt, its deterministic rendering, and declared slots. Tests can provide a local evaluator;
/// production uses a real pinned provider and never trusts caller-supplied observed outputs.
pub trait PromptEvaluator: Send + Sync {
    fn evaluate(
        &self,
        artifact: &PromptArtifact,
        rendered: &str,
        slots: &BTreeMap<String, Json>,
    ) -> Result<Json, RuntimeError>;
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanaryObservation {
    pub input_hash: String,
    pub downstream_success: bool,
    pub guard_violation: bool,
    pub ledger_hash: String,
}

pub struct GenericArtifactGate {
    store: EmbeddedStore,
}

impl GenericArtifactGate {
    pub fn new(store: EmbeddedStore) -> Self {
        Self { store }
    }

    pub fn gate_flow(
        &self,
        flow: &FlowPush,
        cases: &[SandboxCase],
        limits: SandboxLimits,
        deployer_approval: Option<String>,
    ) -> Result<GateResult, RuntimeError> {
        let candidate = artifact_from_flow_push(flow)?;
        self.gate_flow_candidate(flow, candidate, cases, limits, deployer_approval, false)
    }

    /// Gate a builder artifact whose sealed interface/description intentionally differs from the
    /// compatibility defaults used by ordinary `FlowPush` callers. The stored immutable body must
    /// still match the supplied executable flow byte-for-byte.
    pub(crate) fn gate_stored_flow(
        &self,
        flow: &FlowPush,
        cases: &[SandboxCase],
        limits: SandboxLimits,
        deployer_approval: Option<String>,
    ) -> Result<GateResult, RuntimeError> {
        let artifacts = ArtifactRepository::open(self.store.clone()).map_err(artifact_error)?;
        let version = flow.flow_rev.parse::<u32>().map_err(|_| {
            RuntimeError::Invalid("flow_rev must be a positive base-10 artifact version".into())
        })?;
        let candidate = artifacts
            .get(&flow.tenant, &flow.flow_id, version)
            .map_err(artifact_error)?
            .ok_or_else(|| RuntimeError::NotFound(format!("{}@{version}", flow.flow_id)))?
            .artifact;
        let expected_body = serde_json::json!({
            "program": flow.program,
            "targets": flow.targets,
            "prompts": flow.prompts,
        });
        if candidate.body != expected_body {
            return Err(RuntimeError::Conflict(
                "gate flow body differs from the immutable stored artifact".into(),
            ));
        }
        self.gate_flow_candidate(flow, candidate, cases, limits, deployer_approval, true)
    }

    /// Gate an immutable composite artifact using the same lifecycle and evidence thresholds as a
    /// Flow. Child execution is resolved exclusively from the admitted pins sealed into `body`.
    #[allow(clippy::too_many_arguments)]
    pub fn gate_stored_harness(
        &self,
        tenant: &str,
        artifact_id: &str,
        version: u32,
        harness: &HarnessBody,
        cases: &[SandboxCase],
        limits: SandboxLimits,
        deployer_approval: Option<String>,
    ) -> Result<GateResult, RuntimeError> {
        harness.validate_shape().map_err(artifact_error)?;
        if cases
            .iter()
            .any(|case| !meaningful_expected(&case.expected))
        {
            return Err(RuntimeError::Invalid(
                "gate cases require non-empty expected stable properties".into(),
            ));
        }

        let mut artifacts = ArtifactRepository::open(self.store.clone()).map_err(artifact_error)?;
        let mut record = artifacts
            .get(tenant, artifact_id, version)
            .map_err(artifact_error)?
            .ok_or_else(|| RuntimeError::NotFound(format!("{artifact_id}@{version}")))?;
        if record.artifact.class != crate::ArtifactClass::Harness
            || record.artifact.body != serde_json::json!({"harness": harness})
        {
            return Err(RuntimeError::Conflict(
                "gate harness differs from the immutable stored artifact".into(),
            ));
        }
        if record.status == ArtifactStatus::Proposed {
            record = match artifacts.transition(
                tenant,
                artifact_id,
                version,
                ArtifactTransition {
                    expected: ArtifactStatus::Proposed,
                    trigger: ArtifactTrigger::StructuralPass,
                    actor: ArtifactActor::System,
                    evidence_snapshot_hash: Some(record.artifact.hash.clone()),
                },
            ) {
                Ok(record) => record,
                Err(crate::ArtifactError::Conflict(_)) => artifacts
                    .get(tenant, artifact_id, version)
                    .map_err(artifact_error)?
                    .ok_or_else(|| RuntimeError::NotFound(format!("{artifact_id}@{version}")))?,
                Err(error) => return Err(artifact_error(error)),
            };
        }
        if !matches!(
            record.status,
            ArtifactStatus::Shadow | ArtifactStatus::Canary | ArtifactStatus::Promoted
        ) {
            return Err(RuntimeError::Conflict(format!(
                "gate requires shadow/canary/promoted status, found {:?}",
                record.status
            )));
        }

        let report =
            HarnessSandboxRunner::new(self.store.clone()).run(tenant, harness, cases, limits)?;
        let mut evidence =
            ArtifactEvidenceRepository::open(self.store.clone()).map_err(artifact_error)?;
        for case in &report.cases {
            evidence
                .record(
                    tenant,
                    ArtifactEvidence::observed(ArtifactEvidenceDraft {
                        artifact_id: artifact_id.to_owned(),
                        artifact_version: version,
                        phase: EvidencePhase::Shadow,
                        input_hash: case.input_hash.clone(),
                        agreed: case.agreed,
                        downstream_success: case.agreed,
                        guard_violation: false,
                        ledger_hash: case.ledger_hash.clone(),
                    })
                    .map_err(artifact_error)?,
                )
                .map_err(artifact_error)?;
        }
        let summary = evidence
            .summary(tenant, artifact_id, version, EvidencePhase::Shadow)
            .map_err(artifact_error)?;
        if record.status == ArtifactStatus::Shadow
            && summary.distinct_inputs >= 20
            && summary.validation_rate >= 0.95
            && report.cases.iter().all(|case| case.agreed)
        {
            let actor = match record.artifact.tier {
                ArtifactTier::Auto => Some(ArtifactActor::System),
                ArtifactTier::Reviewed => deployer_approval.map(ArtifactActor::Deployer),
                ArtifactTier::Locked => None,
            };
            if let Some(actor) = actor {
                let snapshot = serde_json::to_value(&summary)
                    .map_err(|error| RuntimeError::Internal(error.to_string()))?;
                let snapshot_hash = value_hash(
                    &aelio_kernel::json_from(&snapshot)
                        .map_err(|error| RuntimeError::Internal(error.to_string()))?,
                );
                record = match artifacts.transition(
                    tenant,
                    artifact_id,
                    version,
                    ArtifactTransition {
                        expected: ArtifactStatus::Shadow,
                        trigger: ArtifactTrigger::ShadowThresholdsMet {
                            distinct_inputs: summary.distinct_inputs,
                            validation_rate: summary.validation_rate,
                            approved: true,
                        },
                        actor,
                        evidence_snapshot_hash: Some(snapshot_hash),
                    },
                ) {
                    Ok(record) => record,
                    Err(crate::ArtifactError::Conflict(_)) => artifacts
                        .get(tenant, artifact_id, version)
                        .map_err(artifact_error)?
                        .ok_or_else(|| {
                            RuntimeError::NotFound(format!("{artifact_id}@{version}"))
                        })?,
                    Err(error) => return Err(artifact_error(error)),
                };
            }
        }
        Ok(GateResult {
            record,
            report,
            evidence: summary,
        })
    }

    fn gate_flow_candidate(
        &self,
        flow: &FlowPush,
        candidate: crate::Artifact,
        cases: &[SandboxCase],
        limits: SandboxLimits,
        deployer_approval: Option<String>,
        require_all: bool,
    ) -> Result<GateResult, RuntimeError> {
        let mut artifacts = ArtifactRepository::open(self.store.clone()).map_err(artifact_error)?;
        let mut record = artifacts
            .get(&flow.tenant, &candidate.id, candidate.version)
            .map_err(artifact_error)?
            .ok_or_else(|| {
                RuntimeError::NotFound(format!(
                    "artifact {}@{} must be pushed before gating",
                    candidate.id, candidate.version
                ))
            })?;
        if record.artifact.hash != candidate.hash {
            return Err(RuntimeError::Conflict(
                "gate candidate differs from the immutable pushed artifact".into(),
            ));
        }
        if record.status == ArtifactStatus::Proposed {
            record = match artifacts.transition(
                &flow.tenant,
                &candidate.id,
                candidate.version,
                ArtifactTransition {
                    expected: ArtifactStatus::Proposed,
                    trigger: ArtifactTrigger::StructuralPass,
                    actor: ArtifactActor::System,
                    evidence_snapshot_hash: Some(candidate.hash.clone()),
                },
            ) {
                Ok(record) => record,
                Err(crate::ArtifactError::Conflict(_)) => artifacts
                    .get(&flow.tenant, &candidate.id, candidate.version)
                    .map_err(artifact_error)?
                    .ok_or_else(|| RuntimeError::NotFound(candidate.key()))?,
                Err(error) => return Err(artifact_error(error)),
            };
        }
        if !matches!(
            record.status,
            ArtifactStatus::Shadow | ArtifactStatus::Canary | ArtifactStatus::Promoted
        ) {
            return Err(RuntimeError::Conflict(format!(
                "gate requires shadow/canary/promoted status, found {:?}",
                record.status
            )));
        }

        if cases
            .iter()
            .any(|case| !meaningful_expected(&case.expected))
        {
            return Err(RuntimeError::Invalid(
                "gate cases require non-empty expected stable properties".into(),
            ));
        }
        let report = SandboxRunner::run_flow_with_store(flow, cases, limits, &self.store)?;
        let mut evidence =
            ArtifactEvidenceRepository::open(self.store.clone()).map_err(artifact_error)?;
        for case in &report.cases {
            evidence
                .record(
                    &flow.tenant,
                    ArtifactEvidence::observed(ArtifactEvidenceDraft {
                        artifact_id: candidate.id.clone(),
                        artifact_version: candidate.version,
                        phase: EvidencePhase::Shadow,
                        input_hash: case.input_hash.clone(),
                        agreed: case.agreed,
                        downstream_success: case.agreed,
                        guard_violation: false,
                        ledger_hash: case.ledger_hash.clone(),
                    })
                    .map_err(artifact_error)?,
                )
                .map_err(artifact_error)?;
        }
        let summary = evidence
            .summary(
                &flow.tenant,
                &candidate.id,
                candidate.version,
                EvidencePhase::Shadow,
            )
            .map_err(artifact_error)?;

        if record.status == ArtifactStatus::Shadow
            && summary.distinct_inputs >= 20
            && summary.validation_rate >= 0.95
            && (!require_all || report.cases.iter().all(|case| case.agreed))
        {
            let actor = match candidate.tier {
                ArtifactTier::Auto => Some(ArtifactActor::System),
                ArtifactTier::Reviewed => deployer_approval.map(ArtifactActor::Deployer),
                ArtifactTier::Locked => None,
            };
            if let Some(actor) = actor {
                let snapshot = serde_json::to_value(&summary)
                    .map_err(|error| RuntimeError::Internal(error.to_string()))?;
                let snapshot_hash = value_hash(
                    &aelio_kernel::json_from(&snapshot)
                        .map_err(|error| RuntimeError::Internal(error.to_string()))?,
                );
                record = match artifacts.transition(
                    &flow.tenant,
                    &candidate.id,
                    candidate.version,
                    ArtifactTransition {
                        expected: ArtifactStatus::Shadow,
                        trigger: ArtifactTrigger::ShadowThresholdsMet {
                            distinct_inputs: summary.distinct_inputs,
                            validation_rate: summary.validation_rate,
                            approved: true,
                        },
                        actor,
                        evidence_snapshot_hash: Some(snapshot_hash),
                    },
                ) {
                    Ok(record) => record,
                    Err(crate::ArtifactError::Conflict(_)) => artifacts
                        .get(&flow.tenant, &candidate.id, candidate.version)
                        .map_err(artifact_error)?
                        .ok_or_else(|| RuntimeError::NotFound(candidate.key()))?,
                    Err(error) => return Err(artifact_error(error)),
                };
            }
        }
        Ok(GateResult {
            record,
            report,
            evidence: summary,
        })
    }

    pub fn gate_prompt(
        &self,
        tenant: &str,
        prompt: &PromptArtifact,
        cases: &[PromptGateCase],
        evaluator: &dyn PromptEvaluator,
        deployer_approval: Option<String>,
    ) -> Result<PromptGateResult, RuntimeError> {
        if cases.len() < 3 || cases.len() > 128 {
            return Err(RuntimeError::Invalid(
                "prompt gate requires 3..=128 exemplar cases".into(),
            ));
        }
        if !cases
            .iter()
            .any(|case| matches!(case.kind, PromptCaseKind::Negative))
        {
            return Err(RuntimeError::Invalid(
                "prompt gate requires at least one negative/refusal case".into(),
            ));
        }
        aelio_prompt::validate_artifact(prompt)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        if !prompt.layers.is_empty() {
            return Err(RuntimeError::Invalid(
                "prompt gate cannot execute unresolved layers; admit and resolve layer artifacts first"
                    .into(),
            ));
        }
        let candidate = crate::mint::prompt_as_artifact(prompt, tenant)?;
        let mut artifacts = ArtifactRepository::open(self.store.clone()).map_err(artifact_error)?;
        let mut record = artifacts
            .get(tenant, &candidate.id, candidate.version)
            .map_err(artifact_error)?
            .ok_or_else(|| RuntimeError::NotFound(candidate.key()))?;
        if record.artifact.hash != candidate.hash {
            return Err(RuntimeError::Conflict(
                "prompt gate candidate differs from the immutable stored artifact".into(),
            ));
        }
        if record.status == ArtifactStatus::Proposed {
            record = artifacts
                .transition(
                    tenant,
                    &candidate.id,
                    candidate.version,
                    ArtifactTransition {
                        expected: ArtifactStatus::Proposed,
                        trigger: ArtifactTrigger::StructuralPass,
                        actor: ArtifactActor::System,
                        evidence_snapshot_hash: Some(candidate.hash.clone()),
                    },
                )
                .map_err(artifact_error)?;
        }
        if record.status != ArtifactStatus::Shadow {
            return Err(RuntimeError::Conflict(format!(
                "prompt shadow gate requires shadow status, found {:?}",
                record.status
            )));
        }

        let declared_slots: BTreeMap<_, _> = prompt
            .slots
            .iter()
            .map(|slot| (slot.name.as_str(), slot))
            .collect();
        let declared_outputs: BTreeSet<_> =
            prompt.output_fields.keys().map(String::as_str).collect();
        let mut registry = TemplateRegistry::default();
        registry
            .register(prompt.to_template())
            .map_err(RuntimeError::Invalid)?;
        let mut evidence =
            ArtifactEvidenceRepository::open(self.store.clone()).map_err(artifact_error)?;
        let mut reports = Vec::with_capacity(cases.len());
        for case in cases {
            validate_prompt_case(case, &declared_slots, &declared_outputs, prompt)?;
            let slots_json = Json::Object(case.slots.clone().into_iter().collect());
            let slots_sol = aelio_kernel::json_from(&slots_json)
                .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
            let composed = registry
                .compose(&prompt.key(), &slots_sol)
                .map_err(RuntimeError::Invalid)?;
            let input_hash = value_hash(&slots_sol);
            let evaluated = evaluator.evaluate(prompt, &composed.text, &case.slots);
            let (actual, detail) = match evaluated {
                Ok(actual) => (actual, None),
                Err(error) => (Json::Null, Some(error.to_string())),
            };
            let agreed = detail.is_none()
                && output_matches_contract(&actual, &prompt.output_fields)
                && stable_subset(
                    &Json::Object(case.expected.clone().into_iter().collect()),
                    &actual,
                );
            let ledger_value = serde_json::json!({
                "artifact": candidate.key(),
                "input_hash": input_hash,
                "prompt_hash": composed.prompt_hash,
                "actual": actual,
                "agreed": agreed,
            });
            let ledger_hash = value_hash(
                &aelio_kernel::json_from(&ledger_value)
                    .map_err(|error| RuntimeError::Internal(error.to_string()))?,
            );
            evidence
                .record(
                    tenant,
                    ArtifactEvidence::observed(ArtifactEvidenceDraft {
                        artifact_id: candidate.id.clone(),
                        artifact_version: candidate.version,
                        phase: EvidencePhase::Shadow,
                        input_hash: input_hash.clone(),
                        agreed,
                        downstream_success: agreed,
                        guard_violation: false,
                        ledger_hash: ledger_hash.clone(),
                    })
                    .map_err(artifact_error)?,
                )
                .map_err(artifact_error)?;
            reports.push(PromptGateCaseReport {
                input_hash,
                prompt_hash: composed.prompt_hash,
                agreed,
                detail,
                ledger_hash,
            });
        }
        let summary = evidence
            .summary(
                tenant,
                &candidate.id,
                candidate.version,
                EvidencePhase::Shadow,
            )
            .map_err(artifact_error)?;
        if summary.distinct_inputs >= 20 && summary.validation_rate >= 0.95 {
            let actor = match candidate.tier {
                ArtifactTier::Auto => Some(ArtifactActor::System),
                ArtifactTier::Reviewed => deployer_approval.map(ArtifactActor::Deployer),
                ArtifactTier::Locked => None,
            };
            if let Some(actor) = actor {
                let snapshot = serde_json::to_value(&summary)
                    .map_err(|error| RuntimeError::Internal(error.to_string()))?;
                let snapshot_hash = value_hash(
                    &aelio_kernel::json_from(&snapshot)
                        .map_err(|error| RuntimeError::Internal(error.to_string()))?,
                );
                record = artifacts
                    .transition(
                        tenant,
                        &candidate.id,
                        candidate.version,
                        ArtifactTransition {
                            expected: ArtifactStatus::Shadow,
                            trigger: ArtifactTrigger::ShadowThresholdsMet {
                                distinct_inputs: summary.distinct_inputs,
                                validation_rate: summary.validation_rate,
                                approved: true,
                            },
                            actor,
                            evidence_snapshot_hash: Some(snapshot_hash),
                        },
                    )
                    .map_err(artifact_error)?;
            }
        }
        Ok(PromptGateResult {
            record,
            report: PromptGateReport {
                artifact: candidate.key(),
                cases: reports,
            },
            evidence: summary,
        })
    }

    pub fn observe_canary(
        &self,
        tenant: &str,
        artifact_id: &str,
        artifact_version: u32,
        observation: CanaryObservation,
    ) -> Result<CanaryResult, RuntimeError> {
        let mut artifacts = ArtifactRepository::open(self.store.clone()).map_err(artifact_error)?;
        let mut record = artifacts
            .get(tenant, artifact_id, artifact_version)
            .map_err(artifact_error)?
            .ok_or_else(|| {
                RuntimeError::NotFound(format!("artifact {artifact_id}@{artifact_version}"))
            })?;
        if record.status != ArtifactStatus::Canary {
            return Err(RuntimeError::Conflict(format!(
                "canary observation requires canary status, found {:?}",
                record.status
            )));
        }
        let mut repository =
            ArtifactEvidenceRepository::open(self.store.clone()).map_err(artifact_error)?;
        repository
            .record(
                tenant,
                ArtifactEvidence::observed(ArtifactEvidenceDraft {
                    artifact_id: artifact_id.into(),
                    artifact_version,
                    phase: EvidencePhase::Canary,
                    input_hash: observation.input_hash,
                    agreed: observation.downstream_success,
                    downstream_success: observation.downstream_success,
                    guard_violation: observation.guard_violation,
                    ledger_hash: observation.ledger_hash,
                })
                .map_err(artifact_error)?,
            )
            .map_err(artifact_error)?;
        let summary = repository
            .summary(tenant, artifact_id, artifact_version, EvidencePhase::Canary)
            .map_err(artifact_error)?;
        let failure_rate = 1.0 - summary.downstream_success_rate;
        let should_demote = summary.guard_violations > 0 || failure_rate > 0.02;
        let should_propose = summary.distinct_inputs >= 20
            && summary.downstream_success_rate >= 0.98
            && summary.guard_violations == 0;
        if should_demote {
            let trigger = ArtifactTrigger::AttributedFailure {
                failure_rate,
                guard_violation: summary.guard_violations > 0,
            };
            let snapshot = serde_json::to_value(&summary)
                .map_err(|error| RuntimeError::Internal(error.to_string()))?;
            let snapshot_hash = value_hash(
                &aelio_kernel::json_from(&snapshot)
                    .map_err(|error| RuntimeError::Internal(error.to_string()))?,
            );
            record = match artifacts.transition(
                tenant,
                artifact_id,
                artifact_version,
                ArtifactTransition {
                    expected: ArtifactStatus::Canary,
                    trigger,
                    actor: ArtifactActor::System,
                    evidence_snapshot_hash: Some(snapshot_hash),
                },
            ) {
                Ok(record) => record,
                Err(crate::ArtifactError::Conflict(_)) => {
                    let latest = artifacts
                        .get(tenant, artifact_id, artifact_version)
                        .map_err(artifact_error)?
                        .ok_or_else(|| {
                            RuntimeError::NotFound(format!(
                                "artifact {artifact_id}@{artifact_version}"
                            ))
                        })?;
                    if latest.status == ArtifactStatus::Canary {
                        return Err(RuntimeError::Conflict(
                            "canary lifecycle changed concurrently; retry observation".into(),
                        ));
                    }
                    latest
                }
                Err(error) => return Err(artifact_error(error)),
            };
        }
        let promotion_proposal = if should_propose && !should_demote {
            let snapshot = serde_json::to_value(&summary)
                .map_err(|error| RuntimeError::Internal(error.to_string()))?;
            let snapshot_hash = value_hash(
                &aelio_kernel::json_from(&snapshot)
                    .map_err(|error| RuntimeError::Internal(error.to_string()))?,
            );
            Some(
                PromotionProposalRepository::open(self.store.clone())
                    .map_err(artifact_error)?
                    .propose(tenant, &record.artifact.key(), &snapshot_hash)
                    .map_err(artifact_error)?,
            )
        } else {
            None
        };
        Ok(CanaryResult {
            record,
            evidence: summary,
            promotion_proposal,
        })
    }

    /// Apply a steward proposal through the lifecycle authority. Re-running after a crash is
    /// idempotent: an already-promoted artifact causes the still-pending proposal to be finalized.
    pub fn apply_promotion_proposal(
        &self,
        tenant: &str,
        proposal_id: &str,
    ) -> Result<CanaryResult, RuntimeError> {
        let mut proposals =
            PromotionProposalRepository::open(self.store.clone()).map_err(artifact_error)?;
        let proposal = proposals
            .get(tenant, proposal_id)
            .map_err(artifact_error)?
            .ok_or_else(|| RuntimeError::NotFound(proposal_id.into()))?;
        if proposal.status == PromotionProposalStatus::Rejected {
            return Err(RuntimeError::Conflict(
                "rejected promotion proposal cannot be applied".into(),
            ));
        }
        let (artifact_id, version) = parse_pin(&proposal.artifact_pin)?;
        let evidence =
            ArtifactEvidenceRepository::open(self.store.clone()).map_err(artifact_error)?;
        let summary = evidence
            .summary(tenant, artifact_id, version, EvidencePhase::Canary)
            .map_err(artifact_error)?;
        let snapshot = serde_json::to_value(&summary)
            .map_err(|error| RuntimeError::Internal(error.to_string()))?;
        let snapshot_hash = value_hash(
            &aelio_kernel::json_from(&snapshot)
                .map_err(|error| RuntimeError::Internal(error.to_string()))?,
        );
        if snapshot_hash != proposal.evidence_hash
            || summary.distinct_inputs < 20
            || summary.downstream_success_rate < 0.98
            || summary.guard_violations != 0
        {
            return Err(RuntimeError::Conflict(
                "promotion proposal evidence is stale or below threshold".into(),
            ));
        }
        let mut artifacts = ArtifactRepository::open(self.store.clone()).map_err(artifact_error)?;
        let mut record = artifacts
            .get(tenant, artifact_id, version)
            .map_err(artifact_error)?
            .ok_or_else(|| RuntimeError::NotFound(proposal.artifact_pin.clone()))?;
        if record.status == ArtifactStatus::Canary {
            record = artifacts
                .transition(
                    tenant,
                    artifact_id,
                    version,
                    ArtifactTransition {
                        expected: ArtifactStatus::Canary,
                        trigger: ArtifactTrigger::CanaryThresholdsMet {
                            distinct_inputs: summary.distinct_inputs,
                            downstream_success: summary.downstream_success_rate,
                            guard_violations: summary.guard_violations,
                        },
                        actor: ArtifactActor::System,
                        evidence_snapshot_hash: Some(snapshot_hash),
                    },
                )
                .map_err(artifact_error)?;
        } else if record.status != ArtifactStatus::Promoted {
            return Err(RuntimeError::Conflict(format!(
                "promotion proposal requires canary/promoted artifact, found {:?}",
                record.status
            )));
        }
        let applied = proposals
            .finish(tenant, proposal_id, PromotionProposalStatus::Applied)
            .map_err(artifact_error)?;
        Ok(CanaryResult {
            record,
            evidence: summary,
            promotion_proposal: Some(applied),
        })
    }
}

fn parse_pin(pin: &str) -> Result<(&str, u32), RuntimeError> {
    let (id, version) = pin
        .rsplit_once('@')
        .ok_or_else(|| RuntimeError::Invalid(format!("invalid artifact pin `{pin}`")))?;
    let version = version
        .parse::<u32>()
        .map_err(|_| RuntimeError::Invalid(format!("invalid artifact pin `{pin}`")))?;
    if id.is_empty() || version == 0 {
        return Err(RuntimeError::Invalid(format!(
            "invalid artifact pin `{pin}`"
        )));
    }
    Ok((id, version))
}

fn validate_prompt_case(
    case: &PromptGateCase,
    slots: &BTreeMap<&str, &aelio_prompt::SlotDecl>,
    outputs: &BTreeSet<&str>,
    prompt: &PromptArtifact,
) -> Result<(), RuntimeError> {
    if case.expected.is_empty()
        || case
            .expected
            .keys()
            .any(|name| !outputs.contains(name.as_str()))
    {
        return Err(RuntimeError::Invalid(
            "prompt gate expected properties must be non-empty declared output fields".into(),
        ));
    }
    if case
        .slots
        .keys()
        .any(|name| !slots.contains_key(name.as_str()))
    {
        return Err(RuntimeError::Invalid(
            "prompt gate case contains an undeclared input slot".into(),
        ));
    }
    for (name, slot) in slots {
        match case.slots.get(*name) {
            Some(value) if json_has_type(value, &slot.ty) => {}
            Some(_) => {
                return Err(RuntimeError::Invalid(format!(
                    "prompt gate slot `{name}` has the wrong JSON type"
                )))
            }
            None if slot.required => {
                return Err(RuntimeError::Invalid(format!(
                    "prompt gate case misses required slot `{name}`"
                )))
            }
            None => {}
        }
    }
    if matches!(case.kind, PromptCaseKind::Negative)
        && prompt
            .output_fields
            .get("undeterminable")
            .map(String::as_str)
            == Some("bool")
        && case.expected.get("undeterminable") != Some(&Json::Bool(true))
    {
        return Err(RuntimeError::Invalid(
            "negative prompt case must expect undeterminable=true".into(),
        ));
    }
    Ok(())
}

fn output_matches_contract(actual: &Json, fields: &BTreeMap<String, String>) -> bool {
    let Some(object) = actual.as_object() else {
        return false;
    };
    object.len() == fields.len()
        && fields.iter().all(|(name, ty)| {
            object
                .get(name)
                .is_some_and(|value| json_has_type(value, ty))
        })
}

fn json_has_type(value: &Json, ty: &str) -> bool {
    match ty {
        "null" => value.is_null(),
        "bool" => value.is_boolean(),
        "int" => value.as_i64().is_some() || value.as_u64().is_some(),
        "float" => value.is_number(),
        "str" => value.is_string(),
        "list" => value.is_array(),
        "map" => value.is_object(),
        _ => false,
    }
}

fn stable_subset(expected: &Json, actual: &Json) -> bool {
    match (expected, actual) {
        (Json::Object(expected), Json::Object(actual)) => expected.iter().all(|(key, value)| {
            actual
                .get(key)
                .is_some_and(|actual| stable_subset(value, actual))
        }),
        (Json::Array(expected), Json::Array(actual)) => {
            expected.len() == actual.len()
                && expected
                    .iter()
                    .zip(actual)
                    .all(|(expected, actual)| stable_subset(expected, actual))
        }
        _ => expected == actual,
    }
}

fn meaningful_expected(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Object(values) => !values.is_empty(),
        serde_json::Value::Array(values) => !values.is_empty(),
        serde_json::Value::String(value) => !value.is_empty(),
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => true,
    }
}
