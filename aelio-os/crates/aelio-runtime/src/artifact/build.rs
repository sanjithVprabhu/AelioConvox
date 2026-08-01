//! Closed builder input/output contracts. These types contain no model behavior; they are the
//! deterministic boundary around the durable root-harness state machine.

use super::{
    json_to_sol, validate_hash, validate_id, validate_pin, Artifact, ArtifactEffect, ArtifactError,
    ArtifactInput,
};
use aelio_sol::{value_hash, Limits};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const MAX_REGISTRIES: usize = 128;
const MAX_EXAMPLES: usize = 256;
const MAX_FIXTURES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildBudget {
    pub max_depth: u16,
    pub max_children: u16,
    pub max_llm_calls: u32,
    pub max_tokens: u64,
    pub max_reactions: u32,
    pub max_wall_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildScope {
    pub tenant: String,
    pub registries: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildPolicy {
    pub principal_grants: Vec<ArtifactEffect>,
    pub allowed_effects: Vec<ArtifactEffect>,
    pub denied_effects: Vec<ArtifactEffect>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildExample {
    pub inputs: serde_json::Value,
    pub output: serde_json::Value,
    #[serde(default)]
    pub negative: bool,
    #[serde(default)]
    pub fixtures: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildSpecDraft {
    pub name: String,
    pub description: String,
    pub inputs: Vec<ArtifactInput>,
    pub output: String,
    pub budget: BuildBudget,
    pub scope: BuildScope,
    pub policy: BuildPolicy,
    pub examples: Vec<BuildExample>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildSpec {
    #[serde(flatten)]
    pub draft: BuildSpecDraft,
    pub spec_hash: String,
}

impl BuildSpec {
    pub fn seal(draft: BuildSpecDraft) -> Result<Self, ArtifactError> {
        validate_draft(&draft)?;
        let json = serde_json::to_value(&draft)
            .map_err(|error| ArtifactError::Invalid(error.to_string()))?;
        let spec_hash = value_hash(&json_to_sol(&json)?);
        Ok(Self { draft, spec_hash })
    }

    pub fn validate(&self) -> Result<(), ArtifactError> {
        validate_draft(&self.draft)?;
        validate_hash("spec_hash", &self.spec_hash)?;
        let actual = Self::seal(self.draft.clone())?.spec_hash;
        if actual != self.spec_hash {
            return Err(ArtifactError::HashMismatch {
                expected: actual,
                actual: self.spec_hash.clone(),
            });
        }
        Ok(())
    }
}

fn validate_draft(draft: &BuildSpecDraft) -> Result<(), ArtifactError> {
    validate_id(&draft.name)?;
    if draft.description.trim().is_empty() || draft.description.len() > 16 * 1024 {
        return Err(ArtifactError::Invalid(
            "build description must contain 1..=16384 bytes".into(),
        ));
    }
    if draft.inputs.len() > 256 {
        return Err(ArtifactError::Invalid("too many build inputs".into()));
    }
    let mut names = HashSet::new();
    for input in &draft.inputs {
        validate_id(&input.name)?;
        validate_pin(&input.imprint)?;
        if !names.insert(input.name.as_str()) {
            return Err(ArtifactError::Invalid(
                "build input names must be unique".into(),
            ));
        }
    }
    validate_pin(&draft.output)?;
    let budget = &draft.budget;
    if budget.max_depth == 0
        || budget.max_depth > 32
        || budget.max_children == 0
        || budget.max_children > 64
        || budget.max_llm_calls == 0
        || budget.max_llm_calls > 1_000
        || budget.max_tokens == 0
        || budget.max_tokens > 100_000_000
        || budget.max_reactions == 0
        || budget.max_reactions > 100_000
        || budget.max_wall_ms == 0
        || budget.max_wall_ms > 86_400_000
    {
        return Err(ArtifactError::Invalid(
            "build budget is outside hard bounds".into(),
        ));
    }
    if draft.scope.tenant.trim().is_empty()
        || draft.scope.registries.is_empty()
        || draft.scope.registries.len() > MAX_REGISTRIES
    {
        return Err(ArtifactError::Invalid("build scope is invalid".into()));
    }
    unique("scope registries", &draft.scope.registries)?;
    validate_policy(&draft.policy)?;
    if draft.examples.len() < 3 || draft.examples.len() > MAX_EXAMPLES {
        return Err(ArtifactError::Invalid(
            "build requires 3..=256 examples".into(),
        ));
    }
    if !draft.examples.iter().any(|example| example.negative) {
        return Err(ArtifactError::Invalid(
            "build examples require at least one refusal/negative case".into(),
        ));
    }
    if draft
        .examples
        .iter()
        .any(|example| example.fixtures.len() > MAX_FIXTURES)
    {
        return Err(ArtifactError::Invalid("too many example fixtures".into()));
    }
    let json =
        serde_json::to_value(draft).map_err(|error| ArtifactError::Invalid(error.to_string()))?;
    Limits::default()
        .check(&json_to_sol(&json)?)
        .map_err(|error| ArtifactError::Invalid(error.to_string()))?;
    Ok(())
}

fn validate_policy(policy: &BuildPolicy) -> Result<(), ArtifactError> {
    let grants: HashSet<_> = policy.principal_grants.iter().copied().collect();
    let allowed: HashSet<_> = policy.allowed_effects.iter().copied().collect();
    let denied: HashSet<_> = policy.denied_effects.iter().copied().collect();
    if grants.len() != policy.principal_grants.len()
        || allowed.len() != policy.allowed_effects.len()
        || denied.len() != policy.denied_effects.len()
    {
        return Err(ArtifactError::Invalid(
            "effect policy contains duplicates".into(),
        ));
    }
    if !allowed.is_subset(&grants) {
        return Err(ArtifactError::Invalid(
            "allowed effects exceed principal grants".into(),
        ));
    }
    if !allowed.is_disjoint(&denied) {
        return Err(ArtifactError::Invalid(
            "allowed and denied effects overlap".into(),
        ));
    }
    Ok(())
}

fn unique(name: &str, values: &[String]) -> Result<(), ArtifactError> {
    let mut seen = HashSet::new();
    if values
        .iter()
        .any(|value| value.is_empty() || !seen.insert(value))
    {
        Err(ArtifactError::Invalid(format!(
            "{name} must be nonempty and unique"
        )))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildCost {
    pub llm_calls: u32,
    pub tokens: u64,
    pub wall_ms: u64,
    pub depth_reached: u16,
    pub retries: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildFailure {
    pub stage: String,
    pub spec_path: Vec<String>,
    pub reason_code: String,
    pub detail: String,
    pub missing: Vec<String>,
    pub insufficiency_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum BuildResult {
    Built {
        spec_hash: String,
        cost: BuildCost,
        ledger: Vec<String>,
        artifact: Box<Artifact>,
        gate_verdict_hash: String,
    },
    Reused {
        spec_hash: String,
        cost: BuildCost,
        ledger: Vec<String>,
        artifact_pin: String,
    },
    Failed {
        spec_hash: String,
        cost: BuildCost,
        ledger: Vec<String>,
        failure: BuildFailure,
    },
}
