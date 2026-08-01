//! Deduplicated build demand and immutable promotion proposals. Neither record is executable and
//! neither can alter lifecycle state by itself.

use super::{
    decode, encode, ensure_repository_schema, json_to_sol, now_ms, store_error, validate_hash,
    validate_id, validate_pin, validate_tenant, ArtifactEffect, ArtifactError, ArtifactInput,
};
use aelio_sol::value_hash;
use aelio_store::{PutIfAbsent, Store, StoreError};
use serde::{Deserialize, Serialize};

pub const CAPABILITY_REQUEST_TABLE: &str = "capability_requests";
pub const PROMOTION_PROPOSAL_TABLE: &str = "promotion_proposals";
const SCHEMA_VERSION: u32 = 1;
const MAX_EVIDENCE_REFS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityReason {
    MissingCapability,
    UnknownTarget,
    InterfaceNotClosed,
    InefficientArtifact,
    DependencyDeparture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Open,
    Queued,
    Failed,
    Resolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRequestDraft {
    pub normalized_need: String,
    pub inputs: Vec<ArtifactInput>,
    pub output: String,
    pub allowed_effects: Vec<ArtifactEffect>,
    pub requester: String,
    pub reason: CapabilityReason,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRequest {
    pub request_id: String,
    pub need_hash: String,
    #[serde(flatten)]
    pub draft: CapabilityRequestDraft,
    pub demand_count: u64,
    pub status: CapabilityStatus,
    pub build_id: Option<String>,
    #[serde(default)]
    pub build_attempts: Vec<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone)]
pub struct CapabilityRequestRepository<S: Store + Clone> {
    store: S,
}

impl<S: Store + Clone> CapabilityRequestRepository<S> {
    pub fn open(mut store: S) -> Result<Self, ArtifactError> {
        ensure_repository_schema(&mut store, CAPABILITY_REQUEST_TABLE, SCHEMA_VERSION)?;
        Ok(Self { store })
    }

    pub fn record(
        &mut self,
        tenant: &str,
        draft: CapabilityRequestDraft,
    ) -> Result<CapabilityRequest, ArtifactError> {
        validate_tenant(tenant)?;
        validate_draft(&draft)?;
        let need_hash = value_hash(&json_to_sol(
            &serde_json::to_value(serde_json::json!({
                "need": draft.normalized_need,
                "reason": draft.reason,
            }))
            .map_err(|error| ArtifactError::Invalid(error.to_string()))?,
        )?);
        let request_id = format!("capability.{}", &need_hash[..32]);
        let now = now_ms()?;
        let fresh = CapabilityRequest {
            request_id: request_id.clone(),
            need_hash,
            draft,
            demand_count: 1,
            status: CapabilityStatus::Open,
            build_id: None,
            build_attempts: Vec::new(),
            created_at_ms: now,
            updated_at_ms: now,
        };
        validate_request(&fresh)?;
        match self
            .store
            .put_if_absent(
                tenant,
                CAPABILITY_REQUEST_TABLE,
                &request_id,
                encode(&fresh)?,
            )
            .map_err(store_error)?
        {
            PutIfAbsent::Inserted { .. } => Ok(fresh),
            PutIfAbsent::Existing(mut row) => {
                for _ in 0..16 {
                    let mut current: CapabilityRequest = decode(&row.value)?;
                    validate_request(&current)?;
                    if current.draft.normalized_need != fresh.draft.normalized_need
                        || current.draft.reason != fresh.draft.reason
                    {
                        return Err(ArtifactError::Conflict(
                            "capability request hash collision".into(),
                        ));
                    }
                    current.demand_count = current
                        .demand_count
                        .checked_add(1)
                        .ok_or_else(|| ArtifactError::Invalid("demand counter exhausted".into()))?;
                    if matches!(
                        current.status,
                        CapabilityStatus::Resolved | CapabilityStatus::Failed
                    ) {
                        current.status = CapabilityStatus::Open;
                        current.build_id = None;
                    }
                    current.updated_at_ms = now_ms()?;
                    match self.store.cas(
                        tenant,
                        CAPABILITY_REQUEST_TABLE,
                        &request_id,
                        row.version,
                        encode(&current)?,
                    ) {
                        Ok(_) => return Ok(current),
                        Err(StoreError::Conflict) => {
                            row = self
                                .store
                                .get(tenant, CAPABILITY_REQUEST_TABLE, &request_id)
                                .map_err(store_error)?
                                .ok_or_else(|| ArtifactError::NotFound(request_id.clone()))?;
                        }
                        Err(error) => return Err(store_error(error)),
                    }
                }
                Err(ArtifactError::Conflict(
                    "capability demand remained contended after 16 CAS attempts".into(),
                ))
            }
        }
    }

    pub fn list_open(
        &self,
        tenant: &str,
        limit: usize,
    ) -> Result<Vec<CapabilityRequest>, ArtifactError> {
        validate_tenant(tenant)?;
        if limit == 0 || limit > 1_000 {
            return Err(ArtifactError::Invalid(
                "demand limit must be 1..=1000".into(),
            ));
        }
        let mut rows: Vec<_> = self
            .store
            .scan_prefix(tenant, CAPABILITY_REQUEST_TABLE, "", limit)
            .map_err(store_error)?
            .into_iter()
            .map(|(_, row)| decode::<CapabilityRequest>(&row.value))
            .collect::<Result<_, _>>()?;
        rows.retain(|request| request.status == CapabilityStatus::Open);
        rows.sort_by(|left, right| {
            right
                .demand_count
                .cmp(&left.demand_count)
                .then_with(|| left.request_id.cmp(&right.request_id))
        });
        rows.truncate(limit);
        Ok(rows)
    }

    pub fn get(
        &self,
        tenant: &str,
        request_id: &str,
    ) -> Result<Option<CapabilityRequest>, ArtifactError> {
        validate_tenant(tenant)?;
        validate_id(request_id)?;
        self.store
            .get(tenant, CAPABILITY_REQUEST_TABLE, request_id)
            .map_err(store_error)?
            .map(|row| {
                let request: CapabilityRequest = decode(&row.value)?;
                validate_request(&request)?;
                Ok(request)
            })
            .transpose()
    }

    /// Atomically bind an open demand to its content-addressed build. Retrying the same binding is
    /// idempotent; attempting to redirect a queued demand to another build fails closed.
    pub fn queue(
        &mut self,
        tenant: &str,
        request_id: &str,
        build_id: &str,
    ) -> Result<CapabilityRequest, ArtifactError> {
        validate_id(build_id)?;
        self.transition(tenant, request_id, CapabilityStatus::Queued, Some(build_id))
    }

    /// Resolve only the build already bound by `queue`; callers cannot mark an unbuilt open demand
    /// resolved or substitute another build id.
    pub fn resolve(
        &mut self,
        tenant: &str,
        request_id: &str,
        build_id: &str,
    ) -> Result<CapabilityRequest, ArtifactError> {
        validate_id(build_id)?;
        self.transition(
            tenant,
            request_id,
            CapabilityStatus::Resolved,
            Some(build_id),
        )
    }

    pub fn fail(
        &mut self,
        tenant: &str,
        request_id: &str,
        build_id: &str,
    ) -> Result<CapabilityRequest, ArtifactError> {
        validate_id(build_id)?;
        self.transition(tenant, request_id, CapabilityStatus::Failed, Some(build_id))
    }

    pub fn find_by_build(
        &self,
        tenant: &str,
        build_id: &str,
        limit: usize,
    ) -> Result<Vec<CapabilityRequest>, ArtifactError> {
        validate_tenant(tenant)?;
        validate_id(build_id)?;
        if limit == 0 || limit > 1_000 {
            return Err(ArtifactError::Invalid(
                "demand scan limit must be 1..=1000".into(),
            ));
        }
        self.store
            .scan_prefix(tenant, CAPABILITY_REQUEST_TABLE, "", limit)
            .map_err(store_error)?
            .into_iter()
            .map(|(_, row)| decode::<CapabilityRequest>(&row.value))
            .filter_map(|request| match request {
                Ok(request) if request.build_id.as_deref() == Some(build_id) => Some(Ok(request)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    fn transition(
        &mut self,
        tenant: &str,
        request_id: &str,
        target: CapabilityStatus,
        build_id: Option<&str>,
    ) -> Result<CapabilityRequest, ArtifactError> {
        validate_tenant(tenant)?;
        validate_id(request_id)?;
        let mut row = self
            .store
            .get(tenant, CAPABILITY_REQUEST_TABLE, request_id)
            .map_err(store_error)?
            .ok_or_else(|| ArtifactError::NotFound(request_id.into()))?;
        for _ in 0..16 {
            let mut current: CapabilityRequest = decode(&row.value)?;
            validate_request(&current)?;
            if current.status == target && current.build_id.as_deref() == build_id {
                return Ok(current);
            }
            let legal = matches!(
                (current.status, target),
                (CapabilityStatus::Open, CapabilityStatus::Queued)
                    | (CapabilityStatus::Failed, CapabilityStatus::Queued)
                    | (CapabilityStatus::Queued, CapabilityStatus::Failed)
                    | (CapabilityStatus::Queued, CapabilityStatus::Resolved)
            );
            if !legal {
                return Err(ArtifactError::IllegalTransition(format!(
                    "capability request {:?} -> {:?}",
                    current.status, target
                )));
            }
            if current.status == CapabilityStatus::Queued && current.build_id.as_deref() != build_id
            {
                return Err(ArtifactError::Conflict(
                    "queued capability request is bound to another build".into(),
                ));
            }
            if target == CapabilityStatus::Queued {
                let build_id = build_id.expect("queued transition validated build id");
                if !current
                    .build_attempts
                    .iter()
                    .any(|attempt| attempt == build_id)
                {
                    if current.build_attempts.len() >= 32 {
                        return Err(ArtifactError::Invalid(
                            "capability request exceeds 32 build attempts".into(),
                        ));
                    }
                    current.build_attempts.push(build_id.to_owned());
                }
            }
            current.status = target;
            current.build_id = build_id.map(str::to_owned);
            current.updated_at_ms = now_ms()?;
            match self.store.cas(
                tenant,
                CAPABILITY_REQUEST_TABLE,
                request_id,
                row.version,
                encode(&current)?,
            ) {
                Ok(_) => return Ok(current),
                Err(StoreError::Conflict) => {
                    row = self
                        .store
                        .get(tenant, CAPABILITY_REQUEST_TABLE, request_id)
                        .map_err(store_error)?
                        .ok_or_else(|| ArtifactError::NotFound(request_id.into()))?;
                }
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(ArtifactError::Conflict(
            "capability request transition remained contended after 16 CAS attempts".into(),
        ))
    }
}

fn validate_draft(draft: &CapabilityRequestDraft) -> Result<(), ArtifactError> {
    if draft.normalized_need.trim().is_empty() || draft.normalized_need.len() > 500 {
        return Err(ArtifactError::Invalid(
            "normalized need must be 1..=500 bytes".into(),
        ));
    }
    validate_pin(&draft.output)?;
    validate_id(&draft.requester)?;
    if draft.inputs.len() > 256
        || draft.allowed_effects.len() > 4
        || draft.evidence_refs.len() > MAX_EVIDENCE_REFS
        || draft
            .evidence_refs
            .iter()
            .any(|reference| reference.is_empty() || reference.len() > 256)
    {
        return Err(ArtifactError::Invalid(
            "capability request exceeds bounds".into(),
        ));
    }
    let mut effects = std::collections::HashSet::new();
    if draft
        .allowed_effects
        .iter()
        .any(|effect| !effects.insert(*effect))
    {
        return Err(ArtifactError::Invalid(
            "allowed effects must be unique".into(),
        ));
    }
    Ok(())
}

fn validate_request(request: &CapabilityRequest) -> Result<(), ArtifactError> {
    validate_draft(&request.draft)?;
    validate_id(&request.request_id)?;
    validate_hash("need_hash", &request.need_hash)?;
    if request.demand_count == 0
        || request.created_at_ms > request.updated_at_ms
        || request.build_attempts.len() > 32
        || request
            .build_attempts
            .iter()
            .any(|build_id| validate_id(build_id).is_err())
    {
        return Err(ArtifactError::Invalid(
            "invalid capability request counters".into(),
        ));
    }
    match request.status {
        CapabilityStatus::Open if request.build_id.is_some() => {
            return Err(ArtifactError::Invalid(
                "open capability request cannot carry a build id".into(),
            ))
        }
        CapabilityStatus::Queued | CapabilityStatus::Failed | CapabilityStatus::Resolved
            if request.build_id.is_none() =>
        {
            return Err(ArtifactError::Invalid(
                "queued/resolved capability request requires a build id".into(),
            ))
        }
        _ => {}
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionProposalStatus {
    Pending,
    Applied,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionProposal {
    pub proposal_id: String,
    pub artifact_pin: String,
    pub evidence_hash: String,
    pub status: PromotionProposalStatus,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone)]
pub struct PromotionProposalRepository<S: Store + Clone> {
    store: S,
}

impl<S: Store + Clone> PromotionProposalRepository<S> {
    pub fn open(mut store: S) -> Result<Self, ArtifactError> {
        ensure_repository_schema(&mut store, PROMOTION_PROPOSAL_TABLE, SCHEMA_VERSION)?;
        Ok(Self { store })
    }

    pub fn propose(
        &mut self,
        tenant: &str,
        artifact_pin: &str,
        evidence_hash: &str,
    ) -> Result<PromotionProposal, ArtifactError> {
        validate_tenant(tenant)?;
        validate_pin(artifact_pin)?;
        validate_hash("evidence_hash", evidence_hash)?;
        let proposal_id = format!(
            "promotion.{}.{}",
            artifact_pin.replace('@', "."),
            &evidence_hash[..16]
        );
        validate_id(&proposal_id)?;
        let proposal = PromotionProposal {
            proposal_id: proposal_id.clone(),
            artifact_pin: artifact_pin.into(),
            evidence_hash: evidence_hash.into(),
            status: PromotionProposalStatus::Pending,
            created_at_ms: now_ms()?,
            updated_at_ms: now_ms()?,
        };
        match self
            .store
            .put_if_absent(
                tenant,
                PROMOTION_PROPOSAL_TABLE,
                &proposal_id,
                encode(&proposal)?,
            )
            .map_err(store_error)?
        {
            PutIfAbsent::Inserted { .. } => Ok(proposal),
            PutIfAbsent::Existing(row) => {
                let existing: PromotionProposal = decode(&row.value)?;
                if existing.artifact_pin == proposal.artifact_pin
                    && existing.evidence_hash == proposal.evidence_hash
                {
                    Ok(existing)
                } else {
                    Err(ArtifactError::Conflict(
                        "promotion proposal collision".into(),
                    ))
                }
            }
        }
    }

    pub fn get(
        &self,
        tenant: &str,
        proposal_id: &str,
    ) -> Result<Option<PromotionProposal>, ArtifactError> {
        validate_tenant(tenant)?;
        validate_id(proposal_id)?;
        self.store
            .get(tenant, PROMOTION_PROPOSAL_TABLE, proposal_id)
            .map_err(store_error)?
            .map(|row| decode(&row.value))
            .transpose()
    }

    pub fn finish(
        &mut self,
        tenant: &str,
        proposal_id: &str,
        status: PromotionProposalStatus,
    ) -> Result<PromotionProposal, ArtifactError> {
        validate_tenant(tenant)?;
        validate_id(proposal_id)?;
        if status == PromotionProposalStatus::Pending {
            return Err(ArtifactError::Invalid(
                "promotion proposal finish status must be applied or rejected".into(),
            ));
        }
        for _ in 0..16 {
            let row = self
                .store
                .get(tenant, PROMOTION_PROPOSAL_TABLE, proposal_id)
                .map_err(store_error)?
                .ok_or_else(|| ArtifactError::NotFound(proposal_id.into()))?;
            let mut proposal: PromotionProposal = decode(&row.value)?;
            if proposal.status == status {
                return Ok(proposal);
            }
            if proposal.status != PromotionProposalStatus::Pending {
                return Err(ArtifactError::Conflict(
                    "promotion proposal already has a different terminal status".into(),
                ));
            }
            proposal.status = status;
            proposal.updated_at_ms = now_ms()?;
            match self.store.cas(
                tenant,
                PROMOTION_PROPOSAL_TABLE,
                proposal_id,
                row.version,
                encode(&proposal)?,
            ) {
                Ok(_) => return Ok(proposal),
                Err(StoreError::Conflict) => continue,
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(ArtifactError::Conflict(
            "promotion proposal CAS contention exceeded retry bound".into(),
        ))
    }
}
