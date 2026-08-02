//! Closure between the mother-document learning predicates and the unified artifact authority.
//!
//! Mining never registers an executable target directly. It proposes an immutable Procedure view;
//! shadow traces are compared by `aelio-learn`, then the resulting agreement enters the same
//! evidence/lifecycle repository as every other artifact.

use crate::{
    artifact_error, AgreementGateResult, AgreementObservation, ArtifactActor, ArtifactClass,
    ArtifactEffect, ArtifactRecord, GenericArtifactGate, ProcedureArtifactDraft,
    ProcedureSignatureStep, Provenance, Runtime, RuntimeError,
};
use aelio_learn::{procedure_agreement, ProcedureCandidate, TraceStep};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct LearnedProcedureRequest {
    pub tenant: String,
    pub id: String,
    pub version: u32,
    pub situation_hash: String,
    pub candidate: ProcedureCandidate,
    /// A pre-built, admitted Flow/Harness containing the mined subsequence.
    pub implementation: String,
    /// Miner Call id -> already admitted immutable Flow/Harness artifact pin.
    pub call_artifacts: BTreeMap<String, String>,
    pub tool_dependencies: Vec<String>,
    pub prompt_dependencies: Vec<String>,
    pub embedding_model: String,
    pub effect: ArtifactEffect,
    pub examples: Vec<serde_json::Value>,
    pub requester: String,
}

#[derive(Debug, Clone)]
pub struct ProcedureTraceObservation {
    pub input_hash: String,
    pub actual: Vec<TraceStep>,
    pub downstream_success: bool,
    pub guard_violation: bool,
    pub ledger_hash: String,
}

impl Runtime {
    /// Register a mined candidate as inert `proposed` data. The mining threshold is only proposal
    /// eligibility; it cannot weaken the generic 20-distinct shadow threshold.
    pub fn propose_learned_procedure(
        &self,
        request: LearnedProcedureRequest,
    ) -> Result<ArtifactRecord, RuntimeError> {
        if request.candidate.occurrences < 5 || request.candidate.distinct_flows < 5 {
            return Err(RuntimeError::Invalid(
                "learned procedure requires at least five successful distinct source flows".into(),
            ));
        }
        let steps = request
            .candidate
            .signature
            .iter()
            .map(|step| {
                request
                    .call_artifacts
                    .get(&step.call_id)
                    .cloned()
                    .ok_or_else(|| {
                        RuntimeError::Invalid(format!(
                            "mined Call `{}` has no admitted artifact pin",
                            step.call_id
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let signature = request
            .candidate
            .signature
            .iter()
            .map(|step| ProcedureSignatureStep {
                call_id: step.call_id.clone(),
                arg_shape: step.arg_shape.clone(),
            })
            .collect();
        let artifact = ProcedureArtifactDraft {
            id: request.id,
            version: request.version,
            situation_hash: request.situation_hash,
            signature,
            implementation: request.implementation,
            steps,
            tool_dependencies: request.tool_dependencies,
            prompt_dependencies: request.prompt_dependencies,
            embedding_model: request.embedding_model,
            effect: request.effect,
            examples: request.examples,
        }
        .lower(Provenance {
            built_by: Some("aelio.learn.procedure_miner@1".into()),
            requester: Some(request.requester),
            ..Provenance::default()
        })
        .map_err(artifact_error)?;
        self.artifact_repository()?
            .put_proposed(&request.tenant, artifact, ArtifactActor::System)
            .map_err(artifact_error)
    }

    /// Compare mined signature predictions with old-path traces and apply the generic lifecycle.
    /// Callers supply ledger references and observations, never an `agreed` boolean.
    pub fn gate_learned_procedure(
        &self,
        tenant: &str,
        artifact_id: &str,
        artifact_version: u32,
        observations: &[ProcedureTraceObservation],
        deployer_approval: Option<String>,
    ) -> Result<AgreementGateResult, RuntimeError> {
        let record = self
            .artifact_repository()?
            .get(tenant, artifact_id, artifact_version)
            .map_err(artifact_error)?
            .ok_or_else(|| {
                RuntimeError::NotFound(format!("artifact {artifact_id}@{artifact_version}"))
            })?;
        if record.artifact.class != ArtifactClass::Procedure {
            return Err(RuntimeError::Conflict(
                "learned procedure gate received a non-Procedure artifact".into(),
            ));
        }
        let body: ProcedureArtifactDraft = serde_json::from_value(record.artifact.body.clone())
            .map_err(|error| RuntimeError::Internal(format!("corrupt Procedure body: {error}")))?;
        let predicted = body
            .signature
            .iter()
            .map(|step| TraceStep {
                call_id: step.call_id.clone(),
                arg_shape: step.arg_shape.clone(),
            })
            .collect::<Vec<_>>();
        let agreements = observations
            .iter()
            .map(|observation| AgreementObservation {
                input_hash: observation.input_hash.clone(),
                agreed: procedure_agreement(&predicted, &observation.actual),
                downstream_success: observation.downstream_success,
                guard_violation: observation.guard_violation,
                ledger_hash: observation.ledger_hash.clone(),
            })
            .collect::<Vec<_>>();
        GenericArtifactGate::new(self.inner.store.clone()).gate_agreement(
            tenant,
            artifact_id,
            artifact_version,
            ArtifactClass::Procedure,
            &agreements,
            deployer_approval,
        )
    }
}
