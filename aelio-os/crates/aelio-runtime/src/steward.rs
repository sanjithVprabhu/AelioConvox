//! Deterministic steward proposals and dependency cascades. The steward can demote invalidated
//! dependents and write control-plane proposals; it has no API that directly promotes artifacts.

use crate::{
    artifact_error, ArtifactActor, ArtifactPins, ArtifactRepository, ArtifactStatus,
    ArtifactTransition, ArtifactTrigger, BuildResult, BuildSpec, CapabilityReason,
    CapabilityRequest, CapabilityRequestDraft, CapabilityRequestRepository, PromotionProposal,
    PromotionProposalRepository, Runtime, RuntimeError,
};
use aelio_sol::value_hash;
use serde::Serialize;
use std::collections::{HashSet, VecDeque};

#[derive(Debug, Clone, Serialize)]
pub struct DependencyCascade {
    pub dependency: String,
    pub demoted: Vec<String>,
    pub capability_requests: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KernelMigration {
    pub kernel_version: String,
    pub demoted: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DemandBuild {
    pub request: CapabilityRequest,
    pub job: crate::BuildJob,
}

impl Runtime {
    pub fn record_capability_request(
        &self,
        tenant: &str,
        draft: CapabilityRequestDraft,
    ) -> Result<CapabilityRequest, RuntimeError> {
        CapabilityRequestRepository::open(self.inner.store.clone())
            .map_err(artifact_error)?
            .record(tenant, draft)
            .map_err(artifact_error)
    }

    pub fn list_capability_requests(
        &self,
        tenant: &str,
        limit: usize,
    ) -> Result<Vec<crate::CapabilityRequest>, RuntimeError> {
        CapabilityRequestRepository::open(self.inner.store.clone())
            .map_err(artifact_error)?
            .list_open(tenant, limit)
            .map_err(artifact_error)
    }

    pub fn get_capability_request(
        &self,
        tenant: &str,
        request_id: &str,
    ) -> Result<Option<CapabilityRequest>, RuntimeError> {
        CapabilityRequestRepository::open(self.inner.store.clone())
            .map_err(artifact_error)?
            .get(tenant, request_id)
            .map_err(artifact_error)
    }

    /// Bind a reviewed BuildSpec to an observed demand. The request interface/effect ceiling and
    /// tenant are immutable authority; a supplied spec may not reinterpret or escalate them.
    pub fn queue_capability_build(
        &self,
        tenant: &str,
        request_id: &str,
        spec: BuildSpec,
    ) -> Result<DemandBuild, RuntimeError> {
        spec.validate().map_err(artifact_error)?;
        let mut requests =
            CapabilityRequestRepository::open(self.inner.store.clone()).map_err(artifact_error)?;
        let request = requests
            .get(tenant, request_id)
            .map_err(artifact_error)?
            .ok_or_else(|| RuntimeError::NotFound(request_id.into()))?;
        let allowed: HashSet<_> = request.draft.allowed_effects.iter().copied().collect();
        if spec.draft.scope.tenant != tenant
            || spec.draft.inputs != request.draft.inputs
            || spec.draft.output != request.draft.output
            || spec
                .draft
                .policy
                .allowed_effects
                .iter()
                .any(|effect| !allowed.contains(effect))
        {
            return Err(RuntimeError::Conflict(
                "build spec does not preserve the demand tenant/interface/effect ceiling".into(),
            ));
        }
        let job = self.submit_build(spec)?;
        let request = requests
            .queue(tenant, request_id, &job.build_id)
            .map_err(artifact_error)?;
        Ok(DemandBuild { request, job })
    }

    pub(crate) fn settle_capability_build(
        &self,
        tenant: &str,
        build_id: &str,
        result: &BuildResult,
    ) -> Result<(), RuntimeError> {
        let mut requests =
            CapabilityRequestRepository::open(self.inner.store.clone()).map_err(artifact_error)?;
        for request in requests
            .find_by_build(tenant, build_id, 1_000)
            .map_err(artifact_error)?
        {
            match result {
                BuildResult::Built { .. } | BuildResult::Reused { .. } => {
                    requests
                        .resolve(tenant, &request.request_id, build_id)
                        .map_err(artifact_error)?;
                }
                BuildResult::Failed { .. } => {
                    requests
                        .fail(tenant, &request.request_id, build_id)
                        .map_err(artifact_error)?;
                }
            }
        }
        Ok(())
    }

    pub fn steward_dependency_departure(
        &self,
        tenant: &str,
        dependency_pin: &str,
    ) -> Result<DependencyCascade, RuntimeError> {
        let mut artifacts =
            ArtifactRepository::open(self.inner.store.clone()).map_err(artifact_error)?;
        let mut candidates = artifacts.list(tenant, 1_000).map_err(artifact_error)?;
        candidates.sort_by_key(|record| record.artifact.key());
        let mut demand =
            CapabilityRequestRepository::open(self.inner.store.clone()).map_err(artifact_error)?;
        let mut demoted = Vec::new();
        let mut requests = Vec::new();
        let mut frontier = VecDeque::from([dependency_pin.to_owned()]);
        let mut visited_dependencies = HashSet::new();
        let mut visited_artifacts = HashSet::new();
        while let Some(departed) = frontier.pop_front() {
            if !visited_dependencies.insert(departed.clone()) {
                continue;
            }
            for candidate in &candidates {
                let candidate_pin = candidate.artifact.key();
                if visited_artifacts.contains(&candidate_pin)
                    || !matches!(
                        candidate.status,
                        ArtifactStatus::Canary | ArtifactStatus::Promoted
                    )
                    || !artifact_depends_on(&candidate.artifact, &departed)
                {
                    continue;
                }
                let updated = artifacts
                    .transition(
                        tenant,
                        &candidate.artifact.id,
                        candidate.artifact.version,
                        ArtifactTransition {
                            expected: candidate.status,
                            trigger: ArtifactTrigger::Invalidated,
                            actor: ArtifactActor::System,
                            evidence_snapshot_hash: Some(candidate.artifact.hash.clone()),
                        },
                    )
                    .map_err(artifact_error)?;
                visited_artifacts.insert(candidate_pin.clone());
                demoted.push(candidate_pin.clone());
                frontier.push_back(candidate_pin);
                let request = demand
                    .record(
                        tenant,
                        CapabilityRequestDraft {
                            normalized_need: format!(
                                "rebuild {} after dependency departure",
                                updated.artifact.id
                            ),
                            inputs: updated.artifact.interface.inputs.clone(),
                            output: updated.artifact.interface.output.clone(),
                            allowed_effects: updated.artifact.effects.clone(),
                            requester: "steward".into(),
                            reason: CapabilityReason::DependencyDeparture,
                            evidence_refs: vec![format!("departure:{departed}")],
                        },
                    )
                    .map_err(artifact_error)?;
                requests.push(request.request_id);
            }
        }
        Ok(DependencyCascade {
            dependency: dependency_pin.into(),
            demoted,
            capability_requests: requests,
        })
    }

    /// Conservatively returns promoted machine-built artifacts to canary when their immutable
    /// kernel pin differs from the running runtime. Human/vendor artifacts are not described as
    /// learned evidence and are therefore left for explicit operator migration.
    pub fn steward_kernel_migration(&self, tenant: &str) -> Result<KernelMigration, RuntimeError> {
        let current = env!("CARGO_PKG_VERSION").to_owned();
        let mut artifacts =
            ArtifactRepository::open(self.inner.store.clone()).map_err(artifact_error)?;
        let mut candidates = artifacts.list(tenant, 1_000).map_err(artifact_error)?;
        candidates.sort_by_key(|record| record.artifact.key());
        let mut demoted = Vec::new();
        for candidate in candidates {
            if candidate.status != ArtifactStatus::Promoted
                || candidate.artifact.provenance.built_by.is_none()
                || candidate.artifact.kernel_version == current
            {
                continue;
            }
            let pin = candidate.artifact.key();
            artifacts
                .transition(
                    tenant,
                    &candidate.artifact.id,
                    candidate.artifact.version,
                    ArtifactTransition {
                        expected: ArtifactStatus::Promoted,
                        trigger: ArtifactTrigger::KernelBump,
                        actor: ArtifactActor::System,
                        evidence_snapshot_hash: Some(candidate.artifact.hash.clone()),
                    },
                )
                .map_err(artifact_error)?;
            demoted.push(pin);
        }
        Ok(KernelMigration {
            kernel_version: current,
            demoted,
        })
    }

    pub fn steward_propose_promotion<T: Serialize>(
        &self,
        tenant: &str,
        artifact_pin: &str,
        evidence: &T,
    ) -> Result<PromotionProposal, RuntimeError> {
        let evidence = serde_json::to_value(evidence)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        let evidence_hash = value_hash(
            &aelio_kernel::json_from(&evidence)
                .map_err(|error| RuntimeError::Invalid(error.to_string()))?,
        );
        PromotionProposalRepository::open(self.inner.store.clone())
            .map_err(artifact_error)?
            .propose(tenant, artifact_pin, &evidence_hash)
            .map_err(artifact_error)
    }
}

fn artifact_depends_on(artifact: &crate::Artifact, needle: &str) -> bool {
    let pins: &ArtifactPins = &artifact.pins;
    [
        &pins.targets,
        &pins.artifacts,
        &pins.prompts,
        &pins.models,
        &pins.embeddings,
        &pins.converters,
        &pins.datasets,
    ]
    .into_iter()
    .any(|group| group.iter().any(|pin| pin == needle))
        || artifact.interface.output == needle
        || artifact
            .interface
            .inputs
            .iter()
            .any(|input| input.imprint == needle)
}
