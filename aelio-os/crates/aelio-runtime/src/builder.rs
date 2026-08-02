//! Durable root-builder control loop. Oracle work is emitted as an idempotent action only after
//! its intent is committed; callers submit a closed response carrying the same reaction id.

use crate::{
    artifact_error, ArtifactEffect, ArtifactError, ArtifactStatus, BuildBudgetCharge,
    BuildBudgetRepository, BuildFailure, BuildJob, BuildJobRecord, BuildJobRepository,
    BuildReaction, BuildReactionKind, BuildReactionStatus, BuildResult, BuildSpec, BuildStage,
    CapabilityReason, CapabilityRequestDraft, CapabilityRequestRepository, FlowPush,
    GenericArtifactGate, LeaseRequest, NameLeaseRepository, PromptArtifact, Runtime, RuntimeError,
    SandboxCase, SandboxFixtureCall, SandboxLimits, SandboxRunner, VerificationCacheEntry,
    VerificationCacheRepository, VerificationVerdict,
};
use aelio_db_query::{execute_prism, ColumnKind, Database, Value as DbValue};
use aelio_prompt::{prompt_artifact_hash, ModelPin, SlotDecl, TemplateRegistry};
use aelio_query::parse_prism;
use aelio_sol::{value_hash, SolValue};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::time::Instant;

const SELECT_PROMPT: &str = "aelio.template.builder_select@2";
const COMPOSE_PROMPT: &str = "aelio.template.builder_compose@2";
const DECOMPOSE_PROMPT: &str = "aelio.template.builder_decompose@2";
const SELECT_MIN_SIMILARITY: f64 = 0.60;
const SELECT_MIN_MARGIN: f64 = 0.08;
const SELECT_MAX_ENTROPY: f64 = 0.75;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BuildAction {
    Progress {
        job: BuildJob,
    },
    Oracle {
        job: BuildJob,
        reaction: BuildReaction,
        rendered_prompt: String,
    },
    Complete {
        job: BuildJob,
        result: BuildResult,
    },
}

#[derive(Debug, Clone)]
pub struct BuildOracleRequest<'a> {
    pub tenant: &'a str,
    pub build_id: &'a str,
    pub reaction: &'a BuildReaction,
    pub rendered_prompt: &'a str,
    pub retry: bool,
}

#[derive(Debug, Clone)]
pub struct BuildOracleResponse {
    pub text: String,
    pub usage_tokens: u64,
}

/// Intelligence boundary for the deterministic builder. Implementations return text only; the
/// runtime injects the expected reaction discriminator and validates the closed Rust schema before
/// any state transition can consume the response.
pub trait BuildOracle: Send + Sync {
    fn complete(
        &self,
        request: BuildOracleRequest<'_>,
    ) -> Result<BuildOracleResponse, RuntimeError>;
}

struct HostBuildOracle<'a> {
    host: &'a crate::HostClient,
}

impl BuildOracle for HostBuildOracle<'_> {
    fn complete(
        &self,
        request: BuildOracleRequest<'_>,
    ) -> Result<BuildOracleResponse, RuntimeError> {
        let prompt = if request.retry {
            format!(
                "{}\n\nYour prior response failed the declared closed JSON schema. Return only one valid JSON object with exactly the requested fields; do not add commentary.",
                request.rendered_prompt
            )
        } else {
            request.rendered_prompt.to_owned()
        };
        let invocation = self
            .host
            .invoke(
                "aelio.model.complete@1",
                &SolValue::map([
                    ("prompt", SolValue::str(prompt)),
                    (
                        "prompt_hash",
                        SolValue::str(request.reaction.prompt_hash.clone()),
                    ),
                    ("model", SolValue::str("aelio.model.builder@1")),
                    ("max_tokens", SolValue::Int(16_384)),
                    ("temperature", SolValue::Float(0.0)),
                ]),
                &aelio_kernel::registry::CallContext {
                    corr: format!(
                        "{}.{}",
                        request.reaction.reaction_id,
                        if request.retry { "retry" } else { "initial" }
                    ),
                    tenant: request.tenant.into(),
                    instance_id: request.build_id.into(),
                    turn_id: request.reaction.reaction_id.clone(),
                    nid: "builder.oracle".into(),
                    deadline_ms: Some(60_000),
                },
            )
            .map_err(RuntimeError::from)?;
        let text = invocation
            .output
            .as_map()
            .and_then(|map| map.get("text"))
            .and_then(|value| match value {
                SolValue::Str(value) => Some(value.clone()),
                _ => None,
            })
            .ok_or_else(|| RuntimeError::Invalid("builder model output lacks text".into()))?;
        Ok(BuildOracleResponse {
            text,
            usage_tokens: invocation.usage_tokens,
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BuildReactionOutput {
    Selection {
        selected: Vec<SelectionChoice>,
        #[serde(default)]
        runners_up: Vec<SelectionRunnerUp>,
        #[serde(default)]
        unmet: Vec<String>,
        #[serde(default)]
        undeterminable: bool,
    },
    Composition {
        tree: Vec<crate::HarnessNode>,
        seams: Vec<crate::DecompositionSeam>,
        #[serde(default)]
        undeterminable: bool,
    },
    Decomposition {
        verdict: DecompositionVerdict,
        #[serde(default)]
        undeterminable: bool,
        #[serde(default)]
        children: Vec<BuildSpec>,
        #[serde(default)]
        seams: Vec<crate::DecompositionSeam>,
        #[serde(default)]
        parent_complexity: u32,
        #[serde(default)]
        child_complexities: BTreeMap<String, u32>,
        #[serde(default)]
        detail: String,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionChoice {
    pub id: String,
    pub score: f64,
    pub role: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionRunnerUp {
    pub id: String,
    pub score: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CompositionDraft {
    tree: Vec<crate::HarnessNode>,
    seams: Vec<crate::DecompositionSeam>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecompositionVerdict {
    Children,
    Atomic,
    Cannot,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildReactionUsage {
    #[serde(default = "one_model_call")]
    pub model_calls: u32,
    pub tokens: u64,
    pub wall_ms: u64,
}

impl Default for BuildReactionUsage {
    fn default() -> Self {
        Self {
            model_calls: 1,
            tokens: 0,
            wall_ms: 0,
        }
    }
}

fn one_model_call() -> u32 {
    1
}

pub fn builder_vendor_prompts() -> Vec<PromptArtifact> {
    vec![
        seed_prompt(
            "aelio.template.builder_select",
            "2",
            "Select only candidate pins supplied in payload. Scores are advisory; the kernel thresholds measured_similarity. Return JSON {selected:[{id,score,role}],runners_up:[{id,score}],unmet:string[],undeterminable:bool}. Payload={{payload}}",
            BTreeMap::from([
                ("selected".into(), "list".into()),
                ("runners_up".into(), "list".into()),
                ("unmet".into(), "list".into()),
                ("undeterminable".into(), "bool".into()),
            ]),
        ),
        seed_prompt(
            "aelio.template.builder_compose",
            "2",
            "Propose only topology using selected immutable artifact pins. Return closed JSON {tree:[{nid,artifact}],seams:[{from,to}],undeterminable}. Never author executable operations, target declarations, imprints, converters, versions, or unoffered pins. Set undeterminable=true when closure cannot be proven from the payload. Payload={{payload}}",
            BTreeMap::from([
                ("tree".into(), "list".into()),
                ("seams".into(), "list".into()),
                ("undeterminable".into(), "bool".into()),
            ]),
        ),
        seed_prompt(
            "aelio.template.builder_decompose",
            "2",
            "Return closed JSON {verdict,undeterminable,children,seams,parent_complexity,child_complexities,detail}. Every child must be strictly smaller. Seams use tagged parent_input/node_output sources and node_input/parent_output sinks; every required child input and the parent output must have exactly one producer. Do not name converters. Set undeterminable=true with no children or seams when the evidence cannot support a decomposition. Otherwise report atomic or cannot without inventing facts. Payload={{payload}}",
            BTreeMap::from([
                ("verdict".into(), "str".into()),
                ("undeterminable".into(), "bool".into()),
                ("children".into(), "list".into()),
                ("seams".into(), "list".into()),
                ("parent_complexity".into(), "int".into()),
                ("child_complexities".into(), "map".into()),
                ("detail".into(), "str".into()),
            ]),
        ),
    ]
}

/// Hand-authored registry entry #1: the root harness describes its own closed build interface and
/// deterministic spine. It is a vendor axiom, not an alternate executable implementation.
pub fn root_harness_vendor_artifact() -> Result<crate::Artifact, ArtifactError> {
    let self_spec = BuildSpec::seal(crate::BuildSpecDraft {
        name: "aelio.root_harness".into(),
        description: "build, reuse, validate, gate, or report a bounded Aelio artifact".into(),
        inputs: vec![crate::ArtifactInput {
            name: "spec".into(),
            imprint: "aelio.build_spec@2".into(),
            required: true,
            sensitivity: "internal".into(),
        }],
        output: "aelio.build_result@2".into(),
        budget: crate::BuildBudget {
            max_depth: 4,
            max_children: 8,
            max_llm_calls: 40,
            max_tokens: 100_000,
            max_reactions: 10_000,
            max_wall_ms: 120_000,
        },
        scope: crate::BuildScope {
            tenant: crate::mint::VENDOR_ARTIFACT_TENANT.into(),
            registries: vec!["*".into()],
        },
        policy: crate::BuildPolicy {
            principal_grants: vec![
                ArtifactEffect::Pure,
                ArtifactEffect::Read,
                ArtifactEffect::Write,
                ArtifactEffect::External,
            ],
            allowed_effects: vec![ArtifactEffect::Pure],
            denied_effects: vec![],
        },
        examples: vec![
            crate::BuildExample {
                inputs: serde_json::json!({"spec":"valid"}),
                output: serde_json::json!({"status":"built"}),
                negative: false,
                fixtures: vec![],
            },
            crate::BuildExample {
                inputs: serde_json::json!({"spec":"exact"}),
                output: serde_json::json!({"status":"reused"}),
                negative: false,
                fixtures: vec![],
            },
            crate::BuildExample {
                inputs: serde_json::json!({"spec":"invalid"}),
                output: serde_json::json!({"status":"failed"}),
                negative: true,
                fixtures: vec![],
            },
        ],
    })?;
    crate::Artifact::new(
        "aelio.root_harness",
        2,
        crate::ArtifactClass::Harness,
        crate::ArtifactTier::Locked,
        "1",
        env!("CARGO_PKG_VERSION"),
        crate::ArtifactInterface {
            inputs: self_spec.draft.inputs.clone(),
            output: self_spec.draft.output.clone(),
        },
        self_spec.draft.description.clone(),
        vec!["root".into(), "builder".into(), "axiom".into()],
        vec![ArtifactEffect::Pure],
        crate::ArtifactPins {
            prompts: vec![SELECT_PROMPT.into(), COMPOSE_PROMPT.into(), DECOMPOSE_PROMPT.into()],
            ..crate::ArtifactPins::default()
        },
        self_spec
            .draft
            .examples
            .iter()
            .map(|example| serde_json::json!({"inputs":example.inputs,"output":example.output,"negative":example.negative}))
            .collect(),
        serde_json::json!({
            "self_spec": self_spec,
            "stages": ["admit","resolve","search","select","compose","validate","decompose","admit_children","recurse","assemble","gate","fail"],
            "oracle_stages": ["select","compose","decompose"],
            "retry_caps": {"widen":1,"recompose":1},
        }),
        crate::Provenance {
            built_by: None,
            requester: Some("human-bootstrap".into()),
            build_id: None,
            metadata: serde_json::Map::from_iter([(
                "origin".into(),
                serde_json::Value::String("vendor".into()),
            )]),
        },
    )
}

fn seed_prompt(
    id: &str,
    version: &str,
    body: &str,
    output_fields: BTreeMap<String, String>,
) -> PromptArtifact {
    let output_imprint = match id {
        "aelio.template.builder_select" => "aelio.selection_result@2",
        "aelio.template.builder_compose" => "aelio.compose_result@2",
        "aelio.template.builder_decompose" => "aelio.decompose_result@2",
        _ => unreachable!("seed prompt id is closed"),
    };
    let mut prompt = PromptArtifact {
        template_format: 1,
        id: id.into(),
        version: version.into(),
        description: format!("Human-authored root-builder seed prompt {id}."),
        objective: id.into(),
        body: body.into(),
        slots: vec![SlotDecl {
            name: "payload".into(),
            ty: "str".into(),
            sensitivity: "internal".into(),
            required: true,
        }],
        layers: vec![],
        output_imprint: output_imprint.into(),
        output_fields,
        model: ModelPin {
            id: "aelio.model.builder@1".into(),
            params: [("temperature".into(), serde_json::json!(0.0))]
                .into_iter()
                .collect(),
        },
        exemplars: vec![],
        is_axiom: false,
        root_version: String::new(),
        composed_hash: String::new(),
        inputs_sufficient: true,
        sufficiency_note: "human-authored vendor artifact".into(),
    };
    prompt.composed_hash = prompt_artifact_hash(&prompt);
    prompt
}

impl Runtime {
    pub fn host_adapter_configured(&self) -> bool {
        self.inner.host.is_some()
    }

    pub fn submit_build(&self, spec: BuildSpec) -> Result<BuildJob, RuntimeError> {
        let tenant = spec.draft.scope.tenant.clone();
        let mut jobs =
            BuildJobRepository::open(self.inner.store.clone()).map_err(artifact_error)?;
        let record = jobs.create(&tenant, spec).map_err(artifact_error)?;
        BuildBudgetRepository::open(self.inner.store.clone())
            .map_err(artifact_error)?
            .create(
                &tenant,
                &record.job.lineage.root_build_id,
                &record.job.spec.draft.budget,
                record.job.created_at_ms,
            )
            .map_err(artifact_error)?;
        Ok(record.job)
    }

    pub fn get_build(
        &self,
        tenant: &str,
        build_id: &str,
    ) -> Result<Option<BuildJob>, RuntimeError> {
        BuildJobRepository::open(self.inner.store.clone())
            .map_err(artifact_error)?
            .get(tenant, build_id)
            .map(|record| record.map(|record| record.job))
            .map_err(artifact_error)
    }

    /// Advance exactly one durable boundary. Repeated calls after a crash either return the same
    /// pending oracle action (same reaction id) or continue from the committed deterministic stage.
    pub fn advance_build(
        &self,
        tenant: &str,
        build_id: &str,
        deployer: Option<String>,
    ) -> Result<BuildAction, RuntimeError> {
        let mut jobs =
            BuildJobRepository::open(self.inner.store.clone()).map_err(artifact_error)?;
        let record = jobs
            .get(tenant, build_id)
            .map_err(artifact_error)?
            .ok_or_else(|| RuntimeError::NotFound(build_id.into()))?;
        if record.job.spec.draft.scope.tenant != tenant {
            return Err(RuntimeError::NotFound(build_id.into()));
        }
        if record.job.stage == BuildStage::Complete {
            release_lease(self, tenant, &record.job)?;
            let result = record.job.result.clone().expect("validated complete job");
            self.settle_capability_build(tenant, build_id, &result)?;
            return Ok(BuildAction::Complete {
                result,
                job: record.job,
            });
        }
        if let Some(reaction) = &record.job.reaction {
            if reaction.status == BuildReactionStatus::Pending {
                let rendered_prompt = self.render_build_reaction(&record.job, reaction.kind)?;
                return Ok(BuildAction::Oracle {
                    job: record.job.clone(),
                    reaction: reaction.clone(),
                    rendered_prompt,
                });
            }
        }

        let action = match record.job.stage {
            BuildStage::Admit => self.advance_admit(&mut jobs, tenant, record),
            BuildStage::Resolve => self.advance_resolve(&mut jobs, tenant, record),
            BuildStage::Search => self.advance_search(&mut jobs, tenant, record),
            BuildStage::Select => {
                self.prepare_reaction(&mut jobs, tenant, record, BuildReactionKind::Select)
            }
            BuildStage::Compose => {
                self.prepare_reaction(&mut jobs, tenant, record, BuildReactionKind::Compose)
            }
            BuildStage::Validate => self.advance_validate(&mut jobs, tenant, record),
            BuildStage::Decompose => {
                self.prepare_reaction(&mut jobs, tenant, record, BuildReactionKind::Decompose)
            }
            BuildStage::AdmitChildren => self.advance_admit_children(&mut jobs, tenant, record),
            BuildStage::Recurse => self.advance_recurse(&mut jobs, tenant, record),
            BuildStage::Assemble => self.advance_assemble(&mut jobs, tenant, record),
            BuildStage::Gate => self.advance_gate(&mut jobs, tenant, record, deployer),
            BuildStage::Fail => self.advance_fail(&mut jobs, tenant, record),
            BuildStage::Complete => unreachable!(),
        }?;
        if let BuildAction::Complete { result, job } = &action {
            self.settle_capability_build(tenant, &job.build_id, result)?;
        }
        Ok(action)
    }

    /// Drive a durable build through deterministic stages and bounded model reactions. This never
    /// holds an HTTP request unless the caller explicitly chooses to; the same method is suitable
    /// for a background worker and returns the durable action at completion.
    pub fn run_build_worker(
        &self,
        tenant: &str,
        build_id: &str,
        deployer: Option<String>,
        oracle: &dyn BuildOracle,
        max_boundaries: usize,
    ) -> Result<BuildAction, RuntimeError> {
        if max_boundaries == 0 || max_boundaries > 4_096 {
            return Err(RuntimeError::Invalid(
                "build worker boundary limit must be 1..=4096".into(),
            ));
        }
        for _ in 0..max_boundaries {
            match self.advance_build(tenant, build_id, deployer.clone())? {
                complete @ BuildAction::Complete { .. } => return Ok(complete),
                BuildAction::Progress { .. } => continue,
                BuildAction::Oracle {
                    job,
                    reaction,
                    rendered_prompt,
                } => {
                    let started = Instant::now();
                    let mut tokens = 0_u64;
                    let mut calls = 0_u32;
                    let mut accepted = false;
                    let mut last_error = String::new();
                    for attempt in 0..2 {
                        if attempt == 1 {
                            let account = BuildBudgetRepository::open(self.inner.store.clone())
                                .map_err(artifact_error)?
                                .get(tenant, &job.lineage.root_build_id)
                                .map_err(artifact_error)?;
                            if account.llm_calls.saturating_add(calls).saturating_add(1)
                                > account.max_llm_calls
                            {
                                last_error =
                                    "closed-schema retry would exceed model-call budget".into();
                                break;
                            }
                        }
                        let response = oracle.complete(BuildOracleRequest {
                            tenant,
                            build_id,
                            reaction: &reaction,
                            rendered_prompt: &rendered_prompt,
                            retry: attempt == 1,
                        })?;
                        calls = calls.saturating_add(1);
                        tokens = tokens.saturating_add(response.usage_tokens);
                        let output = match parse_build_oracle_output(reaction.kind, &response.text)
                        {
                            Ok(output) => output,
                            Err(error) => {
                                last_error = error.to_string();
                                continue;
                            }
                        };
                        let usage = BuildReactionUsage {
                            model_calls: calls,
                            tokens,
                            wall_ms: u64::try_from(started.elapsed().as_millis())
                                .unwrap_or(u64::MAX),
                        };
                        match self.submit_build_reaction(
                            tenant,
                            build_id,
                            &reaction.reaction_id,
                            output,
                            usage,
                        ) {
                            Ok(_) => {
                                accepted = true;
                                break;
                            }
                            Err(RuntimeError::Invalid(detail)) if attempt == 0 => {
                                last_error = detail;
                            }
                            Err(error) => return Err(error),
                        }
                    }
                    if !accepted {
                        self.reject_build_reaction(
                            tenant,
                            build_id,
                            &reaction.reaction_id,
                            BuildReactionUsage {
                                model_calls: calls.max(1),
                                tokens,
                                wall_ms: u64::try_from(started.elapsed().as_millis())
                                    .unwrap_or(u64::MAX),
                            },
                            &last_error,
                        )?;
                    }
                }
            }
        }
        Err(RuntimeError::Conflict(
            "build worker reached its durable-boundary limit".into(),
        ))
    }

    /// Production worker entrypoint. Model inference crosses only the configured Rust→TypeScript
    /// host adapter; Rust owns prompt pins, parse/semantic validation, budgets, and state commits.
    pub fn run_build_with_host(
        &self,
        tenant: &str,
        build_id: &str,
        deployer: Option<String>,
        max_boundaries: usize,
    ) -> Result<BuildAction, RuntimeError> {
        let host = self.inner.host.as_ref().ok_or_else(|| {
            RuntimeError::Host(
                "build worker requires the configured TypeScript host adapter".into(),
            )
        })?;
        self.run_build_worker(
            tenant,
            build_id,
            deployer,
            &HostBuildOracle { host },
            max_boundaries,
        )
    }

    pub fn submit_build_reaction(
        &self,
        tenant: &str,
        build_id: &str,
        reaction_id: &str,
        output: BuildReactionOutput,
        usage: BuildReactionUsage,
    ) -> Result<BuildJob, RuntimeError> {
        let mut jobs =
            BuildJobRepository::open(self.inner.store.clone()).map_err(artifact_error)?;
        let record = jobs
            .get(tenant, build_id)
            .map_err(artifact_error)?
            .ok_or_else(|| RuntimeError::NotFound(build_id.into()))?;
        let reaction =
            record.job.reaction.as_ref().ok_or_else(|| {
                RuntimeError::Conflict("build has no pending oracle reaction".into())
            })?;
        if reaction.status != BuildReactionStatus::Pending || reaction.reaction_id != reaction_id {
            return Err(RuntimeError::Conflict(
                "reaction id is stale or already completed".into(),
            ));
        }
        validate_reaction_kind(reaction.kind, &output)?;
        validate_reaction_output(&record.job, &output)?;
        let output_json = serde_json::to_value(&output)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        let output_hash = value_hash(
            &aelio_kernel::json_from(&output_json)
                .map_err(|error| RuntimeError::Invalid(error.to_string()))?,
        );
        let mut next = record.job.clone();
        let shared = BuildBudgetRepository::open(self.inner.store.clone())
            .map_err(artifact_error)?
            .charge(
                tenant,
                &record.job.lineage.root_build_id,
                BuildBudgetCharge {
                    build_id: record.job.build_id.clone(),
                    reaction_id: reaction.reaction_id.clone(),
                    model_calls: usage.model_calls,
                    tokens: usage.tokens,
                    reported_wall_ms: usage.wall_ms,
                },
            );
        if matches!(
            shared,
            Err(ArtifactError::Invalid(ref detail)) if detail == "shared build budget exhausted"
        ) {
            next.reaction = None;
            next.workspace.diagnostic =
                Some("budget_exhausted: shared recursive build budget".into());
            next.ledger.push(format!(
                "reaction_rejected:{}:budget_exhausted",
                reaction.reaction_id
            ));
            next.stage = BuildStage::Fail;
            return jobs
                .commit(tenant, record, next)
                .map(|record| record.job)
                .map_err(artifact_error);
        }
        shared.map_err(artifact_error)?;
        next.cost.llm_calls = next.cost.llm_calls.saturating_add(usage.model_calls);
        next.cost.tokens = next.cost.tokens.saturating_add(usage.tokens);
        next.cost.wall_ms = next.cost.wall_ms.saturating_add(usage.wall_ms);
        next.ledger.push(format!(
            "reaction:{}:{}:{}:{}",
            reaction.reaction_id, reaction.prompt_pin, reaction.input_hash, output_hash
        ));
        next.reaction = None;
        match output {
            BuildReactionOutput::Selection {
                selected,
                runners_up,
                unmet,
                undeterminable,
            } => {
                let accepted = validate_measured_selection(
                    &next,
                    &selected,
                    &runners_up,
                    &unmet,
                    undeterminable,
                )?;
                if accepted.is_empty() {
                    next.workspace.selected.clear();
                    next.workspace.diagnostic = Some(
                        "select_abstained: measured similarity, margin, or entropy gate".into(),
                    );
                    next.ledger.push("builder:select:abstained".into());
                    next.stage = BuildStage::Decompose;
                } else {
                    next.workspace.selected = accepted;
                    next.ledger.push("builder:select:measured_pass".into());
                    next.stage = BuildStage::Compose;
                }
            }
            BuildReactionOutput::Composition {
                tree,
                seams,
                undeterminable,
            } => {
                if undeterminable {
                    next.workspace.draft = None;
                    next.workspace.diagnostic =
                        Some("compose_abstained: topology was undeterminable".into());
                    next.ledger.push("builder:compose:abstained".into());
                    next.stage = BuildStage::Decompose;
                } else {
                    next.workspace.draft = Some(
                        serde_json::to_value(CompositionDraft { tree, seams })
                            .map_err(|error| RuntimeError::Invalid(error.to_string()))?,
                    );
                    next.stage = BuildStage::Validate;
                }
            }
            BuildReactionOutput::Decomposition {
                verdict,
                undeterminable,
                children,
                seams,
                parent_complexity,
                child_complexities,
                detail,
            } => {
                if undeterminable {
                    next.workspace.diagnostic = Some(if detail.is_empty() {
                        "decomposition was undeterminable from admitted evidence".into()
                    } else {
                        detail
                    });
                    next.ledger.push("builder:decompose:undeterminable".into());
                    next.stage = BuildStage::Fail;
                } else {
                    match verdict {
                        DecompositionVerdict::Children => {
                            next.workspace.children = children;
                            next.workspace.decomposition_seams = seams;
                            next.workspace.parent_complexity = parent_complexity;
                            next.workspace.child_complexities = child_complexities;
                            next.workspace.diagnostic = (!detail.is_empty()).then_some(detail);
                            next.stage = BuildStage::AdmitChildren;
                        }
                        DecompositionVerdict::Atomic | DecompositionVerdict::Cannot => {
                            next.workspace.diagnostic = Some(if detail.is_empty() {
                                "no admitted primitive can satisfy the atomic capability".into()
                            } else {
                                detail
                            });
                            next.stage = BuildStage::Fail;
                        }
                    }
                }
            }
        }
        jobs.commit(tenant, record, next)
            .map(|record| record.job)
            .map_err(artifact_error)
    }

    fn reject_build_reaction(
        &self,
        tenant: &str,
        build_id: &str,
        reaction_id: &str,
        usage: BuildReactionUsage,
        detail: &str,
    ) -> Result<BuildJob, RuntimeError> {
        let mut jobs =
            BuildJobRepository::open(self.inner.store.clone()).map_err(artifact_error)?;
        let record = jobs
            .get(tenant, build_id)
            .map_err(artifact_error)?
            .ok_or_else(|| RuntimeError::NotFound(build_id.into()))?;
        let reaction = record
            .job
            .reaction
            .as_ref()
            .filter(|reaction| {
                reaction.status == BuildReactionStatus::Pending
                    && reaction.reaction_id == reaction_id
            })
            .ok_or_else(|| {
                RuntimeError::Conflict("builder reaction is no longer pending".into())
            })?;
        let charge = BuildBudgetRepository::open(self.inner.store.clone())
            .map_err(artifact_error)?
            .charge(
                tenant,
                &record.job.lineage.root_build_id,
                BuildBudgetCharge {
                    build_id: record.job.build_id.clone(),
                    reaction_id: reaction.reaction_id.clone(),
                    model_calls: usage.model_calls,
                    tokens: usage.tokens,
                    reported_wall_ms: usage.wall_ms,
                },
            );
        let budget_exhausted = matches!(
            charge,
            Err(ArtifactError::Invalid(ref detail)) if detail == "shared build budget exhausted"
        );
        if !budget_exhausted {
            charge.map_err(artifact_error)?;
        }
        let mut next = record.job.clone();
        next.cost.llm_calls = next.cost.llm_calls.saturating_add(usage.model_calls);
        next.cost.tokens = next.cost.tokens.saturating_add(usage.tokens);
        next.cost.wall_ms = next.cost.wall_ms.saturating_add(usage.wall_ms);
        next.reaction = None;
        next.workspace.diagnostic = Some(if budget_exhausted {
            "budget_exhausted: model response could not be repaired within budget".into()
        } else {
            format!("oracle_schema_invalid: {detail}")
        });
        next.ledger.push(format!(
            "reaction_rejected:{}:{}",
            reaction_id,
            if budget_exhausted {
                "budget_exhausted"
            } else {
                "closed_schema"
            }
        ));
        next.stage = BuildStage::Fail;
        jobs.commit(tenant, record, next)
            .map(|record| record.job)
            .map_err(artifact_error)
    }

    fn advance_admit(
        &self,
        jobs: &mut BuildJobRepository<aelio_store::EmbeddedStore>,
        tenant: &str,
        record: BuildJobRecord,
    ) -> Result<BuildAction, RuntimeError> {
        record.job.spec.validate().map_err(artifact_error)?;
        if let Err(error) = self.resolve_build_imprints_and_examples(tenant, &record.job.spec) {
            let (reason, detail) = match error {
                RuntimeError::NotFound(detail) => ("unknown_imprint", detail),
                other => ("example_type_mismatch", other.to_string()),
            };
            return self.fail_job(jobs, tenant, record, "admit", reason, &detail);
        }
        let interface = serde_json::json!({
            "inputs": record.job.spec.draft.inputs,
            "output": record.job.spec.draft.output,
        });
        let interface_hash = value_hash(
            &aelio_kernel::json_from(&interface)
                .map_err(|error| RuntimeError::Invalid(error.to_string()))?,
        );
        let artifacts = self.artifact_repository()?;
        let same_name: Vec<_> = artifacts
            .list(tenant, 1_000)
            .map_err(artifact_error)?
            .into_iter()
            .filter(|candidate| candidate.artifact.id == record.job.spec.draft.name)
            .collect();
        if same_name.iter().any(|candidate| {
            candidate.artifact.interface.inputs != record.job.spec.draft.inputs
                || candidate.artifact.interface.output != record.job.spec.draft.output
        }) {
            return self.fail_job(
                jobs,
                tenant,
                record,
                "admit",
                "name_collision",
                "artifact name already exists with a different interface",
            );
        }
        let version = same_name
            .iter()
            .map(|record| record.artifact.version)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let ttl = i64::try_from(record.job.spec.draft.budget.max_wall_ms)
            .map_err(|_| RuntimeError::Invalid("build wall budget is invalid".into()))?;
        NameLeaseRepository::open(self.inner.store.clone())
            .map_err(artifact_error)?
            .acquire(
                tenant,
                LeaseRequest {
                    name: &record.job.spec.draft.name,
                    interface_hash: &interface_hash,
                    spec_hash: &record.job.spec.spec_hash,
                    build_id: &record.job.build_id,
                    ttl_ms: ttl,
                },
            )
            .map_err(artifact_error)?;
        let mut next = record.job.clone();
        next.workspace.interface_hash = interface_hash;
        next.workspace.version = version;
        next.cost.depth_reached = 1;
        next.ledger.push("builder:admit:ok".into());
        next.stage = BuildStage::Resolve;
        commit_progress(jobs, tenant, record, next)
    }

    fn advance_resolve(
        &self,
        jobs: &mut BuildJobRepository<aelio_store::EmbeddedStore>,
        tenant: &str,
        record: BuildJobRecord,
    ) -> Result<BuildAction, RuntimeError> {
        let exact = self
            .artifact_repository()?
            .list(tenant, 1_000)
            .map_err(artifact_error)?
            .into_iter()
            .find(|candidate| {
                candidate.status == ArtifactStatus::Promoted
                    && candidate
                        .artifact
                        .provenance
                        .metadata
                        .get("spec_hash")
                        .and_then(Json::as_str)
                        == Some(record.job.spec.spec_hash.as_str())
            });
        if let Some(exact) = exact {
            let mut next = record.job.clone();
            self.sync_shared_cost(tenant, &mut next)?;
            let result = BuildResult::Reused {
                spec_hash: next.spec.spec_hash.clone(),
                cost: next.cost.clone(),
                ledger: next.ledger.clone(),
                artifact_pin: exact.artifact.key(),
            };
            next.result = Some(result.clone());
            next.stage = BuildStage::Complete;
            let committed = jobs.commit(tenant, record, next).map_err(artifact_error)?;
            release_lease(self, tenant, &committed.job)?;
            return Ok(BuildAction::Complete {
                job: committed.job,
                result,
            });
        }
        let mut next = record.job.clone();
        next.ledger.push("builder:resolve:no_exact".into());
        next.stage = BuildStage::Search;
        commit_progress(jobs, tenant, record, next)
    }

    fn advance_search(
        &self,
        jobs: &mut BuildJobRepository<aelio_store::EmbeddedStore>,
        tenant: &str,
        record: BuildJobRecord,
    ) -> Result<BuildAction, RuntimeError> {
        let allowed: HashSet<_> = record
            .job
            .spec
            .draft
            .policy
            .allowed_effects
            .iter()
            .copied()
            .collect();
        let supplied: HashSet<_> = record
            .job
            .spec
            .draft
            .inputs
            .iter()
            .map(|input| (input.name.as_str(), input.imprint.as_str()))
            .collect();
        let eligible: Vec<_> = self
            .artifact_repository()?
            .list(tenant, 1_000)
            .map_err(artifact_error)?
            .into_iter()
            .filter(|candidate| {
                matches!(
                    candidate.status,
                    ArtifactStatus::Canary | ArtifactStatus::Promoted
                ) && candidate.artifact.class == crate::ArtifactClass::Flow
                    && candidate.artifact.interface.output == record.job.spec.draft.output
                    && candidate
                        .artifact
                        .effects
                        .iter()
                        .all(|effect| allowed.contains(effect))
                    && candidate
                        .artifact
                        .interface
                        .inputs
                        .iter()
                        .filter(|input| input.required)
                        .all(|input| {
                            supplied.contains(&(input.name.as_str(), input.imprint.as_str()))
                        })
            })
            .collect();
        let mut measured = self.prism_rank_build_candidates(tenant, &record.job, eligible)?;
        let limit = if record.job.workspace.widened { 50 } else { 25 };
        measured.truncate(limit);
        let candidates = measured
            .iter()
            .map(|candidate| candidate.pin.clone())
            .collect::<Vec<_>>();
        if let Some(pin) = candidates.first() {
            if self.verify_reuse_candidate(tenant, &record.job, pin)? {
                let mut next = record.job.clone();
                self.sync_shared_cost(tenant, &mut next)?;
                next.ledger.push(format!("builder:resolve:verified:{pin}"));
                let result = BuildResult::Reused {
                    spec_hash: next.spec.spec_hash.clone(),
                    cost: next.cost.clone(),
                    ledger: next.ledger.clone(),
                    artifact_pin: pin.clone(),
                };
                next.result = Some(result.clone());
                next.stage = BuildStage::Complete;
                let committed = jobs.commit(tenant, record, next).map_err(artifact_error)?;
                release_lease(self, tenant, &committed.job)?;
                return Ok(BuildAction::Complete {
                    job: committed.job,
                    result,
                });
            }
        }
        let mut next = record.job.clone();
        next.workspace.candidates = candidates;
        next.workspace.candidate_measurements = measured;
        next.ledger.push(format!(
            "builder:search:{}",
            next.workspace.candidates.len()
        ));
        next.stage = if next.workspace.candidates.is_empty() {
            BuildStage::Decompose
        } else {
            BuildStage::Select
        };
        commit_progress(jobs, tenant, record, next)
    }

    fn prism_rank_build_candidates(
        &self,
        tenant: &str,
        job: &BuildJob,
        mut eligible: Vec<crate::ArtifactRecord>,
    ) -> Result<Vec<crate::BuildCandidateMeasurement>, RuntimeError> {
        if eligible.is_empty() {
            return Ok(Vec::new());
        }
        eligible.sort_by_key(|record| record.artifact.key());
        let scan_cap = if job.workspace.widened { 1_000 } else { 250 };
        eligible.truncate(scan_cap);

        let embedding_space = if self.inner.host.is_some() {
            "aelio.embedding.semantic@1"
        } else {
            "aelio.embedding.lexical@1"
        };
        let embed = |text: &str, suffix: &str| -> Result<Vec<f32>, RuntimeError> {
            if self.inner.host.is_some() {
                self.embed_model_text(crate::ModelEmbeddingRequest {
                    tenant,
                    text,
                    model: embedding_space,
                    correlation: &format!("builder:{}:embed:{suffix}", job.build_id),
                })
            } else {
                Ok(pinned_lexical_embedding(text, 256))
            }
        };
        let query_vector = embed(&job.spec.draft.description, "query")?;
        let dimension = u16::try_from(query_vector.len()).map_err(|_| {
            RuntimeError::Invalid("builder embedding dimension exceeds database limit".into())
        })?;
        let mut candidate_vectors = BTreeMap::new();
        for (index, candidate) in eligible.iter().enumerate() {
            let vector = embed(&candidate.artifact.description, &index.to_string())?;
            if vector.len() != query_vector.len() {
                return Err(RuntimeError::Host(format!(
                    "embedding space `{embedding_space}` changed dimension within one search"
                )));
            }
            candidate_vectors.insert(candidate.artifact.key(), vector);
        }

        let namespace_hash =
            blake3::hash(format!("{tenant}:{}:{}", job.build_id, job.revision).as_bytes())
                .to_hex()
                .to_string();
        let namespace = SearchNamespace::create(
            self.inner
                .data_dir
                .join("sandbox")
                .join("builder-search")
                .join(&namespace_hash[..32]),
        )?;
        let mut database = Database::create(namespace.path())
            .map_err(|error| RuntimeError::Store(error.to_string()))?;
        database
            .create_table(
                "artifacts",
                &[
                    ("pin", ColumnKind::Utf8),
                    ("description", ColumnKind::Text),
                    ("embedding", ColumnKind::Vector(dimension)),
                ],
            )
            .map_err(|error| RuntimeError::Store(error.to_string()))?;
        for candidate in &eligible {
            let pin = candidate.artifact.key();
            database
                .insert(
                    "artifacts",
                    &[
                        ("pin", DbValue::Utf8(pin.clone())),
                        (
                            "description",
                            DbValue::Utf8(candidate.artifact.description.clone()),
                        ),
                        (
                            "embedding",
                            DbValue::Vector(
                                candidate_vectors
                                    .get(&pin)
                                    .expect("vector created for every candidate")
                                    .clone(),
                            ),
                        ),
                    ],
                )
                .map_err(|error| RuntimeError::Store(error.to_string()))?;
        }
        let limit = u64::try_from(eligible.len()).unwrap_or(1_000).max(1);
        let selected = serde_json::json!(["pin", "description"]);
        let text_query = parse_prism(&serde_json::json!({
            "from":"artifacts",
            "match":{"kind":"text","on":"description","query":job.spec.draft.description},
            "select":selected,
            "limit":limit
        }))
        .map_err(RuntimeError::Invalid)?;
        let vector_query = parse_prism(&serde_json::json!({
            "from":"artifacts",
            "match":{"kind":"vector","on":"embedding","vector":query_vector},
            "select":["pin","description"],
            "limit":limit
        }))
        .map_err(RuntimeError::Invalid)?;
        let fused_query = parse_prism(&serde_json::json!({
            "from":"artifacts",
            "match":[
                {"kind":"text","on":"description","query":job.spec.draft.description},
                {"kind":"vector","on":"embedding","vector":query_vector},
                {"kind":"fusion","method":"rrf","rrf_k":60.0}
            ],
            "select":["pin","description"],
            "limit":limit
        }))
        .map_err(RuntimeError::Invalid)?;
        let models = BTreeMap::from([("embedding".into(), embedding_space.into())]);
        let execute = |query| {
            execute_prism(&database, query, &models, None)
                .map_err(|error| RuntimeError::Store(error.to_string()))
        };
        let text_hits = execute(&text_query)?;
        let vector_hits = execute(&vector_query)?;
        let fused_hits = execute(&fused_query)?;
        let ranks = |hits: &[aelio_db_query::PrismHit]| {
            hits.iter()
                .enumerate()
                .filter_map(|(index, hit)| match hit.fields.get("pin") {
                    Some(DbValue::Utf8(pin)) => {
                        Some((pin.clone(), u32::try_from(index + 1).unwrap_or(u32::MAX)))
                    }
                    _ => None,
                })
                .collect::<BTreeMap<_, _>>()
        };
        let text_ranks = ranks(&text_hits);
        let vector_ranks = ranks(&vector_hits);
        let records = eligible
            .into_iter()
            .map(|record| (record.artifact.key(), record))
            .collect::<BTreeMap<_, _>>();
        let missing_rank = u32::try_from(records.len() + 1).unwrap_or(u32::MAX);
        let mut measured = Vec::with_capacity(fused_hits.len());
        for hit in fused_hits {
            let Some(DbValue::Utf8(pin)) = hit.fields.get("pin") else {
                return Err(RuntimeError::Store(
                    "Prism artifact projection lacks pin".into(),
                ));
            };
            let candidate = records
                .get(pin)
                .ok_or_else(|| RuntimeError::Store("Prism returned an unknown artifact".into()))?;
            measured.push(crate::BuildCandidateMeasurement {
                pin: pin.clone(),
                embedding_space: embedding_space.into(),
                measured_similarity: vector_cosine(
                    &query_vector,
                    candidate_vectors
                        .get(pin)
                        .expect("ranked candidate retains vector"),
                ),
                rrf_score: f64::from(hit.score),
                text_rank: text_ranks.get(pin).copied().unwrap_or(missing_rank),
                vector_rank: vector_ranks.get(pin).copied().unwrap_or(missing_rank),
                interface: candidate.artifact.interface.clone(),
                description: candidate.artifact.description.clone(),
            });
        }
        Ok(measured)
    }

    fn prepare_reaction(
        &self,
        jobs: &mut BuildJobRepository<aelio_store::EmbeddedStore>,
        tenant: &str,
        record: BuildJobRecord,
        kind: BuildReactionKind,
    ) -> Result<BuildAction, RuntimeError> {
        if let Err(error) =
            BuildBudgetRepository::open(self.inner.store.clone()).and_then(|repository| {
                repository.ensure_dispatch_allowed(tenant, &record.job.lineage.root_build_id)
            })
        {
            return match error {
                ArtifactError::Invalid(detail) if detail == "shared build budget exhausted" => self
                    .fail_job(
                        jobs,
                        tenant,
                        record,
                        "budget",
                        "budget_exhausted",
                        "shared recursive build budget is exhausted",
                    ),
                other => Err(artifact_error(other)),
            };
        }
        let rendered_prompt = self.render_build_reaction(&record.job, kind)?;
        let prompt = builder_prompt(kind);
        self.verify_vendor_prompt(&prompt)?;
        let payload = reaction_payload(&record.job, kind)?;
        let input_hash = value_hash(
            &aelio_kernel::json_from(&payload)
                .map_err(|error| RuntimeError::Invalid(error.to_string()))?,
        );
        let mut registry = TemplateRegistry::default();
        registry
            .register(prompt.to_template())
            .map_err(RuntimeError::Invalid)?;
        let composed = registry
            .compose(
                &prompt.key(),
                &SolValue::map([("payload", SolValue::str(payload.to_string()))]),
            )
            .map_err(RuntimeError::Invalid)?;
        debug_assert_eq!(rendered_prompt, composed.text);
        let reaction = BuildReaction {
            reaction_id: format!(
                "reaction.{}.{}",
                reaction_name(kind),
                record.job.cost.llm_calls + 1
            ),
            kind,
            prompt_pin: prompt.key(),
            input_hash,
            prompt_hash: composed.prompt_hash,
            status: BuildReactionStatus::Pending,
            output_hash: None,
        };
        let mut next = record.job.clone();
        next.reaction = Some(reaction.clone());
        next.ledger
            .push(format!("reaction_intent:{}", reaction.reaction_id));
        let committed = jobs.commit(tenant, record, next).map_err(artifact_error)?;
        Ok(BuildAction::Oracle {
            job: committed.job,
            reaction,
            rendered_prompt,
        })
    }

    fn render_build_reaction(
        &self,
        job: &BuildJob,
        kind: BuildReactionKind,
    ) -> Result<String, RuntimeError> {
        let prompt = builder_prompt(kind);
        let payload = reaction_payload(job, kind)?;
        let mut registry = TemplateRegistry::default();
        registry
            .register(prompt.to_template())
            .map_err(RuntimeError::Invalid)?;
        registry
            .compose(
                &prompt.key(),
                &SolValue::map([("payload", SolValue::str(payload.to_string()))]),
            )
            .map(|composed| composed.text)
            .map_err(RuntimeError::Invalid)
    }

    fn advance_validate(
        &self,
        jobs: &mut BuildJobRepository<aelio_store::EmbeddedStore>,
        tenant: &str,
        record: BuildJobRecord,
    ) -> Result<BuildAction, RuntimeError> {
        let proposal: CompositionDraft = serde_json::from_value(
            record
                .job
                .workspace
                .draft
                .clone()
                .ok_or_else(|| RuntimeError::Internal("compose produced no draft".into()))?,
        )
        .map_err(|error| RuntimeError::Invalid(format!("compose output: {error}")))?;
        let harness = match self.validate_composition_graph(tenant, &record.job, &proposal) {
            Ok(harness) => harness,
            Err(error @ (RuntimeError::Store(_) | RuntimeError::Internal(_))) => return Err(error),
            Err(error) => {
                let class =
                    if matches!(error, RuntimeError::NotFound(_) | RuntimeError::Conflict(_)) {
                        RetryClass::Widen
                    } else {
                        RetryClass::Recompose
                    };
                return self.retry_or_decompose(
                    jobs,
                    tenant,
                    record,
                    class,
                    "composition_not_closed",
                    &error.to_string(),
                );
            }
        };
        let mut next = record.job.clone();
        next.workspace.harness = Some(harness);
        next.ledger.push("builder:validate:harness_closed".into());
        next.stage = BuildStage::Assemble;
        commit_progress(jobs, tenant, record, next)
    }

    fn advance_admit_children(
        &self,
        jobs: &mut BuildJobRepository<aelio_store::EmbeddedStore>,
        tenant: &str,
        record: BuildJobRecord,
    ) -> Result<BuildAction, RuntimeError> {
        let children = record.job.workspace.children.clone();
        let root_budget = BuildBudgetRepository::open(self.inner.store.clone())
            .map_err(artifact_error)?
            .get(tenant, &record.job.lineage.root_build_id)
            .map_err(artifact_error)?;
        if children.is_empty()
            || children.len() > usize::from(record.job.spec.draft.budget.max_children)
            || record.job.lineage.depth >= record.job.spec.draft.budget.max_depth
            || children.iter().any(|child| {
                child.draft.scope.tenant != tenant
                    || child.draft.scope.registries != record.job.spec.draft.scope.registries
                    || child.draft.budget.max_llm_calls != root_budget.max_llm_calls
                    || child.draft.budget.max_tokens != root_budget.max_tokens
                    || child.draft.budget.max_wall_ms != root_budget.max_wall_ms
                    || child.draft.policy.allowed_effects.iter().any(|effect| {
                        !record
                            .job
                            .spec
                            .draft
                            .policy
                            .allowed_effects
                            .contains(effect)
                    })
            })
        {
            return self.fail_job(
                jobs,
                tenant,
                record,
                "admit_children",
                "interface_not_closed",
                "decomposition violates tenant, breadth, depth, or effect caps",
            );
        }
        let mut hashes = HashSet::new();
        let mut ancestors = record.job.lineage.ancestor_spec_hashes.clone();
        ancestors.push(record.job.spec.spec_hash.clone());
        if children.iter().any(|child| {
            !hashes.insert(child.spec_hash.as_str()) || ancestors.contains(&child.spec_hash)
        }) {
            return self.fail_job(
                jobs,
                tenant,
                record,
                "admit_children",
                "oscillation",
                "decomposition repeats parent or sibling specs",
            );
        }
        let (resolved_seams, topological_children) = match self
            .validate_decomposition_graph(tenant, &record.job)
        {
            Ok(graph) => graph,
            Err(error @ (RuntimeError::Store(_) | RuntimeError::Internal(_))) => return Err(error),
            Err(error) => {
                return self.fail_job(
                    jobs,
                    tenant,
                    record,
                    "admit_children",
                    "interface_not_closed",
                    &error.to_string(),
                )
            }
        };
        let mut child_build_ids = Vec::with_capacity(children.len());
        let children_by_name: BTreeMap<_, _> = children
            .iter()
            .map(|child| (child.draft.name.as_str(), child))
            .collect();
        for child_name in &topological_children {
            let child = children_by_name
                .get(child_name.as_str())
                .expect("validated topological child names");
            let child_record = jobs
                .create_child(
                    tenant,
                    (*child).clone(),
                    &record.job.lineage.root_build_id,
                    &record.job.build_id,
                    ancestors.clone(),
                    record.job.lineage.depth.saturating_add(1),
                )
                .map_err(artifact_error)?;
            child_build_ids.push(child_record.job.build_id);
        }
        let mut next = record.job.clone();
        next.workspace.resolved_seams = resolved_seams;
        next.workspace.topological_children = topological_children;
        next.workspace.child_build_ids = child_build_ids;
        next.cost.depth_reached = next
            .cost
            .depth_reached
            .max(next.lineage.depth.saturating_add(1));
        next.ledger
            .push(format!("builder:children:admitted:{}", children.len()));
        next.stage = BuildStage::Recurse;
        commit_progress(jobs, tenant, record, next)
    }

    fn advance_recurse(
        &self,
        jobs: &mut BuildJobRepository<aelio_store::EmbeddedStore>,
        tenant: &str,
        record: BuildJobRecord,
    ) -> Result<BuildAction, RuntimeError> {
        let mut pins = Vec::new();
        if record.job.workspace.child_build_ids.len() != record.job.workspace.children.len() {
            return Err(RuntimeError::Internal(
                "child build id/spec cardinality mismatch".into(),
            ));
        }
        for child_id in &record.job.workspace.child_build_ids {
            let Some(job) = self.get_build(tenant, child_id)? else {
                return Err(RuntimeError::Internal(
                    "admitted child job disappeared".into(),
                ));
            };
            match job.result {
                Some(BuildResult::Built { artifact, .. }) => pins.push(artifact.key()),
                Some(BuildResult::Reused { artifact_pin, .. }) => pins.push(artifact_pin),
                Some(BuildResult::Failed { .. }) => {
                    return self.fail_job(
                        jobs,
                        tenant,
                        record,
                        "recurse",
                        "child_failed",
                        "one or more child builds failed",
                    )
                }
                None => {
                    return Ok(BuildAction::Progress { job: record.job });
                }
            }
        }
        let mut next = record.job.clone();
        next.workspace.child_artifacts = pins;
        let harness = crate::HarnessBody {
            nodes: next
                .workspace
                .topological_children
                .iter()
                .cloned()
                .zip(next.workspace.child_artifacts.iter().cloned())
                .map(|(nid, artifact)| crate::HarnessNode { nid, artifact })
                .collect(),
            seams: next.workspace.resolved_seams.clone(),
        };
        harness.validate_shape().map_err(artifact_error)?;
        next.workspace.harness = Some(harness);
        next.ledger.push("builder:recurse:children_complete".into());
        next.stage = BuildStage::Assemble;
        commit_progress(jobs, tenant, record, next)
    }

    fn advance_assemble(
        &self,
        jobs: &mut BuildJobRepository<aelio_store::EmbeddedStore>,
        tenant: &str,
        record: BuildJobRecord,
    ) -> Result<BuildAction, RuntimeError> {
        let mut next = record.job.clone();
        if let Some(harness) = &next.workspace.harness {
            let artifact = self.assemble_harness_artifact(tenant, &next, harness)?;
            next.workspace.assembled_hash = Some(artifact.hash.clone());
            self.artifact_repository()?
                .put_proposed(tenant, artifact, crate::ArtifactActor::System)
                .map_err(artifact_error)?;
            next.ledger
                .push("builder:assemble:harness_immutable".into());
        } else {
            next.ledger.push("builder:assemble:flow_immutable".into());
        }
        next.stage = BuildStage::Gate;
        commit_progress(jobs, tenant, record, next)
    }

    fn advance_gate(
        &self,
        jobs: &mut BuildJobRepository<aelio_store::EmbeddedStore>,
        tenant: &str,
        record: BuildJobRecord,
        deployer: Option<String>,
    ) -> Result<BuildAction, RuntimeError> {
        let cases = build_cases(&record.job.spec)?;
        let limits = SandboxLimits {
            max_reactions: usize::try_from(record.job.spec.draft.budget.max_reactions)
                .map_err(|_| RuntimeError::Invalid("reaction budget is invalid".into()))?,
            max_wall_ms: record.job.spec.draft.budget.max_wall_ms,
        };
        let gate = GenericArtifactGate::new(self.inner.store.clone());
        let report = if let Some(harness) = &record.job.workspace.harness {
            gate.gate_stored_harness(
                tenant,
                &record.job.spec.draft.name,
                record.job.workspace.version,
                harness,
                &cases,
                limits,
                deployer,
            )
        } else {
            let draft = record.job.workspace.draft.clone().ok_or_else(|| {
                RuntimeError::Internal("flow build reached gate without a draft".into())
            })?;
            let flow: FlowPush = serde_json::from_value(draft)
                .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
            gate.gate_stored_flow(&flow, &cases, limits, deployer)
        };
        let report = match report {
            Ok(report) => report,
            Err(error @ (RuntimeError::Store(_) | RuntimeError::Internal(_))) => return Err(error),
            Err(error) => {
                return self.fail_job(
                    jobs,
                    tenant,
                    record,
                    "gate",
                    "examples_failed",
                    &error.to_string(),
                )
            }
        };
        if !report.report.cases.iter().all(|case| case.agreed)
            || !matches!(
                report.record.status,
                ArtifactStatus::Canary | ArtifactStatus::Promoted
            )
        {
            return self.fail_job(
                jobs,
                tenant,
                record,
                "gate",
                "examples_failed",
                "one or more declared examples failed or required approval was absent",
            );
        }
        let gate_snapshot = serde_json::to_value(&report.evidence)
            .map_err(|error| RuntimeError::Internal(error.to_string()))?;
        let gate_verdict_hash = value_hash(
            &aelio_kernel::json_from(&gate_snapshot)
                .map_err(|error| RuntimeError::Internal(error.to_string()))?,
        );
        let artifact = report.record.artifact;
        let mut next = record.job.clone();
        self.sync_shared_cost(tenant, &mut next)?;
        next.ledger
            .push(format!("builder:gate:{gate_verdict_hash}"));
        let result = BuildResult::Built {
            spec_hash: next.spec.spec_hash.clone(),
            cost: next.cost.clone(),
            ledger: next.ledger.clone(),
            artifact: Box::new(artifact),
            gate_verdict_hash,
        };
        next.result = Some(result.clone());
        next.stage = BuildStage::Complete;
        let committed = jobs.commit(tenant, record, next).map_err(artifact_error)?;
        release_lease(self, tenant, &committed.job)?;
        Ok(BuildAction::Complete {
            job: committed.job,
            result,
        })
    }

    fn advance_fail(
        &self,
        jobs: &mut BuildJobRepository<aelio_store::EmbeddedStore>,
        tenant: &str,
        record: BuildJobRecord,
    ) -> Result<BuildAction, RuntimeError> {
        let detail = record
            .job
            .workspace
            .diagnostic
            .clone()
            .unwrap_or_else(|| "builder failed without a diagnostic".into());
        let mut next = record.job.clone();
        self.sync_shared_cost(tenant, &mut next)?;
        next.ledger.push("builder:fail".into());
        let demand = CapabilityRequestRepository::open(self.inner.store.clone())
            .map_err(artifact_error)?
            .record(
                tenant,
                CapabilityRequestDraft {
                    normalized_need: normalize_need(
                        &next.spec.draft.description,
                        &next.spec.draft.name,
                    ),
                    inputs: next.spec.draft.inputs.clone(),
                    output: next.spec.draft.output.clone(),
                    allowed_effects: next.spec.draft.policy.allowed_effects.clone(),
                    requester: next.build_id.clone(),
                    reason: CapabilityReason::MissingCapability,
                    evidence_refs: next.ledger.iter().rev().take(8).cloned().collect(),
                },
            )
            .map_err(artifact_error)?;
        let result = BuildResult::Failed {
            spec_hash: next.spec.spec_hash.clone(),
            cost: next.cost.clone(),
            ledger: next.ledger.clone(),
            failure: BuildFailure {
                stage: "fail".into(),
                spec_path: vec![next.spec.draft.name.clone()],
                reason_code: "missing_capability".into(),
                detail,
                missing: vec![next.spec.draft.output.clone()],
                insufficiency_ids: vec![demand.request_id],
            },
        };
        next.result = Some(result.clone());
        next.stage = BuildStage::Complete;
        let committed = jobs.commit(tenant, record, next).map_err(artifact_error)?;
        release_lease(self, tenant, &committed.job)?;
        Ok(BuildAction::Complete {
            job: committed.job,
            result,
        })
    }

    fn fail_job(
        &self,
        jobs: &mut BuildJobRepository<aelio_store::EmbeddedStore>,
        tenant: &str,
        record: BuildJobRecord,
        stage: &str,
        reason: &str,
        detail: &str,
    ) -> Result<BuildAction, RuntimeError> {
        let mut next = record.job.clone();
        next.workspace.diagnostic = Some(format!("{stage}/{reason}: {detail}"));
        next.ledger.push(format!("builder:{stage}:{reason}"));
        next.stage = BuildStage::Fail;
        commit_progress(jobs, tenant, record, next)
    }

    fn sync_shared_cost(&self, tenant: &str, job: &mut BuildJob) -> Result<(), RuntimeError> {
        let account = BuildBudgetRepository::open(self.inner.store.clone())
            .map_err(artifact_error)?
            .get(tenant, &job.lineage.root_build_id)
            .map_err(artifact_error)?;
        job.cost.llm_calls = account.llm_calls;
        job.cost.tokens = account.tokens;
        job.cost.wall_ms = account
            .charges
            .values()
            .try_fold(0_u64, |total, charge| {
                total.checked_add(charge.reported_wall_ms)
            })
            .ok_or_else(|| RuntimeError::Internal("reported build wall time overflow".into()))?;
        job.cost.depth_reached = job.cost.depth_reached.max(job.lineage.depth);
        Ok(())
    }

    fn assemble_harness_artifact(
        &self,
        tenant: &str,
        job: &BuildJob,
        harness: &crate::HarnessBody,
    ) -> Result<crate::Artifact, RuntimeError> {
        harness.validate_shape().map_err(artifact_error)?;
        let repository = self.artifact_repository()?;
        let mut effects = HashSet::new();
        let mut reviewed = false;
        for node in &harness.nodes {
            let (id, version) = parse_artifact_pin(&node.artifact)?;
            let child = repository
                .get(tenant, id, version)
                .map_err(artifact_error)?
                .ok_or_else(|| RuntimeError::NotFound(node.artifact.clone()))?;
            if !matches!(
                child.status,
                ArtifactStatus::Canary | ArtifactStatus::Promoted
            ) {
                return Err(RuntimeError::Conflict(format!(
                    "child artifact `{}` is no longer executable",
                    node.artifact
                )));
            }
            effects.extend(child.artifact.effects.iter().copied());
            reviewed |= child.artifact.tier != crate::ArtifactTier::Auto;
        }
        if effects.is_empty() {
            effects.insert(ArtifactEffect::Pure);
        }
        let mut effects: Vec<_> = effects.into_iter().collect();
        effects.sort_by_key(|effect| match effect {
            ArtifactEffect::Pure => 0,
            ArtifactEffect::Read => 1,
            ArtifactEffect::Write => 2,
            ArtifactEffect::External => 3,
        });
        reviewed |= effects
            .iter()
            .any(|effect| matches!(effect, ArtifactEffect::Write | ArtifactEffect::External));
        let converters: BTreeSet<_> = harness
            .seams
            .iter()
            .filter_map(|seam| seam.converter.clone())
            .collect();
        let examples = job
            .spec
            .draft
            .examples
            .iter()
            .map(|example| {
                serde_json::json!({
                    "inputs": example.inputs,
                    "output": example.output,
                    "negative": example.negative,
                    "fixtures": example.fixtures,
                })
            })
            .collect();
        crate::Artifact::new(
            job.spec.draft.name.clone(),
            job.workspace.version,
            crate::ArtifactClass::Harness,
            if reviewed {
                crate::ArtifactTier::Reviewed
            } else {
                crate::ArtifactTier::Auto
            },
            "1",
            env!("CARGO_PKG_VERSION"),
            crate::ArtifactInterface {
                inputs: job.spec.draft.inputs.clone(),
                output: job.spec.draft.output.clone(),
            },
            job.spec.draft.description.clone(),
            vec!["builder".into(), "harness".into()],
            effects,
            crate::ArtifactPins {
                artifacts: harness
                    .nodes
                    .iter()
                    .map(|node| node.artifact.clone())
                    .collect(),
                converters: converters.into_iter().collect(),
                ..crate::ArtifactPins::default()
            },
            examples,
            serde_json::json!({"harness":harness}),
            crate::Provenance {
                built_by: Some("aelio.root_harness@2".into()),
                requester: Some(tenant.into()),
                build_id: Some(job.build_id.clone()),
                metadata: serde_json::Map::from_iter([(
                    "spec_hash".into(),
                    Json::String(job.spec.spec_hash.clone()),
                )]),
            },
        )
        .map_err(artifact_error)
    }

    fn resolve_build_imprints_and_examples(
        &self,
        tenant: &str,
        spec: &BuildSpec,
    ) -> Result<(), RuntimeError> {
        let registry =
            crate::ImprintRegistry::open(self.inner.store.clone()).map_err(artifact_error)?;
        for input in &spec.draft.inputs {
            registry
                .resolve(tenant, &input.imprint)
                .map_err(artifact_error)?;
        }
        registry
            .resolve(tenant, &spec.draft.output)
            .map_err(artifact_error)?;
        let declared: BTreeMap<_, _> = spec
            .draft
            .inputs
            .iter()
            .map(|input| (input.name.as_str(), input))
            .collect();
        for (index, example) in spec.draft.examples.iter().enumerate() {
            let inputs = example.inputs.as_object().ok_or_else(|| {
                RuntimeError::Invalid(format!(
                    "example_type_mismatch: example {index} inputs must be an object"
                ))
            })?;
            for input in &spec.draft.inputs {
                match inputs.get(&input.name) {
                    Some(value) => registry
                        .validate_value(tenant, &input.imprint, value)
                        .map_err(|error| {
                            RuntimeError::Invalid(format!(
                                "example_type_mismatch: example {index} input `{}`: {error}",
                                input.name
                            ))
                        })?,
                    None if input.required => {
                        return Err(RuntimeError::Invalid(format!(
                            "example_type_mismatch: example {index} lacks `{}`",
                            input.name
                        )))
                    }
                    None => {}
                }
            }
            if let Some(unknown) = inputs
                .keys()
                .find(|name| !declared.contains_key(name.as_str()))
            {
                return Err(RuntimeError::Invalid(format!(
                    "example_type_mismatch: example {index} has undeclared input `{unknown}`"
                )));
            }
            registry
                .validate_value(tenant, &spec.draft.output, &example.output)
                .map_err(|error| {
                    RuntimeError::Invalid(format!(
                        "example_type_mismatch: example {index} output: {error}"
                    ))
                })?;
        }
        Ok(())
    }

    fn validate_decomposition_graph(
        &self,
        tenant: &str,
        job: &BuildJob,
    ) -> Result<(Vec<crate::HarnessSeam>, Vec<String>), RuntimeError> {
        let children: BTreeMap<_, _> = job
            .workspace
            .children
            .iter()
            .map(|child| (child.draft.name.as_str(), child))
            .collect();
        if children.len() != job.workspace.children.len()
            || job.workspace.parent_complexity == 0
            || job.workspace.child_complexities.len() != children.len()
            || children.iter().any(|(name, _)| {
                !matches!(
                    job.workspace.child_complexities.get(*name),
                    Some(value) if *value > 0 && *value < job.workspace.parent_complexity
                )
            })
        {
            return Err(RuntimeError::Invalid(
                "child names/complexities must be unique, complete, positive, and strictly smaller"
                    .into(),
            ));
        }
        if job.workspace.decomposition_seams.is_empty()
            || job.workspace.decomposition_seams.len() > 1_024
        {
            return Err(RuntimeError::Invalid(
                "decomposition requires 1..=1024 seams".into(),
            ));
        }
        let parent_inputs: BTreeMap<_, _> = job
            .spec
            .draft
            .inputs
            .iter()
            .map(|input| (input.name.as_str(), input))
            .collect();
        let mut sinks = HashSet::new();
        let mut edges: BTreeMap<String, BTreeSet<String>> = children
            .keys()
            .map(|name| ((*name).to_owned(), BTreeSet::new()))
            .collect();
        let mut output_source = None;
        let mut resolved = Vec::with_capacity(job.workspace.decomposition_seams.len());
        for seam in &job.workspace.decomposition_seams {
            let (from_imprint, from_node) = match &seam.from {
                crate::HarnessSource::ParentInput { slot } => (
                    parent_inputs
                        .get(slot.as_str())
                        .ok_or_else(|| {
                            RuntimeError::Invalid(format!(
                                "seam references unknown parent input `{slot}`"
                            ))
                        })?
                        .imprint
                        .clone(),
                    None,
                ),
                crate::HarnessSource::NodeOutput { node } => (
                    children
                        .get(node.as_str())
                        .ok_or_else(|| {
                            RuntimeError::Invalid(format!(
                                "seam references unknown producer child `{node}`"
                            ))
                        })?
                        .draft
                        .output
                        .clone(),
                    Some(node.clone()),
                ),
            };
            let (to_imprint, sink_key, to_node) = match &seam.to {
                crate::HarnessSink::NodeInput { node, slot } => {
                    let input = children
                        .get(node.as_str())
                        .and_then(|child| {
                            child.draft.inputs.iter().find(|input| input.name == *slot)
                        })
                        .ok_or_else(|| {
                            RuntimeError::Invalid(format!(
                                "seam references unknown child input `{node}.{slot}`"
                            ))
                        })?;
                    (
                        input.imprint.clone(),
                        format!("node:{node}:{slot}"),
                        Some(node.clone()),
                    )
                }
                crate::HarnessSink::ParentOutput => {
                    (job.spec.draft.output.clone(), "parent:output".into(), None)
                }
            };
            if !sinks.insert(sink_key) {
                return Err(RuntimeError::Invalid(
                    "each child input and parent output must have exactly one producer".into(),
                ));
            }
            if let (Some(from), Some(to)) = (&from_node, &to_node) {
                if from == to {
                    return Err(RuntimeError::Invalid("harness self-cycle".into()));
                }
                edges
                    .get_mut(from)
                    .expect("validated child")
                    .insert(to.clone());
            }
            if matches!(seam.to, crate::HarnessSink::ParentOutput) {
                output_source = Some(from_node.clone().ok_or_else(|| {
                    RuntimeError::Invalid(
                        "decomposed parent output must be produced by a child".into(),
                    )
                })?);
            }
            let converter = if from_imprint == to_imprint {
                None
            } else {
                Some(self.resolve_glu_converter(tenant, &from_imprint, &to_imprint)?)
            };
            resolved.push(crate::HarnessSeam {
                from: seam.from.clone(),
                to: seam.to.clone(),
                from_imprint,
                to_imprint,
                converter,
            });
        }
        for (name, child) in &children {
            for input in child.draft.inputs.iter().filter(|input| input.required) {
                if !sinks.contains(&format!("node:{name}:{}", input.name)) {
                    return Err(RuntimeError::Invalid(format!(
                        "required child input `{name}.{}` is unsatisfied",
                        input.name
                    )));
                }
            }
        }
        if !sinks.contains("parent:output") {
            return Err(RuntimeError::Invalid(
                "children do not jointly produce the parent output".into(),
            ));
        }
        let order = topological_order(&edges)?;
        let output_source = output_source.expect("validated parent output source");
        let mut contributes = HashSet::from([output_source]);
        loop {
            let before = contributes.len();
            for (from, tos) in &edges {
                if tos.iter().any(|to| contributes.contains(to)) {
                    contributes.insert(from.clone());
                }
            }
            if contributes.len() == before {
                break;
            }
        }
        if contributes.len() != children.len() {
            return Err(RuntimeError::Invalid(
                "decomposition contains children that do not contribute to parent output".into(),
            ));
        }
        Ok((resolved, order))
    }

    /// Resolve a model-proposed topology into a closed executable Harness. The proposal may name
    /// only node ids, already-selected immutable artifact pins, and endpoint topology. Interfaces,
    /// effects, converter pins, ordering, and graph closure remain kernel-owned facts.
    fn validate_composition_graph(
        &self,
        tenant: &str,
        job: &BuildJob,
        proposal: &CompositionDraft,
    ) -> Result<crate::HarnessBody, RuntimeError> {
        if proposal.tree.is_empty()
            || proposal.tree.len() > 64
            || proposal.seams.is_empty()
            || proposal.seams.len() > 1_024
        {
            return Err(RuntimeError::Invalid(
                "composition requires 1..=64 nodes and 1..=1024 seams".into(),
            ));
        }
        let selected: HashSet<_> = job.workspace.selected.iter().map(String::as_str).collect();
        let allowed: HashSet<_> = job
            .spec
            .draft
            .policy
            .allowed_effects
            .iter()
            .copied()
            .collect();
        let repository = self.artifact_repository()?;
        let mut nodes = BTreeMap::new();
        for node in &proposal.tree {
            if !selected.contains(node.artifact.as_str()) {
                return Err(RuntimeError::Invalid(format!(
                    "composition named unselected artifact `{}`",
                    node.artifact
                )));
            }
            let (id, version) = parse_artifact_pin(&node.artifact)?;
            let record = repository
                .get(tenant, id, version)
                .map_err(artifact_error)?
                .ok_or_else(|| RuntimeError::NotFound(node.artifact.clone()))?;
            if !matches!(
                record.status,
                ArtifactStatus::Canary | ArtifactStatus::Promoted
            ) {
                return Err(RuntimeError::Conflict(format!(
                    "composition artifact `{}` is no longer executable",
                    node.artifact
                )));
            }
            if record
                .artifact
                .effects
                .iter()
                .any(|effect| !allowed.contains(effect))
            {
                return Err(RuntimeError::Invalid(format!(
                    "composition artifact `{}` exceeds allowed effects",
                    node.artifact
                )));
            }
            if nodes.insert(node.nid.clone(), record.artifact).is_some() {
                return Err(RuntimeError::Invalid(
                    "composition node ids must be unique".into(),
                ));
            }
        }

        let parent_inputs: BTreeMap<_, _> = job
            .spec
            .draft
            .inputs
            .iter()
            .map(|input| (input.name.as_str(), input))
            .collect();
        let mut sinks = HashSet::new();
        let mut edges: BTreeMap<String, BTreeSet<String>> = nodes
            .keys()
            .map(|nid| (nid.clone(), BTreeSet::new()))
            .collect();
        let mut output_source = None;
        let mut resolved = Vec::with_capacity(proposal.seams.len());
        for seam in &proposal.seams {
            let (from_imprint, from_node) = match &seam.from {
                crate::HarnessSource::ParentInput { slot } => (
                    parent_inputs
                        .get(slot.as_str())
                        .ok_or_else(|| {
                            RuntimeError::Invalid(format!(
                                "seam references unknown parent input `{slot}`"
                            ))
                        })?
                        .imprint
                        .clone(),
                    None,
                ),
                crate::HarnessSource::NodeOutput { node } => (
                    nodes
                        .get(node)
                        .ok_or_else(|| {
                            RuntimeError::Invalid(format!(
                                "seam references unknown producer node `{node}`"
                            ))
                        })?
                        .interface
                        .output
                        .clone(),
                    Some(node.clone()),
                ),
            };
            let (to_imprint, sink_key, to_node) = match &seam.to {
                crate::HarnessSink::NodeInput { node, slot } => {
                    let input = nodes
                        .get(node)
                        .and_then(|artifact| {
                            artifact
                                .interface
                                .inputs
                                .iter()
                                .find(|input| input.name == *slot)
                        })
                        .ok_or_else(|| {
                            RuntimeError::Invalid(format!(
                                "seam references unknown node input `{node}.{slot}`"
                            ))
                        })?;
                    (
                        input.imprint.clone(),
                        format!("node:{node}:{slot}"),
                        Some(node.clone()),
                    )
                }
                crate::HarnessSink::ParentOutput => {
                    (job.spec.draft.output.clone(), "parent:output".into(), None)
                }
            };
            if !sinks.insert(sink_key) {
                return Err(RuntimeError::Invalid(
                    "each node input and parent output must have exactly one producer".into(),
                ));
            }
            if let (Some(from), Some(to)) = (&from_node, &to_node) {
                if from == to {
                    return Err(RuntimeError::Invalid("harness self-cycle".into()));
                }
                edges
                    .get_mut(from)
                    .expect("validated composition node")
                    .insert(to.clone());
            }
            if matches!(seam.to, crate::HarnessSink::ParentOutput) {
                output_source = Some(from_node.clone().ok_or_else(|| {
                    RuntimeError::Invalid(
                        "composed parent output must be produced by an artifact node".into(),
                    )
                })?);
            }
            let converter = if from_imprint == to_imprint {
                None
            } else {
                Some(self.resolve_glu_converter(tenant, &from_imprint, &to_imprint)?)
            };
            resolved.push(crate::HarnessSeam {
                from: seam.from.clone(),
                to: seam.to.clone(),
                from_imprint,
                to_imprint,
                converter,
            });
        }
        for (nid, artifact) in &nodes {
            for input in artifact
                .interface
                .inputs
                .iter()
                .filter(|input| input.required)
            {
                if !sinks.contains(&format!("node:{nid}:{}", input.name)) {
                    return Err(RuntimeError::Invalid(format!(
                        "required node input `{nid}.{}` is unsatisfied",
                        input.name
                    )));
                }
            }
        }
        if !sinks.contains("parent:output") {
            return Err(RuntimeError::Invalid(
                "composition does not produce the parent output".into(),
            ));
        }
        let order = topological_order(&edges)?;
        let output_source = output_source.expect("validated composition output");
        let mut contributes = HashSet::from([output_source]);
        loop {
            let before = contributes.len();
            for (from, tos) in &edges {
                if tos.iter().any(|to| contributes.contains(to)) {
                    contributes.insert(from.clone());
                }
            }
            if contributes.len() == before {
                break;
            }
        }
        if contributes.len() != nodes.len() {
            return Err(RuntimeError::Invalid(
                "composition contains nodes that do not contribute to parent output".into(),
            ));
        }
        let pins: BTreeMap<_, _> = proposal
            .tree
            .iter()
            .map(|node| (node.nid.as_str(), node.artifact.as_str()))
            .collect();
        let harness = crate::HarnessBody {
            nodes: order
                .into_iter()
                .map(|nid| crate::HarnessNode {
                    artifact: pins
                        .get(nid.as_str())
                        .expect("topology node retains immutable pin")
                        .to_string(),
                    nid,
                })
                .collect(),
            seams: resolved,
        };
        harness.validate_shape().map_err(artifact_error)?;
        Ok(harness)
    }

    fn resolve_glu_converter(
        &self,
        tenant: &str,
        from: &str,
        to: &str,
    ) -> Result<String, RuntimeError> {
        let mut candidates = self
            .artifact_repository()?
            .list(tenant, 1_000)
            .map_err(artifact_error)?;
        candidates.extend(
            crate::ArtifactRepository::open(self.inner.store.clone())
                .map_err(artifact_error)?
                .list(crate::mint::VENDOR_ARTIFACT_TENANT, 1_000)
                .map_err(artifact_error)?,
        );
        candidates
            .into_iter()
            .filter(|record| {
                record.artifact.class == crate::ArtifactClass::Glu
                    && matches!(
                        record.status,
                        ArtifactStatus::Canary | ArtifactStatus::Promoted
                    )
                    && record.artifact.interface.inputs.len() == 1
                    && record.artifact.interface.inputs[0].imprint == from
                    && record.artifact.interface.output == to
            })
            .max_by(|left, right| {
                left.artifact
                    .version
                    .cmp(&right.artifact.version)
                    .then_with(|| right.artifact.id.cmp(&left.artifact.id))
            })
            .map(|record| record.artifact.key())
            .ok_or_else(|| {
                RuntimeError::Invalid(format!(
                    "no admitted Glu converter closes `{from}` -> `{to}`"
                ))
            })
    }

    fn retry_or_decompose(
        &self,
        jobs: &mut BuildJobRepository<aelio_store::EmbeddedStore>,
        tenant: &str,
        record: BuildJobRecord,
        class: RetryClass,
        reason: &str,
        detail: &str,
    ) -> Result<BuildAction, RuntimeError> {
        let mut next = record.job.clone();
        next.workspace.diagnostic = Some(format!("validate/{reason}: {detail}"));
        next.workspace.draft = None;
        let retry = match class {
            RetryClass::Widen if !next.workspace.widened => {
                next.workspace.widened = true;
                next.workspace.selected.clear();
                next.workspace.candidates.clear();
                next.workspace.candidate_measurements.clear();
                next.stage = BuildStage::Search;
                true
            }
            RetryClass::Recompose if !next.workspace.recomposed => {
                next.workspace.recomposed = true;
                next.stage = BuildStage::Compose;
                true
            }
            RetryClass::Widen | RetryClass::Recompose => {
                next.stage = BuildStage::Decompose;
                false
            }
        };
        if retry {
            next.cost.retries = next.cost.retries.saturating_add(1);
        }
        next.ledger.push(format!(
            "builder:validate:{reason}:{}",
            if retry { "retry" } else { "decompose" }
        ));
        commit_progress(jobs, tenant, record, next)
    }

    fn verify_reuse_candidate(
        &self,
        tenant: &str,
        job: &BuildJob,
        pin: &str,
    ) -> Result<bool, RuntimeError> {
        let (id, version) = parse_artifact_pin(pin)?;
        let record = self
            .artifact_repository()?
            .get(tenant, id, version)
            .map_err(artifact_error)?
            .ok_or_else(|| RuntimeError::NotFound(pin.into()))?;
        if record.artifact.interface.inputs != job.spec.draft.inputs
            || record.artifact.interface.output != job.spec.draft.output
            || record
                .artifact
                .effects
                .iter()
                .any(|effect| !job.spec.draft.policy.allowed_effects.contains(effect))
        {
            return Ok(false);
        }
        let examples_json = serde_json::to_value(&job.spec.draft.examples)
            .map_err(|error| RuntimeError::Internal(error.to_string()))?;
        let examples_hash = value_hash(
            &aelio_kernel::json_from(&examples_json)
                .map_err(|error| RuntimeError::Internal(error.to_string()))?,
        );
        let pin_context = serde_json::json!({
            "pins": record.artifact.pins,
            "scope": job.spec.draft.scope.registries,
            "effects": job.spec.draft.policy.allowed_effects,
        });
        let pin_context_hash = value_hash(
            &aelio_kernel::json_from(&pin_context)
                .map_err(|error| RuntimeError::Internal(error.to_string()))?,
        );
        let mut cache =
            VerificationCacheRepository::open(self.inner.store.clone()).map_err(artifact_error)?;
        if let Some(entry) = cache
            .get(
                tenant,
                &record.artifact.hash,
                &examples_hash,
                &pin_context_hash,
            )
            .map_err(artifact_error)?
        {
            return Ok(entry.verdict == VerificationVerdict::Pass);
        }
        let flow = flow_from_artifact(tenant, &record.artifact)?;
        let report = SandboxRunner::run_flow_with_store(
            &flow,
            &build_cases(&job.spec)?,
            SandboxLimits {
                max_reactions: usize::try_from(job.spec.draft.budget.max_reactions)
                    .map_err(|_| RuntimeError::Invalid("reaction budget is invalid".into()))?,
                max_wall_ms: job.spec.draft.budget.max_wall_ms,
            },
            &self.inner.store,
        );
        let Ok(report) = report else {
            return Ok(false);
        };
        let passed = report.cases.iter().all(|case| case.agreed);
        let report_json = serde_json::to_value(&report)
            .map_err(|error| RuntimeError::Internal(error.to_string()))?;
        let ledger_ref = value_hash(
            &aelio_kernel::json_from(&report_json)
                .map_err(|error| RuntimeError::Internal(error.to_string()))?,
        );
        cache
            .put(
                tenant,
                VerificationCacheEntry {
                    candidate_hash: record.artifact.hash,
                    examples_hash,
                    pin_context_hash,
                    verdict: if passed {
                        VerificationVerdict::Pass
                    } else {
                        VerificationVerdict::Fail
                    },
                    ledger_ref,
                },
            )
            .map_err(artifact_error)?;
        Ok(passed)
    }
}

#[derive(Debug, Clone, Copy)]
enum RetryClass {
    Widen,
    Recompose,
}

fn commit_progress(
    jobs: &mut BuildJobRepository<aelio_store::EmbeddedStore>,
    tenant: &str,
    record: BuildJobRecord,
    next: BuildJob,
) -> Result<BuildAction, RuntimeError> {
    jobs.commit(tenant, record, next)
        .map(|record| BuildAction::Progress { job: record.job })
        .map_err(artifact_error)
}

fn builder_prompt(kind: BuildReactionKind) -> PromptArtifact {
    let id = match kind {
        BuildReactionKind::Select => SELECT_PROMPT,
        BuildReactionKind::Compose => COMPOSE_PROMPT,
        BuildReactionKind::Decompose => DECOMPOSE_PROMPT,
    };
    builder_vendor_prompts()
        .into_iter()
        .find(|prompt| prompt.key() == id)
        .expect("builder prompt constants and installer agree")
}

fn reaction_name(kind: BuildReactionKind) -> &'static str {
    match kind {
        BuildReactionKind::Select => "select",
        BuildReactionKind::Compose => "compose",
        BuildReactionKind::Decompose => "decompose",
    }
}

fn reaction_payload(job: &BuildJob, kind: BuildReactionKind) -> Result<Json, RuntimeError> {
    let interface = serde_json::json!({
        "name": job.spec.draft.name,
        "description": job.spec.draft.description,
        "inputs": job.spec.draft.inputs,
        "output": job.spec.draft.output,
        "allowed_effects": job.spec.draft.policy.allowed_effects,
        "registries": job.spec.draft.scope.registries,
        "version": job.workspace.version,
    });
    Ok(match kind {
        BuildReactionKind::Select => serde_json::json!({
            "spec": interface,
            "candidates": job.workspace.candidate_measurements,
            "thresholds": {
                "min_measured_similarity": SELECT_MIN_SIMILARITY,
                "min_top_margin": SELECT_MIN_MARGIN,
                "max_normalized_entropy": SELECT_MAX_ENTROPY,
            },
        }),
        BuildReactionKind::Compose => serde_json::json!({
            "spec": interface,
            "selected": job.workspace.selected,
            "diagnostic": job.workspace.diagnostic,
        }),
        BuildReactionKind::Decompose => serde_json::json!({
            "spec": interface,
            "max_children": job.spec.draft.budget.max_children,
            "remaining_depth": job.spec.draft.budget.max_depth.saturating_sub(job.cost.depth_reached),
        }),
    })
}

fn validate_reaction_kind(
    expected: BuildReactionKind,
    output: &BuildReactionOutput,
) -> Result<(), RuntimeError> {
    let matches = matches!(
        (expected, output),
        (
            BuildReactionKind::Select,
            BuildReactionOutput::Selection { .. }
        ) | (
            BuildReactionKind::Compose,
            BuildReactionOutput::Composition { .. }
        ) | (
            BuildReactionKind::Decompose,
            BuildReactionOutput::Decomposition { .. }
        )
    );
    if matches {
        Ok(())
    } else {
        Err(RuntimeError::Invalid(
            "oracle response kind does not match durable intent".into(),
        ))
    }
}

fn parse_build_oracle_output(
    kind: BuildReactionKind,
    text: &str,
) -> Result<BuildReactionOutput, RuntimeError> {
    if text.len() > 2 * 1024 * 1024 {
        return Err(RuntimeError::Invalid(
            "builder model response exceeds 2 MiB".into(),
        ));
    }
    let mut value: Json = serde_json::from_str(text)
        .map_err(|error| RuntimeError::Invalid(format!("builder model JSON: {error}")))?;
    let object = value.as_object_mut().ok_or_else(|| {
        RuntimeError::Invalid("builder model response must be one JSON object".into())
    })?;
    let expected = match kind {
        BuildReactionKind::Select => "selection",
        BuildReactionKind::Compose => "composition",
        BuildReactionKind::Decompose => "decomposition",
    };
    match object.get("kind").and_then(Json::as_str) {
        None => {
            object.insert("kind".into(), Json::String(expected.into()));
        }
        Some(actual) if actual == expected => {}
        Some(actual) => {
            return Err(RuntimeError::Invalid(format!(
                "builder response kind `{actual}` does not match `{expected}`"
            )))
        }
    }
    serde_json::from_value(value)
        .map_err(|error| RuntimeError::Invalid(format!("builder response schema: {error}")))
}

fn validate_reaction_output(
    job: &BuildJob,
    output: &BuildReactionOutput,
) -> Result<(), RuntimeError> {
    match output {
        BuildReactionOutput::Selection {
            selected,
            runners_up,
            unmet,
            undeterminable,
        } => {
            validate_measured_selection(job, selected, runners_up, unmet, *undeterminable)?;
        }
        BuildReactionOutput::Composition {
            tree,
            seams,
            undeterminable,
        } => {
            if tree.len() > 64 || seams.len() > 1_024 {
                return Err(RuntimeError::Invalid(
                    "composition result exceeds closed bounds".into(),
                ));
            }
            if !undeterminable && (tree.is_empty() || seams.is_empty()) {
                return Err(RuntimeError::Invalid(
                    "determinate composition requires a non-empty tree and seams".into(),
                ));
            }
            let offered: HashSet<_> = job.workspace.selected.iter().map(String::as_str).collect();
            if tree
                .iter()
                .any(|node| !offered.contains(node.artifact.as_str()))
            {
                return Err(RuntimeError::Invalid(
                    "composition may name only selected immutable artifact pins".into(),
                ));
            }
        }
        BuildReactionOutput::Decomposition {
            verdict,
            undeterminable,
            children,
            seams,
            child_complexities,
            detail,
            ..
        } => {
            if children.len() > 64
                || seams.len() > 1_024
                || child_complexities.len() > 64
                || detail.len() > 4_096
            {
                return Err(RuntimeError::Invalid(
                    "decomposition result exceeds closed bounds".into(),
                ));
            }
            if *undeterminable
                && (!children.is_empty()
                    || !seams.is_empty()
                    || !child_complexities.is_empty()
                    || matches!(verdict, DecompositionVerdict::Children))
            {
                return Err(RuntimeError::Invalid(
                    "undeterminable decomposition must not invent children, seams, or complexity"
                        .into(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_measured_selection(
    job: &BuildJob,
    selected: &[SelectionChoice],
    runners_up: &[SelectionRunnerUp],
    unmet: &[String],
    undeterminable: bool,
) -> Result<Vec<String>, RuntimeError> {
    if selected.len() > 25
        || runners_up.len() > 25
        || unmet.len() > 64
        || unmet.iter().any(|need| need.is_empty() || need.len() > 512)
    {
        return Err(RuntimeError::Invalid(
            "selection result exceeds closed bounds".into(),
        ));
    }
    let offered = job
        .workspace
        .candidate_measurements
        .iter()
        .map(|candidate| (candidate.pin.as_str(), candidate.measured_similarity))
        .collect::<BTreeMap<_, _>>();
    if offered.len() != job.workspace.candidates.len() {
        return Err(RuntimeError::Invalid(
            "selection lacks kernel candidate measurements".into(),
        ));
    }
    let mut selected_ids = HashSet::new();
    for choice in selected {
        if !offered.contains_key(choice.id.as_str())
            || !selected_ids.insert(choice.id.as_str())
            || !choice.score.is_finite()
            || !(0.0..=1.0).contains(&choice.score)
            || choice.role.is_empty()
            || choice.role.len() > 256
        {
            return Err(RuntimeError::Invalid(
                "selection invented, duplicated, or malformed a candidate".into(),
            ));
        }
    }
    let mut runner_ids = HashSet::new();
    for runner in runners_up {
        if !offered.contains_key(runner.id.as_str())
            || selected_ids.contains(runner.id.as_str())
            || !runner_ids.insert(runner.id.as_str())
            || !runner.score.is_finite()
            || !(0.0..=1.0).contains(&runner.score)
        {
            return Err(RuntimeError::Invalid(
                "selection runner-up is invented, selected, duplicated, or malformed".into(),
            ));
        }
    }
    if undeterminable || selected.is_empty() {
        return Ok(Vec::new());
    }

    let similarities = selected
        .iter()
        .map(|choice| offered[choice.id.as_str()])
        .collect::<Vec<_>>();
    let minimum = similarities.iter().copied().fold(f64::INFINITY, f64::min);
    let top_selected = similarities
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let top_unselected = offered
        .iter()
        .filter(|(pin, _)| !selected_ids.contains(**pin))
        .map(|(_, similarity)| *similarity)
        .fold(0.0, f64::max);
    let entropy = normalized_softmax_entropy(&similarities);
    if minimum < SELECT_MIN_SIMILARITY
        || top_selected - top_unselected < SELECT_MIN_MARGIN
        || entropy > SELECT_MAX_ENTROPY
    {
        return Ok(Vec::new());
    }
    Ok(selected.iter().map(|choice| choice.id.clone()).collect())
}

fn normalized_softmax_entropy(values: &[f64]) -> f64 {
    if values.len() <= 1 {
        return 0.0;
    }
    let maximum = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let weights = values
        .iter()
        .map(|value| (value - maximum).exp())
        .collect::<Vec<_>>();
    let total: f64 = weights.iter().sum();
    let entropy = weights
        .iter()
        .map(|weight| {
            let probability = weight / total;
            -probability * probability.ln()
        })
        .sum::<f64>();
    entropy / (values.len() as f64).ln()
}

fn parse_artifact_pin(pin: &str) -> Result<(&str, u32), RuntimeError> {
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

fn topological_order(
    edges: &BTreeMap<String, BTreeSet<String>>,
) -> Result<Vec<String>, RuntimeError> {
    let mut indegree: BTreeMap<String, usize> =
        edges.keys().map(|node| (node.clone(), 0)).collect();
    for targets in edges.values() {
        for target in targets {
            let degree = indegree.get_mut(target).ok_or_else(|| {
                RuntimeError::Invalid("seam graph references unknown node".into())
            })?;
            *degree = degree.saturating_add(1);
        }
    }
    let mut ready: BTreeSet<_> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(node, _)| node.clone())
        .collect();
    let mut order = Vec::with_capacity(edges.len());
    while let Some(node) = ready.pop_first() {
        order.push(node.clone());
        for target in edges.get(&node).expect("known node") {
            let degree = indegree.get_mut(target).expect("validated target");
            *degree -= 1;
            if *degree == 0 {
                ready.insert(target.clone());
            }
        }
    }
    if order.len() != edges.len() {
        Err(RuntimeError::Invalid(
            "decomposition seam graph is cyclic".into(),
        ))
    } else {
        Ok(order)
    }
}

fn flow_from_artifact(tenant: &str, artifact: &crate::Artifact) -> Result<FlowPush, RuntimeError> {
    let body = artifact
        .body
        .as_object()
        .ok_or_else(|| RuntimeError::Internal("flow artifact body is not an object".into()))?;
    Ok(FlowPush {
        tenant: tenant.into(),
        flow_id: artifact.id.clone(),
        flow_rev: artifact.version.to_string(),
        program: body
            .get("program")
            .cloned()
            .ok_or_else(|| RuntimeError::Internal("flow artifact body lacks program".into()))?,
        targets: serde_json::from_value(
            body.get("targets")
                .cloned()
                .ok_or_else(|| RuntimeError::Internal("flow artifact body lacks targets".into()))?,
        )
        .map_err(|error| RuntimeError::Internal(error.to_string()))?,
        prompts: serde_json::from_value(
            body.get("prompts")
                .cloned()
                .ok_or_else(|| RuntimeError::Internal("flow artifact body lacks prompts".into()))?,
        )
        .map_err(|error| RuntimeError::Internal(error.to_string()))?,
    })
}

fn build_cases(spec: &BuildSpec) -> Result<Vec<SandboxCase>, RuntimeError> {
    spec.draft
        .examples
        .iter()
        .map(|example| {
            let fixtures = example
                .fixtures
                .iter()
                .cloned()
                .map(serde_json::from_value::<SandboxFixtureCall>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| RuntimeError::Invalid(format!("example fixture: {error}")))?;
            Ok(SandboxCase {
                input: example.inputs.clone(),
                wakes: vec![],
                expect_park: false,
                expected: example.output.clone(),
                fixtures,
            })
        })
        .collect()
}

fn vector_cosine(left: &[f32], right: &[f32]) -> f64 {
    if left.is_empty() || left.len() != right.len() {
        return 0.0;
    }
    let (dot, left_norm, right_norm) = left.iter().zip(right).fold(
        (0.0_f64, 0.0_f64, 0.0_f64),
        |(dot, left_norm, right_norm), (left, right)| {
            let left = f64::from(*left);
            let right = f64::from(*right);
            (
                dot + left * right,
                left_norm + left * left,
                right_norm + right * right,
            )
        },
    );
    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        (dot / (left_norm.sqrt() * right_norm.sqrt())).clamp(-1.0, 1.0)
    }
}

/// Offline/test embedding space. Token hashing (rather than byte-position hashing) preserves the
/// key invariant that unrelated descriptions do not acquire high similarity merely because they
/// have comparable length. The pin is recorded on every measurement and is never mixed with the
/// production semantic space.
fn pinned_lexical_embedding(text: &str, dimension: usize) -> Vec<f32> {
    let mut vector = vec![0.0_f32; dimension.max(1)];
    for token in text
        .to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| token.len() >= 2)
        .take(256)
    {
        let digest = blake3::hash(token.as_bytes());
        let bytes = digest.as_bytes();
        let index = u16::from_le_bytes([bytes[0], bytes[1]]) as usize % vector.len();
        let sign = if bytes[2] & 1 == 0 { 1.0 } else { -1.0 };
        vector[index] += sign;
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

struct SearchNamespace(PathBuf);

impl SearchNamespace {
    fn create(path: PathBuf) -> Result<Self, RuntimeError> {
        if path.exists() {
            std::fs::remove_dir_all(&path).map_err(|error| {
                RuntimeError::Store(format!(
                    "cannot clear stale builder search namespace: {error}"
                ))
            })?;
        }
        std::fs::create_dir_all(&path).map_err(|error| {
            RuntimeError::Store(format!("cannot create builder search namespace: {error}"))
        })?;
        Ok(Self(path))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for SearchNamespace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn normalize_need(description: &str, fallback: &str) -> String {
    let normalized = description
        .to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| token.len() >= 3)
        .take(12)
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() {
        fallback.into()
    } else {
        normalized
    }
}

fn release_lease(runtime: &Runtime, tenant: &str, job: &BuildJob) -> Result<(), RuntimeError> {
    if job.workspace.interface_hash.is_empty() {
        return Ok(());
    }
    match NameLeaseRepository::open(runtime.inner.store.clone())
        .map_err(artifact_error)?
        .release(
            tenant,
            &job.spec.draft.name,
            &job.workspace.interface_hash,
            &job.build_id,
        ) {
        Ok(_) | Err(crate::ArtifactError::NotFound(_)) => Ok(()),
        Err(error) => Err(artifact_error(error)),
    }
}
