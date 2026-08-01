//! Distinct-input evidence used by the generic artifact lifecycle gate.

use super::{
    decode, encode, ensure_repository_schema, now_ms, store_error, validate_hash, validate_id,
    validate_tenant, ArtifactError,
};
use aelio_store::{PutIfAbsent, Store};
use serde::{Deserialize, Serialize};

pub const ARTIFACT_EVIDENCE_TABLE: &str = "artifact_evidence";
pub const EVIDENCE_SCHEMA_VERSION: u32 = 1;
pub const MAX_EVIDENCE_PER_ARTIFACT_PHASE: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidencePhase {
    Shadow,
    Canary,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactEvidence {
    pub artifact_id: String,
    pub artifact_version: u32,
    pub phase: EvidencePhase,
    pub input_hash: String,
    pub agreed: bool,
    pub downstream_success: bool,
    pub guard_violation: bool,
    pub ledger_hash: String,
    pub observed_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactEvidenceDraft {
    pub artifact_id: String,
    pub artifact_version: u32,
    pub phase: EvidencePhase,
    pub input_hash: String,
    pub agreed: bool,
    pub downstream_success: bool,
    pub guard_violation: bool,
    pub ledger_hash: String,
}

impl ArtifactEvidence {
    pub fn observed(draft: ArtifactEvidenceDraft) -> Result<Self, ArtifactError> {
        let evidence = Self {
            artifact_id: draft.artifact_id,
            artifact_version: draft.artifact_version,
            phase: draft.phase,
            input_hash: draft.input_hash,
            agreed: draft.agreed,
            downstream_success: draft.downstream_success,
            guard_violation: draft.guard_violation,
            ledger_hash: draft.ledger_hash,
            observed_at_ms: now_ms()?,
        };
        validate_evidence(&evidence)?;
        Ok(evidence)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceSummary {
    pub distinct_inputs: u32,
    pub validation_rate: f64,
    pub downstream_success_rate: f64,
    pub guard_violations: u32,
}

#[derive(Clone)]
pub struct ArtifactEvidenceRepository<S: Store + Clone> {
    store: S,
}

impl<S: Store + Clone> ArtifactEvidenceRepository<S> {
    pub fn open(mut store: S) -> Result<Self, ArtifactError> {
        ensure_repository_schema(&mut store, ARTIFACT_EVIDENCE_TABLE, EVIDENCE_SCHEMA_VERSION)?;
        Ok(Self { store })
    }

    pub fn record(
        &mut self,
        tenant: &str,
        evidence: ArtifactEvidence,
    ) -> Result<ArtifactEvidence, ArtifactError> {
        validate_tenant(tenant)?;
        validate_evidence(&evidence)?;
        let key = evidence_key(&evidence);
        match self
            .store
            .put_if_absent(tenant, ARTIFACT_EVIDENCE_TABLE, &key, encode(&evidence)?)
            .map_err(store_error)?
        {
            PutIfAbsent::Inserted { .. } => Ok(evidence),
            PutIfAbsent::Existing(existing) => {
                let current: ArtifactEvidence = decode(&existing.value)?;
                if same_observation(&current, &evidence) {
                    Ok(current)
                } else {
                    Err(ArtifactError::Conflict(
                        "the same distinct input already has different evidence".into(),
                    ))
                }
            }
        }
    }

    pub fn summary(
        &self,
        tenant: &str,
        artifact_id: &str,
        artifact_version: u32,
        phase: EvidencePhase,
    ) -> Result<EvidenceSummary, ArtifactError> {
        validate_tenant(tenant)?;
        validate_id(artifact_id)?;
        let prefix = format!(
            "{}@{}|{}|",
            artifact_id,
            artifact_version,
            phase_name(phase)
        );
        let rows = self
            .store
            .scan_prefix(
                tenant,
                ARTIFACT_EVIDENCE_TABLE,
                &prefix,
                MAX_EVIDENCE_PER_ARTIFACT_PHASE + 1,
            )
            .map_err(store_error)?;
        if rows.len() > MAX_EVIDENCE_PER_ARTIFACT_PHASE {
            return Err(ArtifactError::Invalid(
                "artifact evidence exceeds its hydration bound".into(),
            ));
        }
        let mut agreed = 0u32;
        let mut downstream = 0u32;
        let mut guards = 0u32;
        for (_, row) in &rows {
            let evidence: ArtifactEvidence = decode(&row.value)?;
            validate_evidence(&evidence)?;
            agreed += u32::from(evidence.agreed);
            downstream += u32::from(evidence.downstream_success);
            guards += u32::from(evidence.guard_violation);
        }
        let distinct_inputs = u32::try_from(rows.len())
            .map_err(|_| ArtifactError::Invalid("evidence count overflow".into()))?;
        let rate = |count: u32| {
            if distinct_inputs == 0 {
                0.0
            } else {
                f64::from(count) / f64::from(distinct_inputs)
            }
        };
        Ok(EvidenceSummary {
            distinct_inputs,
            validation_rate: rate(agreed),
            downstream_success_rate: rate(downstream),
            guard_violations: guards,
        })
    }
}

fn validate_evidence(evidence: &ArtifactEvidence) -> Result<(), ArtifactError> {
    validate_id(&evidence.artifact_id)?;
    if evidence.artifact_version == 0 {
        return Err(ArtifactError::Invalid(
            "evidence artifact_version must be positive".into(),
        ));
    }
    validate_hash("input_hash", &evidence.input_hash)?;
    validate_hash("ledger_hash", &evidence.ledger_hash)?;
    Ok(())
}

fn evidence_key(evidence: &ArtifactEvidence) -> String {
    format!(
        "{}@{}|{}|{}",
        evidence.artifact_id,
        evidence.artifact_version,
        phase_name(evidence.phase),
        evidence.input_hash
    )
}

fn phase_name(phase: EvidencePhase) -> &'static str {
    match phase {
        EvidencePhase::Shadow => "shadow",
        EvidencePhase::Canary => "canary",
    }
}

fn same_observation(left: &ArtifactEvidence, right: &ArtifactEvidence) -> bool {
    left.artifact_id == right.artifact_id
        && left.artifact_version == right.artifact_version
        && left.phase == right.phase
        && left.input_hash == right.input_hash
        && left.agreed == right.agreed
        && left.downstream_success == right.downstream_success
        && left.guard_violation == right.guard_violation
        && left.ledger_hash == right.ledger_hash
}
