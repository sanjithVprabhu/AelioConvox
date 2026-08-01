//! Canonical, immutable artifacts and their atomic lifecycle record.
//!
//! Executable identity and mutable operational state are deliberately separated. The artifact
//! hash covers every field that can change execution or admission. Lifecycle status and history
//! share one CAS-updated row, so a crash cannot expose a status without its audit event.

use aelio_convert::{transition as lifecycle_transition, Status, Trigger};
use aelio_sol::{value_hash, Limits, SolValue};
use aelio_store::{PutIfAbsent, Store, StoreError};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

pub mod build;
pub mod coordination;
pub mod demand;
pub mod evidence;
pub mod harness;
pub mod imprint;
pub mod job;
pub use build::{
    BuildBudget, BuildCost, BuildExample, BuildFailure, BuildPolicy, BuildResult, BuildScope,
    BuildSpec, BuildSpecDraft,
};
pub use coordination::{
    LeaseRecord, LeaseRequest, LeaseStatus, NameLeaseRepository, VerificationCacheEntry,
    VerificationCacheRepository, VerificationVerdict, NAME_LEASE_TABLE, VERIFICATION_CACHE_TABLE,
};
pub use demand::{
    CapabilityReason, CapabilityRequest, CapabilityRequestDraft, CapabilityRequestRepository,
    CapabilityStatus, PromotionProposal, PromotionProposalRepository, PromotionProposalStatus,
    CAPABILITY_REQUEST_TABLE, PROMOTION_PROPOSAL_TABLE,
};
pub use evidence::{
    ArtifactEvidence, ArtifactEvidenceDraft, ArtifactEvidenceRepository, EvidencePhase,
    EvidenceSummary, ARTIFACT_EVIDENCE_TABLE, MAX_EVIDENCE_PER_ARTIFACT_PHASE,
};
pub use harness::{
    DecompositionSeam, HarnessBody, HarnessNode, HarnessSeam, HarnessSink, HarnessSource,
};
pub use imprint::{
    base_vendor_imprints, ImprintDeclaration, ImprintField, ImprintRegistry, ImprintSensitivity,
    ImprintType,
};
pub use job::{
    BuildBudgetAccount, BuildBudgetCharge, BuildBudgetRepository, BuildJob, BuildJobRecord,
    BuildJobRepository, BuildLineage, BuildReaction, BuildReactionKind, BuildReactionStatus,
    BuildStage, BuildWorkspace, BUILD_BUDGET_TABLE, BUILD_JOB_TABLE,
};

pub const ARTIFACT_TABLE: &str = "artifacts";
pub const REPOSITORY_SCHEMA_TABLE: &str = "repository_schema";
pub const REPOSITORY_SCHEMA_TENANT: &str = "aelio.system";
pub const ARTIFACT_SCHEMA_VERSION: u32 = 1;
const MAX_ID_BYTES: usize = 192;
const MAX_DESCRIPTION_BYTES: usize = 16 * 1024;
const MAX_TAGS: usize = 128;
const MAX_EFFECTS: usize = 4;
const MAX_PINS_PER_CLASS: usize = 1_024;
const MAX_INPUTS: usize = 256;
const MAX_EXAMPLES: usize = 256;
const MAX_HISTORY_EVENTS: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactClass {
    Flow,
    Harness,
    Prompt,
    DerivedOp,
    Glu,
    Imprint,
    Dataset,
    Pathway,
    Procedure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactEffect {
    Pure,
    Read,
    Write,
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactTier {
    Auto,
    Reviewed,
    Locked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactStatus {
    Proposed,
    Rejected,
    Shadow,
    Canary,
    Promoted,
    Retired,
}

impl From<ArtifactStatus> for Status {
    fn from(value: ArtifactStatus) -> Self {
        match value {
            ArtifactStatus::Proposed => Self::Proposed,
            ArtifactStatus::Rejected => Self::Rejected,
            ArtifactStatus::Shadow => Self::Shadow,
            ArtifactStatus::Canary => Self::Canary,
            ArtifactStatus::Promoted => Self::Promoted,
            ArtifactStatus::Retired => Self::Retired,
        }
    }
}

impl From<Status> for ArtifactStatus {
    fn from(value: Status) -> Self {
        match value {
            Status::Proposed => Self::Proposed,
            Status::Rejected => Self::Rejected,
            Status::Shadow => Self::Shadow,
            Status::Canary => Self::Canary,
            Status::Promoted => Self::Promoted,
            Status::Retired => Self::Retired,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactActor {
    System,
    Deployer(String),
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArtifactTrigger {
    ProposalEmitted,
    VendorInstall,
    StructuralPass,
    StructuralFail {
        reason: String,
        rules_hash: String,
    },
    ShadowThresholdsMet {
        distinct_inputs: u32,
        validation_rate: f64,
        approved: bool,
    },
    CanaryThresholdsMet {
        distinct_inputs: u32,
        downstream_success: f64,
        guard_violations: u32,
    },
    AttributedFailure {
        failure_rate: f64,
        guard_violation: bool,
    },
    DemoteManual,
    Invalidated,
    KernelBump,
    RetireManual,
    ThreeDemotions,
    UnusedWindow,
    ManualRevival,
}

impl ArtifactTrigger {
    fn validate_and_lower(&self) -> Result<Trigger, ArtifactError> {
        let rate = |name: &str, value: f64| {
            if value.is_finite() && (0.0..=1.0).contains(&value) {
                Ok(())
            } else {
                Err(ArtifactError::Invalid(format!(
                    "{name} must be finite and within 0..=1"
                )))
            }
        };
        Ok(match self {
            Self::ProposalEmitted => {
                return Err(ArtifactError::IllegalTransition(
                    "proposal_emitted is valid only for record creation".into(),
                ));
            }
            Self::VendorInstall => Trigger::VendorInstall,
            Self::StructuralPass | Self::ManualRevival => Trigger::StructuralPass,
            Self::StructuralFail { reason, rules_hash } => {
                if reason.trim().is_empty() || rules_hash.trim().is_empty() {
                    return Err(ArtifactError::Invalid(
                        "structural failure requires reason and rules_hash".into(),
                    ));
                }
                Trigger::StructuralFail
            }
            Self::ShadowThresholdsMet {
                distinct_inputs,
                validation_rate,
                approved,
            } => {
                rate("validation_rate", *validation_rate)?;
                if *distinct_inputs < 20 || *validation_rate < 0.95 {
                    return Err(ArtifactError::Invalid(
                        "shadow thresholds require >=20 distinct inputs and >=0.95 validation"
                            .into(),
                    ));
                }
                Trigger::ShadowThresholdsMet {
                    approved: *approved,
                }
            }
            Self::CanaryThresholdsMet {
                distinct_inputs,
                downstream_success,
                guard_violations,
            } => {
                rate("downstream_success", *downstream_success)?;
                if *distinct_inputs < 20 || *downstream_success < 0.98 || *guard_violations != 0 {
                    return Err(ArtifactError::Invalid(
                        "canary thresholds require >=20 distinct inputs, >=0.98 success, and zero Guard violations"
                            .into(),
                    ));
                }
                Trigger::CanaryThresholdsMet
            }
            Self::AttributedFailure {
                failure_rate,
                guard_violation,
            } => {
                rate("failure_rate", *failure_rate)?;
                if *failure_rate <= 0.02 && !guard_violation {
                    return Err(ArtifactError::Invalid(
                        "attributed failure requires >0.02 failure or a Guard violation".into(),
                    ));
                }
                Trigger::AttributedFailure
            }
            Self::DemoteManual => Trigger::DemoteManual,
            Self::Invalidated => Trigger::Invalidated,
            Self::KernelBump => Trigger::KernelBump,
            Self::RetireManual => Trigger::RetireManual,
            Self::ThreeDemotions => Trigger::ThreeDemotions,
            Self::UnusedWindow => Trigger::UnusedWindow,
        })
    }

    fn requires_deployer(&self) -> bool {
        matches!(
            self,
            Self::DemoteManual | Self::RetireManual | Self::ManualRevival
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactInput {
    pub name: String,
    pub imprint: String,
    pub required: bool,
    pub sensitivity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactInterface {
    pub inputs: Vec<ArtifactInput>,
    pub output: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPins {
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub artifacts: Vec<String>,
    #[serde(default)]
    pub prompts: Vec<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub embeddings: Vec<String>,
    #[serde(default)]
    pub converters: Vec<String>,
    #[serde(default)]
    pub datasets: Vec<String>,
}

impl ArtifactPins {
    fn groups(&self) -> [&[String]; 7] {
        [
            &self.targets,
            &self.artifacts,
            &self.prompts,
            &self.models,
            &self.embeddings,
            &self.converters,
            &self.datasets,
        ]
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    #[serde(default)]
    pub built_by: Option<String>,
    #[serde(default)]
    pub requester: Option<String>,
    #[serde(default)]
    pub build_id: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub id: String,
    pub version: u32,
    pub class: ArtifactClass,
    pub tier: ArtifactTier,
    pub sol_version: String,
    pub kernel_version: String,
    pub interface: ArtifactInterface,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub effects: Vec<ArtifactEffect>,
    pub pins: ArtifactPins,
    pub examples: Vec<serde_json::Value>,
    pub body: serde_json::Value,
    #[serde(default)]
    pub provenance: Provenance,
    pub hash: String,
}

impl Artifact {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        version: u32,
        class: ArtifactClass,
        tier: ArtifactTier,
        sol_version: impl Into<String>,
        kernel_version: impl Into<String>,
        interface: ArtifactInterface,
        description: impl Into<String>,
        tags: Vec<String>,
        effects: Vec<ArtifactEffect>,
        pins: ArtifactPins,
        examples: Vec<serde_json::Value>,
        body: serde_json::Value,
        provenance: Provenance,
    ) -> Result<Self, ArtifactError> {
        let mut artifact = Self {
            id: id.into(),
            version,
            class,
            tier,
            sol_version: sol_version.into(),
            kernel_version: kernel_version.into(),
            interface,
            description: description.into(),
            tags,
            effects,
            pins,
            examples,
            body,
            provenance,
            hash: String::new(),
        };
        artifact.validate_fields()?;
        artifact.hash = artifact.compute_hash()?;
        Ok(artifact)
    }

    pub fn key(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }

    pub fn compute_hash(&self) -> Result<String, ArtifactError> {
        let identity = serde_json::json!({
            "id": self.id,
            "version": self.version,
            "class": self.class,
            "tier": self.tier,
            "sol_version": self.sol_version,
            "kernel_version": self.kernel_version,
            "interface": self.interface,
            "effects": self.effects,
            "pins": self.pins,
            "examples": self.examples,
            "body": self.body,
        });
        Ok(value_hash(&json_to_sol(&identity)?))
    }

    pub fn validate(&self) -> Result<(), ArtifactError> {
        self.validate_fields()?;
        let actual = self.compute_hash()?;
        if self.hash != actual {
            return Err(ArtifactError::HashMismatch {
                expected: actual,
                actual: self.hash.clone(),
            });
        }
        Ok(())
    }

    fn validate_fields(&self) -> Result<(), ArtifactError> {
        validate_id(&self.id)?;
        if self.version == 0 {
            return Err(ArtifactError::Invalid(
                "artifact version must be positive".into(),
            ));
        }
        for (name, value) in [
            ("sol_version", self.sol_version.as_str()),
            ("kernel_version", self.kernel_version.as_str()),
        ] {
            if value.is_empty() || value.len() > MAX_ID_BYTES {
                return Err(ArtifactError::Invalid(format!("{name} is invalid")));
            }
        }
        if self.description.len() > MAX_DESCRIPTION_BYTES {
            return Err(ArtifactError::Invalid(
                "artifact description is too large".into(),
            ));
        }
        validate_unique_strings("tags", &self.tags, MAX_TAGS, false)?;
        if self.effects.is_empty() || self.effects.len() > MAX_EFFECTS {
            return Err(ArtifactError::Invalid(
                "effects must contain 1..=4 values".into(),
            ));
        }
        let mut effects = HashSet::new();
        if self.effects.iter().any(|effect| !effects.insert(*effect)) {
            return Err(ArtifactError::Invalid("effects must be unique".into()));
        }
        if self.effects.contains(&ArtifactEffect::Pure) && self.effects.len() != 1 {
            return Err(ArtifactError::Invalid(
                "pure cannot be combined with effectful classes".into(),
            ));
        }
        if self.interface.inputs.len() > MAX_INPUTS {
            return Err(ArtifactError::Invalid("too many interface inputs".into()));
        }
        let mut input_names = HashSet::new();
        for input in &self.interface.inputs {
            validate_id(&input.name)?;
            validate_pin(&input.imprint)?;
            if input.sensitivity.is_empty() || input.sensitivity.len() > 64 {
                return Err(ArtifactError::Invalid(
                    "input sensitivity is invalid".into(),
                ));
            }
            if !matches!(
                input.sensitivity.as_str(),
                "public" | "internal" | "pii" | "secret"
            ) {
                return Err(ArtifactError::Invalid(
                    "input sensitivity must be public|internal|pii|secret".into(),
                ));
            }
            if !input_names.insert(input.name.as_str()) {
                return Err(ArtifactError::Invalid("input names must be unique".into()));
            }
        }
        validate_pin(&self.interface.output)?;
        let mut all_pins = HashSet::new();
        for group in self.pins.groups() {
            validate_unique_strings("pin group", group, MAX_PINS_PER_CLASS, true)?;
            for pin in group {
                validate_pin(pin)?;
                if !all_pins.insert(pin.as_str()) {
                    return Err(ArtifactError::Invalid(
                        "a pin may occur only once across pin classes".into(),
                    ));
                }
            }
        }
        if self.examples.len() > MAX_EXAMPLES {
            return Err(ArtifactError::Invalid("too many artifact examples".into()));
        }
        let bounded = serde_json::json!({"examples": self.examples, "body": self.body});
        Limits::default()
            .check(&json_to_sol(&bounded)?)
            .map_err(|error| ArtifactError::Invalid(error.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleEvent {
    pub seq: u32,
    pub ts_ms: i64,
    pub from: Option<ArtifactStatus>,
    pub to: ArtifactStatus,
    pub trigger: ArtifactTrigger,
    pub actor: ArtifactActor,
    pub evidence_snapshot_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRecord {
    pub artifact: Artifact,
    pub status: ArtifactStatus,
    pub history: Vec<LifecycleEvent>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactTransition {
    pub expected: ArtifactStatus,
    pub trigger: ArtifactTrigger,
    pub actor: ArtifactActor,
    pub evidence_snapshot_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactError {
    Invalid(String),
    HashMismatch { expected: String, actual: String },
    Conflict(String),
    NotFound(String),
    IllegalTransition(String),
    Store(String),
    Corrupt(String),
}

impl std::fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(detail) => write!(formatter, "invalid artifact: {detail}"),
            Self::HashMismatch { expected, actual } => {
                write!(
                    formatter,
                    "artifact hash mismatch: expected {expected}, got {actual}"
                )
            }
            Self::Conflict(detail) => write!(formatter, "artifact conflict: {detail}"),
            Self::NotFound(key) => write!(formatter, "artifact not found: {key}"),
            Self::IllegalTransition(detail) => write!(formatter, "illegal transition: {detail}"),
            Self::Store(detail) => write!(formatter, "artifact store: {detail}"),
            Self::Corrupt(detail) => write!(formatter, "corrupt artifact: {detail}"),
        }
    }
}

impl std::error::Error for ArtifactError {}

#[derive(Clone)]
pub struct ArtifactRepository<S: Store + Clone> {
    store: S,
}

impl<S: Store + Clone> ArtifactRepository<S> {
    pub fn open(mut store: S) -> Result<Self, ArtifactError> {
        ensure_repository_schema(&mut store, ARTIFACT_TABLE, ARTIFACT_SCHEMA_VERSION)?;
        Ok(Self { store })
    }

    pub fn put_proposed(
        &mut self,
        tenant: &str,
        artifact: Artifact,
        actor: ArtifactActor,
    ) -> Result<ArtifactRecord, ArtifactError> {
        validate_tenant(tenant)?;
        artifact.validate()?;
        let key = artifact.key();
        let record = ArtifactRecord {
            artifact,
            status: ArtifactStatus::Proposed,
            history: vec![LifecycleEvent {
                seq: 1,
                ts_ms: now_ms()?,
                from: None,
                to: ArtifactStatus::Proposed,
                trigger: ArtifactTrigger::ProposalEmitted,
                actor,
                evidence_snapshot_hash: None,
            }],
        };
        let encoded = encode(&record)?;
        match self
            .store
            .put_if_absent(tenant, ARTIFACT_TABLE, &key, encoded.clone())
            .map_err(store_error)?
        {
            PutIfAbsent::Inserted { .. } => Ok(record),
            PutIfAbsent::Existing(existing) => {
                let existing: ArtifactRecord = decode(&existing.value)?;
                existing.artifact.validate()?;
                validate_history(&existing)?;
                if existing.artifact == record.artifact {
                    Ok(existing)
                } else {
                    Err(ArtifactError::Conflict(format!(
                        "{key} already exists with different immutable content"
                    )))
                }
            }
        }
    }

    /// Install a stock artifact from the versioned vendor namespace. This is the sole gate-exempt
    /// admission path: callers cannot use it for tenant artifacts, deployer-authored artifacts, or
    /// records that do not explicitly carry `provenance.metadata.origin = "vendor"`.
    pub fn put_vendor_promoted(
        &mut self,
        tenant: &str,
        artifact: Artifact,
    ) -> Result<ArtifactRecord, ArtifactError> {
        if tenant != "aelio.vendor"
            || artifact
                .provenance
                .metadata
                .get("origin")
                .and_then(serde_json::Value::as_str)
                != Some("vendor")
        {
            return Err(ArtifactError::Invalid(
                "gate-exempt install is restricted to origin=vendor artifacts in aelio.vendor"
                    .into(),
            ));
        }
        let mut record = self.put_proposed(tenant, artifact, ArtifactActor::System)?;
        if record.status == ArtifactStatus::Proposed {
            record = self.transition(
                tenant,
                &record.artifact.id.clone(),
                record.artifact.version,
                ArtifactTransition {
                    expected: ArtifactStatus::Proposed,
                    trigger: ArtifactTrigger::VendorInstall,
                    actor: ArtifactActor::System,
                    evidence_snapshot_hash: Some(record.artifact.hash.clone()),
                },
            )?;
        }
        if record.status != ArtifactStatus::Promoted {
            return Err(ArtifactError::Conflict(format!(
                "vendor artifact {} is {:?}, expected promoted",
                record.artifact.key(),
                record.status
            )));
        }
        Ok(record)
    }

    pub fn get(
        &self,
        tenant: &str,
        id: &str,
        version: u32,
    ) -> Result<Option<ArtifactRecord>, ArtifactError> {
        validate_tenant(tenant)?;
        validate_id(id)?;
        if version == 0 {
            return Err(ArtifactError::Invalid(
                "artifact version must be positive".into(),
            ));
        }
        self.get_key(tenant, &format!("{id}@{version}"))
            .map(|row| row.map(|(record, _)| record))
    }

    /// Bounded key-ordered tenant scan for deterministic builder resolution. Callers must apply
    /// class/interface/effect/trust filters; the repository never returns another tenant's rows.
    pub fn list(&self, tenant: &str, limit: usize) -> Result<Vec<ArtifactRecord>, ArtifactError> {
        validate_tenant(tenant)?;
        if limit == 0 || limit > 1_000 {
            return Err(ArtifactError::Invalid(
                "artifact list limit must be within 1..=1000".into(),
            ));
        }
        self.store
            .scan_prefix(tenant, ARTIFACT_TABLE, "", limit)
            .map_err(store_error)?
            .into_iter()
            .map(|(_, row)| {
                let record: ArtifactRecord = decode(&row.value)?;
                record.artifact.validate()?;
                validate_history(&record)?;
                Ok(record)
            })
            .collect()
    }

    pub fn transition(
        &mut self,
        tenant: &str,
        id: &str,
        version: u32,
        request: ArtifactTransition,
    ) -> Result<ArtifactRecord, ArtifactError> {
        validate_tenant(tenant)?;
        validate_id(id)?;
        if matches!(&request.actor, ArtifactActor::Deployer(id) if id.trim().is_empty() || id.len() > MAX_ID_BYTES)
        {
            return Err(ArtifactError::Invalid(
                "deployer actor id must contain 1..=192 bytes".into(),
            ));
        }
        if request.trigger.requires_deployer()
            && !matches!(request.actor, ArtifactActor::Deployer(_))
        {
            return Err(ArtifactError::IllegalTransition(
                "manual transitions require an identified deployer".into(),
            ));
        }
        if matches!(request.trigger, ArtifactTrigger::VendorInstall)
            && (tenant != "aelio.vendor" || !matches!(request.actor, ArtifactActor::System))
        {
            return Err(ArtifactError::IllegalTransition(
                "vendor_install requires the aelio.vendor namespace and system actor".into(),
            ));
        }
        if let Some(hash) = &request.evidence_snapshot_hash {
            validate_hash("evidence_snapshot_hash", hash)?;
        }
        let lowered = request.trigger.validate_and_lower()?;
        let key = format!("{id}@{version}");
        let (mut record, store_version) = self
            .get_key(tenant, &key)?
            .ok_or_else(|| ArtifactError::NotFound(key.clone()))?;
        if matches!(request.trigger, ArtifactTrigger::VendorInstall)
            && record
                .artifact
                .provenance
                .metadata
                .get("origin")
                .and_then(serde_json::Value::as_str)
                != Some("vendor")
        {
            return Err(ArtifactError::IllegalTransition(
                "vendor_install artifact does not declare provenance origin=vendor".into(),
            ));
        }
        if record.status != request.expected {
            return Err(ArtifactError::Conflict(format!(
                "expected status {:?}, found {:?}",
                request.expected, record.status
            )));
        }
        if matches!(request.trigger, ArtifactTrigger::ManualRevival)
            && record.status != ArtifactStatus::Retired
        {
            return Err(ArtifactError::IllegalTransition(
                "manual revival is valid only from retired".into(),
            ));
        }
        if matches!(request.trigger, ArtifactTrigger::StructuralPass)
            && record.status == ArtifactStatus::Retired
        {
            return Err(ArtifactError::IllegalTransition(
                "retired artifacts require an explicit manual revival".into(),
            ));
        }
        let next = lifecycle_transition(record.status.into(), lowered)
            .map(ArtifactStatus::from)
            .map_err(|detail| ArtifactError::IllegalTransition(detail.into()))?;
        if record.history.len() >= MAX_HISTORY_EVENTS {
            return Err(ArtifactError::Invalid(format!(
                "artifact history exceeds the {MAX_HISTORY_EVENTS}-event bound"
            )));
        }
        let seq = u32::try_from(record.history.len() + 1)
            .map_err(|_| ArtifactError::Invalid("artifact history exhausted".into()))?;
        record.history.push(LifecycleEvent {
            seq,
            ts_ms: now_ms()?,
            from: Some(record.status),
            to: next,
            trigger: request.trigger,
            actor: request.actor,
            evidence_snapshot_hash: request.evidence_snapshot_hash,
        });
        record.status = next;
        self.store
            .cas(
                tenant,
                ARTIFACT_TABLE,
                &key,
                store_version,
                encode(&record)?,
            )
            .map_err(|error| match error {
                StoreError::Conflict => ArtifactError::Conflict(
                    "concurrent lifecycle transition won the compare-and-swap".into(),
                ),
                other => store_error(other),
            })?;
        Ok(record)
    }

    fn get_key(
        &self,
        tenant: &str,
        key: &str,
    ) -> Result<Option<(ArtifactRecord, u64)>, ArtifactError> {
        self.store
            .get(tenant, ARTIFACT_TABLE, key)
            .map_err(store_error)?
            .map(|row| {
                let record: ArtifactRecord = decode(&row.value)?;
                record.artifact.validate()?;
                validate_history(&record)?;
                Ok((record, row.version))
            })
            .transpose()
    }
}

fn validate_history(record: &ArtifactRecord) -> Result<(), ArtifactError> {
    if record.history.is_empty() || record.history.len() > MAX_HISTORY_EVENTS {
        return Err(ArtifactError::Corrupt("artifact history is empty".into()));
    }
    let mut status = None;
    for (index, event) in record.history.iter().enumerate() {
        let expected_seq = u32::try_from(index + 1)
            .map_err(|_| ArtifactError::Corrupt("artifact history exhausted".into()))?;
        if event.seq != expected_seq || event.from != status {
            return Err(ArtifactError::Corrupt(
                "artifact history sequence or chain is invalid".into(),
            ));
        }
        if index == 0 {
            if event.to != ArtifactStatus::Proposed
                || event.trigger != ArtifactTrigger::ProposalEmitted
            {
                return Err(ArtifactError::Corrupt(
                    "artifact history must begin with proposal_emitted -> proposed".into(),
                ));
            }
        } else {
            let from = event.from.ok_or_else(|| {
                ArtifactError::Corrupt("non-initial history event lacks from status".into())
            })?;
            let lowered = event
                .trigger
                .validate_and_lower()
                .map_err(|error| ArtifactError::Corrupt(error.to_string()))?;
            let replayed = lifecycle_transition(from.into(), lowered)
                .map(ArtifactStatus::from)
                .map_err(|detail| ArtifactError::Corrupt(detail.into()))?;
            if replayed != event.to {
                return Err(ArtifactError::Corrupt(
                    "artifact history transition does not replay".into(),
                ));
            }
        }
        status = Some(event.to);
    }
    if status != Some(record.status) {
        return Err(ArtifactError::Corrupt(
            "artifact status does not match history tail".into(),
        ));
    }
    Ok(())
}

fn validate_tenant(tenant: &str) -> Result<(), ArtifactError> {
    if tenant.is_empty() || tenant.len() > MAX_ID_BYTES {
        Err(ArtifactError::Invalid("tenant is invalid".into()))
    } else {
        Ok(())
    }
}

fn validate_id(id: &str) -> Result<(), ArtifactError> {
    let mut bytes = id.bytes();
    let valid_first = bytes.next().is_some_and(|byte| byte.is_ascii_lowercase());
    if !valid_first
        || id.len() > MAX_ID_BYTES
        || !bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
    {
        return Err(ArtifactError::Invalid(format!("invalid identifier `{id}`")));
    }
    Ok(())
}

pub(crate) fn validate_pin(pin: &str) -> Result<(), ArtifactError> {
    let (id, version) = pin
        .rsplit_once('@')
        .ok_or_else(|| ArtifactError::Invalid(format!("unpinned reference `{pin}`")))?;
    validate_id(id)?;
    if version.is_empty()
        || version.starts_with('0')
        || !version.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(ArtifactError::Invalid(format!(
            "reference `{pin}` must end in @<positive integer>"
        )));
    }
    Ok(())
}

fn validate_unique_strings(
    name: &str,
    values: &[String],
    max: usize,
    allow_empty_list: bool,
) -> Result<(), ArtifactError> {
    if values.len() > max || (!allow_empty_list && values.iter().any(String::is_empty)) {
        return Err(ArtifactError::Invalid(format!(
            "{name} is invalid or too large"
        )));
    }
    let mut unique = HashSet::new();
    if values.iter().any(|value| !unique.insert(value.as_str())) {
        return Err(ArtifactError::Invalid(format!("{name} must be unique")));
    }
    Ok(())
}

fn validate_hash(name: &str, hash: &str) -> Result<(), ArtifactError> {
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Err(ArtifactError::Invalid(format!(
            "{name} must be a BLAKE3 hex digest"
        )))
    } else {
        Ok(())
    }
}

pub(super) fn json_to_sol(value: &serde_json::Value) -> Result<SolValue, ArtifactError> {
    aelio_kernel::json_from(value).map_err(|error| ArtifactError::Invalid(error.to_string()))
}

fn encode<T: Serialize>(value: &T) -> Result<SolValue, ArtifactError> {
    let json =
        serde_json::to_value(value).map_err(|error| ArtifactError::Corrupt(error.to_string()))?;
    json_to_sol(&json)
}

fn decode<T: for<'de> Deserialize<'de>>(value: &SolValue) -> Result<T, ArtifactError> {
    let json: serde_json::Value = serde_json::from_str(&aelio_sol::canonical_string(value))
        .map_err(|error| ArtifactError::Corrupt(error.to_string()))?;
    serde_json::from_value(json).map_err(|error| ArtifactError::Corrupt(error.to_string()))
}

fn now_ms() -> Result<i64, ArtifactError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| ArtifactError::Store(error.to_string()))?
        .as_millis();
    i64::try_from(millis).map_err(|_| ArtifactError::Store("system time overflow".into()))
}

fn store_error(error: StoreError) -> ArtifactError {
    ArtifactError::Store(format!("{error:?}"))
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RepositorySchemaHeader {
    component: String,
    schema_version: u32,
    min_reader_version: u32,
}

fn ensure_repository_schema<S: Store>(
    store: &mut S,
    component: &str,
    schema_version: u32,
) -> Result<(), ArtifactError> {
    let header = RepositorySchemaHeader {
        component: component.into(),
        schema_version,
        min_reader_version: schema_version,
    };
    match store
        .put_if_absent(
            REPOSITORY_SCHEMA_TENANT,
            REPOSITORY_SCHEMA_TABLE,
            component,
            encode(&header)?,
        )
        .map_err(store_error)?
    {
        PutIfAbsent::Inserted { .. } => Ok(()),
        PutIfAbsent::Existing(existing) => {
            let found: RepositorySchemaHeader = decode(&existing.value)?;
            if found.component != component {
                return Err(ArtifactError::Corrupt(format!(
                    "schema header key `{component}` contains `{}`",
                    found.component
                )));
            }
            if found.schema_version == schema_version && found.min_reader_version <= schema_version
            {
                Ok(())
            } else if found.schema_version > schema_version
                || found.min_reader_version > schema_version
            {
                Err(ArtifactError::Invalid(format!(
                    "{component} schema {} requires a newer reader than {}",
                    found.schema_version, schema_version
                )))
            } else {
                Err(ArtifactError::Invalid(format!(
                    "{component} schema {} requires an explicit migration to {}",
                    found.schema_version, schema_version
                )))
            }
        }
    }
}
