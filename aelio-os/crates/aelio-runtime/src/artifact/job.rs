//! CAS-backed state for the durable twelve-stage root builder.

use super::{
    decode, encode, ensure_repository_schema, now_ms, store_error, validate_hash, validate_id,
    validate_pin, validate_tenant, ArtifactError, BuildCost, BuildResult, BuildSpec,
    DecompositionSeam, HarnessBody, HarnessSeam,
};
use aelio_store::{PutIfAbsent, Store, StoreError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const BUILD_JOB_TABLE: &str = "build_jobs";
pub const BUILD_BUDGET_TABLE: &str = "build_budgets";
const SCHEMA_VERSION: u32 = 1;
const JOB_FORMAT: u32 = 1;
const MAX_LEDGER_REFS: usize = 4_096;
const MAX_CANDIDATES: usize = 100;
const MAX_CHILDREN: usize = 64;
const MAX_ANCESTORS: usize = 32;
const MAX_BUDGET_CHARGES: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildStage {
    Admit,
    Resolve,
    Search,
    Select,
    Compose,
    Validate,
    Decompose,
    AdmitChildren,
    Recurse,
    Assemble,
    Gate,
    Fail,
    Complete,
}

impl BuildStage {
    fn may_advance_to(self, next: Self) -> bool {
        use BuildStage::*;
        self == next
            || matches!(
                (self, next),
                (Admit, Resolve | Fail)
                    | (Resolve, Search | Complete | Fail)
                    | (Search, Select | Decompose | Complete | Fail)
                    | (Select, Compose | Decompose | Fail)
                    | (Compose, Validate | Fail)
                    | (Validate, Search | Compose | Decompose | Assemble | Fail)
                    | (Decompose, AdmitChildren | Fail)
                    | (AdmitChildren, Recurse | Fail)
                    | (Recurse, Assemble | Fail)
                    | (Assemble, Gate | Fail)
                    | (Gate, Complete | Fail)
                    | (Fail, Complete)
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildReactionKind {
    Select,
    Compose,
    Decompose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildReactionStatus {
    Pending,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildReaction {
    pub reaction_id: String,
    pub kind: BuildReactionKind,
    pub prompt_pin: String,
    pub input_hash: String,
    pub prompt_hash: String,
    pub status: BuildReactionStatus,
    pub output_hash: Option<String>,
}

/// Kernel-owned measurements attached to one offered artifact. Model-emitted scores never replace
/// these values and are used only for advisory ordering.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildCandidateMeasurement {
    pub pin: String,
    #[serde(default = "legacy_embedding_space")]
    pub embedding_space: String,
    pub measured_similarity: f64,
    pub rrf_score: f64,
    pub text_rank: u32,
    pub vector_rank: u32,
    pub interface: super::ArtifactInterface,
    pub description: String,
}

fn legacy_embedding_space() -> String {
    "aelio.embedding.lexical@1".into()
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildWorkspace {
    pub interface_hash: String,
    pub version: u32,
    pub widened: bool,
    pub recomposed: bool,
    pub candidates: Vec<String>,
    #[serde(default)]
    pub candidate_measurements: Vec<BuildCandidateMeasurement>,
    pub selected: Vec<String>,
    pub draft: Option<serde_json::Value>,
    pub diagnostic: Option<String>,
    pub children: Vec<BuildSpec>,
    pub decomposition_seams: Vec<DecompositionSeam>,
    pub parent_complexity: u32,
    pub child_complexities: BTreeMap<String, u32>,
    pub resolved_seams: Vec<HarnessSeam>,
    pub topological_children: Vec<String>,
    pub child_build_ids: Vec<String>,
    pub child_artifacts: Vec<String>,
    pub assembled_hash: Option<String>,
    pub harness: Option<HarnessBody>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildLineage {
    pub root_build_id: String,
    pub parent_build_id: Option<String>,
    pub ancestor_spec_hashes: Vec<String>,
    pub depth: u16,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildJob {
    pub job_format: u32,
    pub build_id: String,
    pub lineage: BuildLineage,
    pub spec: BuildSpec,
    pub stage: BuildStage,
    pub revision: u64,
    pub cost: BuildCost,
    pub ledger: Vec<String>,
    pub workspace: BuildWorkspace,
    pub reaction: Option<BuildReaction>,
    pub result: Option<BuildResult>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl BuildJob {
    pub fn new(spec: BuildSpec) -> Result<Self, ArtifactError> {
        spec.validate()?;
        let now = now_ms()?;
        let build_id = format!("build-{}", &spec.spec_hash[..32]);
        Ok(Self {
            job_format: JOB_FORMAT,
            lineage: BuildLineage {
                root_build_id: build_id.clone(),
                parent_build_id: None,
                ancestor_spec_hashes: Vec::new(),
                depth: 1,
            },
            build_id,
            spec,
            stage: BuildStage::Admit,
            revision: 1,
            cost: BuildCost::default(),
            ledger: Vec::new(),
            workspace: BuildWorkspace::default(),
            reaction: None,
            result: None,
            created_at_ms: now,
            updated_at_ms: now,
        })
    }

    pub fn child(
        spec: BuildSpec,
        root_build_id: &str,
        parent_build_id: &str,
        ancestor_spec_hashes: Vec<String>,
        depth: u16,
    ) -> Result<Self, ArtifactError> {
        spec.validate()?;
        validate_build_id(root_build_id)?;
        validate_build_id(parent_build_id)?;
        if depth < 2
            || ancestor_spec_hashes.is_empty()
            || ancestor_spec_hashes.len() >= MAX_ANCESTORS
        {
            return Err(ArtifactError::Invalid("invalid child build lineage".into()));
        }
        for hash in &ancestor_spec_hashes {
            validate_hash("ancestor_spec_hash", hash)?;
        }
        let identity = serde_json::json!({"root":root_build_id,"spec":spec.spec_hash});
        let identity_hash = aelio_sol::value_hash(&super::json_to_sol(&identity)?);
        let build_id = format!("build-{}", &identity_hash[..32]);
        let now = now_ms()?;
        Ok(Self {
            job_format: JOB_FORMAT,
            build_id,
            lineage: BuildLineage {
                root_build_id: root_build_id.into(),
                parent_build_id: Some(parent_build_id.into()),
                ancestor_spec_hashes,
                depth,
            },
            spec,
            stage: BuildStage::Admit,
            revision: 1,
            cost: BuildCost::default(),
            ledger: Vec::new(),
            workspace: BuildWorkspace::default(),
            reaction: None,
            result: None,
            created_at_ms: now,
            updated_at_ms: now,
        })
    }

    fn validate(&self) -> Result<(), ArtifactError> {
        if self.job_format != JOB_FORMAT {
            return Err(ArtifactError::Invalid(
                "unsupported build job format".into(),
            ));
        }
        self.spec.validate()?;
        let expected_id = if self.lineage.parent_build_id.is_none() {
            format!("build-{}", &self.spec.spec_hash[..32])
        } else {
            let identity = serde_json::json!({
                "root": self.lineage.root_build_id,
                "spec": self.spec.spec_hash,
            });
            let identity_hash = aelio_sol::value_hash(&super::json_to_sol(&identity)?);
            format!("build-{}", &identity_hash[..32])
        };
        if self.build_id != expected_id {
            return Err(ArtifactError::Invalid("build id/spec hash mismatch".into()));
        }
        validate_build_id(&self.lineage.root_build_id)?;
        if self.lineage.depth == 0
            || self.lineage.depth > 32
            || self.lineage.ancestor_spec_hashes.len() >= MAX_ANCESTORS
            || (self.lineage.parent_build_id.is_none()
                != (self.lineage.root_build_id == self.build_id
                    && self.lineage.ancestor_spec_hashes.is_empty()
                    && self.lineage.depth == 1))
        {
            return Err(ArtifactError::Invalid("invalid build lineage".into()));
        }
        if let Some(parent) = &self.lineage.parent_build_id {
            validate_build_id(parent)?;
        }
        for hash in &self.lineage.ancestor_spec_hashes {
            validate_hash("ancestor_spec_hash", hash)?;
        }
        if self.revision == 0 || self.created_at_ms > self.updated_at_ms {
            return Err(ArtifactError::Invalid(
                "invalid build job revision/time".into(),
            ));
        }
        if self.ledger.len() > MAX_LEDGER_REFS
            || self
                .ledger
                .iter()
                .any(|entry| entry.is_empty() || entry.len() > 512)
        {
            return Err(ArtifactError::Invalid("invalid build ledger".into()));
        }
        if self.workspace.candidates.len() > MAX_CANDIDATES
            || self.workspace.candidate_measurements.len() > MAX_CANDIDATES
            || self.workspace.selected.len() > MAX_CANDIDATES
            || self.workspace.children.len() > MAX_CHILDREN
            || self.workspace.decomposition_seams.len() > 1_024
            || self.workspace.child_complexities.len() > MAX_CHILDREN
            || self.workspace.resolved_seams.len() > 1_024
            || self.workspace.topological_children.len() > MAX_CHILDREN
            || self.workspace.child_build_ids.len() > MAX_CHILDREN
            || self.workspace.child_artifacts.len() > MAX_CHILDREN
        {
            return Err(ArtifactError::Invalid(
                "build workspace exceeds bounds".into(),
            ));
        }
        let mut measured = std::collections::HashSet::new();
        for candidate in &self.workspace.candidate_measurements {
            validate_pin(&candidate.pin)?;
            if !candidate.measured_similarity.is_finite()
                || !(0.0..=1.0).contains(&candidate.measured_similarity)
                || !candidate.rrf_score.is_finite()
                || candidate.rrf_score < 0.0
                || candidate.text_rank == 0
                || candidate.vector_rank == 0
                || candidate.description.len() > 16 * 1024
                || !measured.insert(candidate.pin.as_str())
            {
                return Err(ArtifactError::Invalid(
                    "build candidate measurements are invalid".into(),
                ));
            }
        }
        if self
            .workspace
            .child_complexities
            .iter()
            .any(|(name, complexity)| validate_id(name).is_err() || *complexity == 0)
        {
            return Err(ArtifactError::Invalid(
                "invalid child complexity map".into(),
            ));
        }
        if !self.workspace.interface_hash.is_empty() {
            validate_hash("interface_hash", &self.workspace.interface_hash)?;
        }
        if let Some(hash) = &self.workspace.assembled_hash {
            validate_hash("assembled_hash", hash)?;
        }
        if let Some(harness) = &self.workspace.harness {
            harness.validate_shape()?;
        }
        if let Some(reaction) = &self.reaction {
            validate_reaction(reaction)?;
        }
        if (self.stage == BuildStage::Complete) != self.result.is_some() {
            return Err(ArtifactError::Invalid(
                "only complete jobs may carry a result".into(),
            ));
        }
        Ok(())
    }
}

fn validate_reaction(reaction: &BuildReaction) -> Result<(), ArtifactError> {
    validate_id(&reaction.reaction_id)?;
    validate_pin(&reaction.prompt_pin)?;
    validate_hash("reaction input_hash", &reaction.input_hash)?;
    validate_hash("reaction prompt_hash", &reaction.prompt_hash)?;
    match (reaction.status, &reaction.output_hash) {
        (BuildReactionStatus::Pending, None) => Ok(()),
        (BuildReactionStatus::Completed, Some(hash)) => validate_hash("output_hash", hash),
        _ => Err(ArtifactError::Invalid(
            "invalid reaction status/hash".into(),
        )),
    }
}

#[derive(Debug, Clone)]
pub struct BuildJobRecord {
    pub job: BuildJob,
    store_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildBudgetCharge {
    pub build_id: String,
    pub reaction_id: String,
    #[serde(default = "one_model_call")]
    pub model_calls: u32,
    pub tokens: u64,
    pub reported_wall_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildBudgetAccount {
    pub root_build_id: String,
    pub max_llm_calls: u32,
    pub max_tokens: u64,
    pub max_wall_ms: u64,
    pub deadline_at_ms: i64,
    pub llm_calls: u32,
    pub tokens: u64,
    pub charges: BTreeMap<String, BuildBudgetCharge>,
}

#[derive(Clone)]
pub struct BuildBudgetRepository<S: Store + Clone> {
    store: S,
}

impl<S: Store + Clone> BuildBudgetRepository<S> {
    pub fn open(mut store: S) -> Result<Self, ArtifactError> {
        ensure_repository_schema(&mut store, BUILD_BUDGET_TABLE, SCHEMA_VERSION)?;
        Ok(Self { store })
    }

    pub fn create(
        &mut self,
        tenant: &str,
        root_build_id: &str,
        budget: &super::BuildBudget,
        created_at_ms: i64,
    ) -> Result<BuildBudgetAccount, ArtifactError> {
        validate_tenant(tenant)?;
        validate_build_id(root_build_id)?;
        let wall = i64::try_from(budget.max_wall_ms)
            .map_err(|_| ArtifactError::Invalid("build wall budget is invalid".into()))?;
        let account = BuildBudgetAccount {
            root_build_id: root_build_id.into(),
            max_llm_calls: budget.max_llm_calls,
            max_tokens: budget.max_tokens,
            max_wall_ms: budget.max_wall_ms,
            deadline_at_ms: created_at_ms
                .checked_add(wall)
                .ok_or_else(|| ArtifactError::Invalid("build deadline overflow".into()))?,
            llm_calls: 0,
            tokens: 0,
            charges: BTreeMap::new(),
        };
        validate_budget_account(&account)?;
        match self
            .store
            .put_if_absent(tenant, BUILD_BUDGET_TABLE, root_build_id, encode(&account)?)
            .map_err(store_error)?
        {
            PutIfAbsent::Inserted { .. } => Ok(account),
            PutIfAbsent::Existing(row) => {
                let existing: BuildBudgetAccount = decode(&row.value)?;
                validate_budget_account(&existing)?;
                if existing.root_build_id != account.root_build_id
                    || existing.max_llm_calls != account.max_llm_calls
                    || existing.max_tokens != account.max_tokens
                    || existing.max_wall_ms != account.max_wall_ms
                    || existing.deadline_at_ms != account.deadline_at_ms
                {
                    return Err(ArtifactError::Conflict(
                        "shared budget account differs from sealed root budget".into(),
                    ));
                }
                Ok(existing)
            }
        }
    }

    pub fn get(
        &self,
        tenant: &str,
        root_build_id: &str,
    ) -> Result<BuildBudgetAccount, ArtifactError> {
        validate_tenant(tenant)?;
        validate_build_id(root_build_id)?;
        let row = self
            .store
            .get(tenant, BUILD_BUDGET_TABLE, root_build_id)
            .map_err(store_error)?
            .ok_or_else(|| ArtifactError::NotFound(root_build_id.into()))?;
        let account: BuildBudgetAccount = decode(&row.value)?;
        validate_budget_account(&account)?;
        Ok(account)
    }

    pub fn ensure_dispatch_allowed(
        &self,
        tenant: &str,
        root_build_id: &str,
    ) -> Result<BuildBudgetAccount, ArtifactError> {
        let account = self.get(tenant, root_build_id)?;
        if now_ms()? > account.deadline_at_ms
            || account.llm_calls >= account.max_llm_calls
            || account.tokens >= account.max_tokens
        {
            Err(ArtifactError::Invalid(
                "shared build budget exhausted".into(),
            ))
        } else {
            Ok(account)
        }
    }

    /// Idempotent CAS decrement-or-fail for one persisted oracle intent. A CAS loss retries from
    /// the latest account; the unique `(build,reaction)` charge key prevents double billing.
    pub fn charge(
        &mut self,
        tenant: &str,
        root_build_id: &str,
        charge: BuildBudgetCharge,
    ) -> Result<BuildBudgetAccount, ArtifactError> {
        validate_tenant(tenant)?;
        validate_build_id(root_build_id)?;
        validate_build_id(&charge.build_id)?;
        validate_id(&charge.reaction_id)?;
        let charge_key = format!("{}|{}", charge.build_id, charge.reaction_id);
        for _ in 0..16 {
            let row = self
                .store
                .get(tenant, BUILD_BUDGET_TABLE, root_build_id)
                .map_err(store_error)?
                .ok_or_else(|| ArtifactError::NotFound(root_build_id.into()))?;
            let mut account: BuildBudgetAccount = decode(&row.value)?;
            validate_budget_account(&account)?;
            if let Some(existing) = account.charges.get(&charge_key) {
                if existing == &charge {
                    return Ok(account);
                }
                return Err(ArtifactError::Conflict(
                    "oracle reaction already has a different budget charge".into(),
                ));
            }
            let now = now_ms()?;
            let calls = account
                .llm_calls
                .checked_add(charge.model_calls)
                .ok_or_else(|| ArtifactError::Invalid("LLM call counter overflow".into()))?;
            let tokens = account
                .tokens
                .checked_add(charge.tokens)
                .ok_or_else(|| ArtifactError::Invalid("token counter overflow".into()))?;
            if now > account.deadline_at_ms
                || calls > account.max_llm_calls
                || tokens > account.max_tokens
                || account.charges.len() >= MAX_BUDGET_CHARGES
            {
                return Err(ArtifactError::Invalid(
                    "shared build budget exhausted".into(),
                ));
            }
            account.llm_calls = calls;
            account.tokens = tokens;
            account.charges.insert(charge_key.clone(), charge.clone());
            match self.store.cas(
                tenant,
                BUILD_BUDGET_TABLE,
                root_build_id,
                row.version,
                encode(&account)?,
            ) {
                Ok(_) => return Ok(account),
                Err(StoreError::Conflict) => continue,
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(ArtifactError::Conflict(
            "shared budget CAS contention exceeded retry bound".into(),
        ))
    }
}

fn validate_budget_account(account: &BuildBudgetAccount) -> Result<(), ArtifactError> {
    validate_build_id(&account.root_build_id)?;
    if account.max_llm_calls > 1_000
        || account.max_tokens == 0
        || account.max_tokens > 100_000_000
        || account.llm_calls > account.max_llm_calls
        || account.tokens > account.max_tokens
        || account.charges.len() > MAX_BUDGET_CHARGES
    {
        return Err(ArtifactError::Invalid("invalid shared build budget".into()));
    }
    for (key, charge) in &account.charges {
        validate_build_id(&charge.build_id)?;
        validate_id(&charge.reaction_id)?;
        if key != &format!("{}|{}", charge.build_id, charge.reaction_id) {
            return Err(ArtifactError::Invalid(
                "invalid shared budget charge key".into(),
            ));
        }
        if charge.model_calls == 0 {
            return Err(ArtifactError::Invalid(
                "oracle budget charge must contain at least one model call".into(),
            ));
        }
    }
    Ok(())
}

fn one_model_call() -> u32 {
    1
}

#[derive(Clone)]
pub struct BuildJobRepository<S: Store + Clone> {
    store: S,
}

impl<S: Store + Clone> BuildJobRepository<S> {
    pub fn open(mut store: S) -> Result<Self, ArtifactError> {
        ensure_repository_schema(&mut store, BUILD_JOB_TABLE, SCHEMA_VERSION)?;
        Ok(Self { store })
    }

    pub fn create(
        &mut self,
        tenant: &str,
        spec: BuildSpec,
    ) -> Result<BuildJobRecord, ArtifactError> {
        validate_tenant(tenant)?;
        let job = BuildJob::new(spec)?;
        job.validate()?;
        match self
            .store
            .put_if_absent(tenant, BUILD_JOB_TABLE, &job.build_id, encode(&job)?)
            .map_err(store_error)?
        {
            PutIfAbsent::Inserted { version } => Ok(BuildJobRecord {
                job,
                store_version: version,
            }),
            PutIfAbsent::Existing(row) => {
                let existing: BuildJob = decode(&row.value)?;
                existing.validate()?;
                if existing.spec != job.spec {
                    return Err(ArtifactError::Conflict(
                        "build id collision with different spec".into(),
                    ));
                }
                Ok(BuildJobRecord {
                    job: existing,
                    store_version: row.version,
                })
            }
        }
    }

    pub fn create_child(
        &mut self,
        tenant: &str,
        spec: BuildSpec,
        root_build_id: &str,
        parent_build_id: &str,
        ancestor_spec_hashes: Vec<String>,
        depth: u16,
    ) -> Result<BuildJobRecord, ArtifactError> {
        validate_tenant(tenant)?;
        let job = BuildJob::child(
            spec,
            root_build_id,
            parent_build_id,
            ancestor_spec_hashes,
            depth,
        )?;
        self.create_job(tenant, job)
    }

    fn create_job(&mut self, tenant: &str, job: BuildJob) -> Result<BuildJobRecord, ArtifactError> {
        job.validate()?;
        match self
            .store
            .put_if_absent(tenant, BUILD_JOB_TABLE, &job.build_id, encode(&job)?)
            .map_err(store_error)?
        {
            PutIfAbsent::Inserted { version } => Ok(BuildJobRecord {
                job,
                store_version: version,
            }),
            PutIfAbsent::Existing(row) => {
                let existing: BuildJob = decode(&row.value)?;
                existing.validate()?;
                if existing.spec != job.spec || existing.lineage != job.lineage {
                    return Err(ArtifactError::Conflict(
                        "build id collision with different spec or lineage".into(),
                    ));
                }
                Ok(BuildJobRecord {
                    job: existing,
                    store_version: row.version,
                })
            }
        }
    }

    pub fn get(
        &self,
        tenant: &str,
        build_id: &str,
    ) -> Result<Option<BuildJobRecord>, ArtifactError> {
        validate_tenant(tenant)?;
        validate_build_id(build_id)?;
        self.store
            .get(tenant, BUILD_JOB_TABLE, build_id)
            .map_err(store_error)?
            .map(|row| {
                let job: BuildJob = decode(&row.value)?;
                job.validate()?;
                Ok(BuildJobRecord {
                    job,
                    store_version: row.version,
                })
            })
            .transpose()
    }

    pub fn commit(
        &mut self,
        tenant: &str,
        current: BuildJobRecord,
        mut next: BuildJob,
    ) -> Result<BuildJobRecord, ArtifactError> {
        validate_tenant(tenant)?;
        if next.spec != current.job.spec
            || next.build_id != current.job.build_id
            || next.lineage != current.job.lineage
        {
            return Err(ArtifactError::Invalid("build identity is immutable".into()));
        }
        if !current.job.stage.may_advance_to(next.stage) {
            return Err(ArtifactError::Invalid(format!(
                "illegal builder transition {:?} -> {:?}",
                current.job.stage, next.stage
            )));
        }
        if next.cost.llm_calls < current.job.cost.llm_calls
            || next.cost.tokens < current.job.cost.tokens
            || next.cost.wall_ms < current.job.cost.wall_ms
            || next.cost.depth_reached < current.job.cost.depth_reached
            || next.cost.retries < current.job.cost.retries
            || !next.ledger.starts_with(&current.job.ledger)
        {
            return Err(ArtifactError::Invalid(
                "build cost/ledger must be monotonic".into(),
            ));
        }
        enforce_budget(&next)?;
        next.revision = current
            .job
            .revision
            .checked_add(1)
            .ok_or_else(|| ArtifactError::Invalid("build revision exhausted".into()))?;
        next.created_at_ms = current.job.created_at_ms;
        next.updated_at_ms = now_ms()?;
        next.validate()?;
        let store_version = self
            .store
            .cas(
                tenant,
                BUILD_JOB_TABLE,
                &next.build_id,
                current.store_version,
                encode(&next)?,
            )
            .map_err(|error| match error {
                StoreError::Conflict => ArtifactError::Conflict("build CAS lost".into()),
                other => store_error(other),
            })?;
        Ok(BuildJobRecord {
            job: next,
            store_version,
        })
    }
}

fn enforce_budget(job: &BuildJob) -> Result<(), ArtifactError> {
    let budget = &job.spec.draft.budget;
    if job.cost.llm_calls > budget.max_llm_calls
        || job.cost.tokens > budget.max_tokens
        || job.cost.wall_ms > budget.max_wall_ms
        || job.cost.depth_reached > budget.max_depth
    {
        return Err(ArtifactError::Invalid("build exceeds sealed budget".into()));
    }
    Ok(())
}

fn validate_build_id(build_id: &str) -> Result<(), ArtifactError> {
    if build_id.len() != 38
        || !build_id.starts_with("build-")
        || !build_id[6..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        Err(ArtifactError::Invalid(
            "invalid deterministic build id".into(),
        ))
    } else {
        Ok(())
    }
}
