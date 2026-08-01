//! Ephemeral, fixture-only execution through the production Planner and Executor.
//!
//! Each case gets a fresh in-memory tenant and store. No host adapter, production tenant store,
//! network target, clock target, or model provider is reachable. Declared calls must consume an
//! exact ordered fixture, including an optional canonical argument hash.

use crate::{
    build_prompt_composers, json_to_sol, sol_to_json, verify_declared_value, FlowPush,
    RuntimeError, TargetClassSpec,
};
use aelio_kernel::registry::{Invocation, Registry};
use aelio_kernel::{compile, ErrV1, Instance, InstanceConfig, ReasonCode, TurnOutcome};
use aelio_sol::{value_hash, SolValue};
use aelio_store::{EmbeddedStore, MemoryStore};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const MAX_CASES: usize = 256;
const MAX_FIXTURE_CALLS_PER_CASE: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxFixtureCall {
    pub target: String,
    #[serde(default)]
    pub expected_args_hash: Option<String>,
    pub output: serde_json::Value,
    #[serde(default)]
    pub usage_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxCase {
    pub input: serde_json::Value,
    #[serde(default)]
    pub wakes: Vec<serde_json::Value>,
    #[serde(default)]
    pub expect_park: bool,
    pub expected: serde_json::Value,
    #[serde(default)]
    pub fixtures: Vec<SandboxFixtureCall>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxLimits {
    pub max_reactions: usize,
    pub max_wall_ms: u64,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            max_reactions: 10_000,
            max_wall_ms: 120_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxCaseReport {
    pub input_hash: String,
    pub bag_hash: String,
    pub ledger_hash: String,
    pub reactions: usize,
    pub elapsed_ms: u64,
    pub agreed: bool,
    pub parked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxReport {
    pub namespace_hash: String,
    pub cases: Vec<SandboxCaseReport>,
    pub total_reactions: usize,
    pub total_wall_ms: u64,
}

pub struct SandboxRunner;

impl SandboxRunner {
    pub fn run_flow(
        flow: &FlowPush,
        cases: &[SandboxCase],
        limits: SandboxLimits,
    ) -> Result<SandboxReport, RuntimeError> {
        Self::run_flow_inner(flow, cases, limits, None)
    }

    pub(crate) fn run_flow_with_store(
        flow: &FlowPush,
        cases: &[SandboxCase],
        limits: SandboxLimits,
        imprint_store: &EmbeddedStore,
    ) -> Result<SandboxReport, RuntimeError> {
        Self::run_flow_inner(flow, cases, limits, Some(imprint_store))
    }

    fn run_flow_inner(
        flow: &FlowPush,
        cases: &[SandboxCase],
        limits: SandboxLimits,
        imprint_store: Option<&EmbeddedStore>,
    ) -> Result<SandboxReport, RuntimeError> {
        if cases.is_empty() || cases.len() > MAX_CASES {
            return Err(RuntimeError::Invalid(format!(
                "sandbox requires 1..={MAX_CASES} cases"
            )));
        }
        if limits.max_reactions == 0 || limits.max_reactions > 1_000_000 {
            return Err(RuntimeError::Invalid(
                "sandbox max_reactions must be within 1..=1000000".into(),
            ));
        }
        if limits.max_wall_ms == 0 || limits.max_wall_ms > 86_400_000 {
            return Err(RuntimeError::Invalid(
                "sandbox max_wall_ms must be within 1..=86400000".into(),
            ));
        }
        let program_text = serde_json::to_string(&flow.program)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        let program = compile(&program_text)?;
        let flow_hash = value_hash(&json_to_sol(&serde_json::json!({
            "flow_id": flow.flow_id,
            "flow_rev": flow.flow_rev,
            "program": flow.program,
            "targets": flow.targets,
            "prompts": flow.prompts,
        }))?);
        let started = Instant::now();
        let mut reports = Vec::with_capacity(cases.len());
        let mut total_reactions = 0usize;

        for (index, case) in cases.iter().enumerate() {
            if case.fixtures.len() > MAX_FIXTURE_CALLS_PER_CASE {
                return Err(RuntimeError::Invalid(format!(
                    "sandbox case {index} exceeds fixture-call bound"
                )));
            }
            if case.wakes.len() > 32 {
                return Err(RuntimeError::Invalid(format!(
                    "sandbox case {index} exceeds the 32-wake bound"
                )));
            }
            let case_started = Instant::now();
            let tenant = format!("sandbox.{}.{}", &flow_hash[..16], index);
            let (mut registry, queues) =
                fixture_registry(flow, &tenant, &case.fixtures, imprint_store)?;
            aelio_kernel::plan::plan_with_registry(&program, &registry, &tenant)?;
            let input = json_to_sol(&case.input)?;
            let input_hash = value_hash(&input);
            let mut instance = Instance::with_store(
                program.clone(),
                &mut registry,
                Box::new(MemoryStore::new()),
                InstanceConfig {
                    tenant,
                    instance_id: format!("case-{index}"),
                    flow_id: flow.flow_id.clone(),
                    flow_rev: flow.flow_rev.clone(),
                    event_key_secret: [0; 32],
                },
            )?;
            let mut wakes = case.wakes.iter();
            let mut outcome = instance.start(input)?;
            let (bag, bag_hash, parked) = loop {
                match outcome {
                    TurnOutcome::Completed { bag, bag_hash } => {
                        if case.expect_park {
                            return Err(RuntimeError::Invalid(format!(
                                "sandbox case {index} completed but expected to park"
                            )));
                        }
                        if wakes.next().is_some() {
                            return Err(RuntimeError::Invalid(format!(
                                "sandbox case {index} completed before consuming all wakes"
                            )));
                        }
                        break (bag, bag_hash, false);
                    }
                    TurnOutcome::Parked(parked) => {
                        if let Some(wake) = wakes.next() {
                            outcome = instance.resume_stored(json_to_sol(wake)?)?;
                        } else if case.expect_park {
                            let bag_hash = value_hash(&parked.bag);
                            break (parked.bag, bag_hash, true);
                        } else {
                            return Err(RuntimeError::Invalid(format!(
                                "sandbox case {index} parked without a declared wake"
                            )));
                        }
                    }
                }
            };
            ensure_fixtures_consumed(index, &queues)?;
            let actual = sol_to_json(&bag)?;
            let agreed = json_contains(&actual, &case.expected);
            let reactions = instance.ledger().entries().len();
            total_reactions = total_reactions
                .checked_add(reactions)
                .ok_or_else(|| RuntimeError::Invalid("sandbox reaction count overflow".into()))?;
            if total_reactions > limits.max_reactions {
                return Err(RuntimeError::Kernel {
                    code: ReasonCode::BudgetCalls.code().into(),
                    detail: "sandbox reaction budget exceeded".into(),
                });
            }
            let elapsed_ms = u64::try_from(case_started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let total_wall_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            if total_wall_ms > limits.max_wall_ms {
                return Err(RuntimeError::Kernel {
                    code: ReasonCode::BudgetMs.code().into(),
                    detail: "sandbox wall budget exceeded".into(),
                });
            }
            let ledger = SolValue::list(
                instance
                    .ledger()
                    .entries()
                    .iter()
                    .map(aelio_kernel::ledger::Entry::to_sol),
            );
            reports.push(SandboxCaseReport {
                input_hash,
                bag_hash,
                ledger_hash: value_hash(&ledger),
                reactions,
                elapsed_ms,
                agreed,
                parked,
            });
        }
        let total_wall_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        Ok(SandboxReport {
            namespace_hash: flow_hash,
            cases: reports,
            total_reactions,
            total_wall_ms,
        })
    }
}

pub(crate) struct HarnessSandboxRunner {
    store: EmbeddedStore,
}

enum NestedHarnessOutcome {
    Completed {
        value: serde_json::Value,
        reactions: usize,
    },
    Parked {
        value: serde_json::Value,
        reactions: usize,
    },
}

impl HarnessSandboxRunner {
    pub(crate) fn new(store: EmbeddedStore) -> Self {
        Self { store }
    }

    pub(crate) fn run(
        &self,
        tenant: &str,
        harness: &crate::HarnessBody,
        cases: &[SandboxCase],
        limits: SandboxLimits,
    ) -> Result<SandboxReport, RuntimeError> {
        harness.validate_shape().map_err(crate::artifact_error)?;
        if cases.is_empty() || cases.len() > MAX_CASES {
            return Err(RuntimeError::Invalid(format!(
                "harness sandbox requires 1..={MAX_CASES} cases"
            )));
        }
        let started = Instant::now();
        let mut reports = Vec::with_capacity(cases.len());
        let mut total_reactions = 0usize;
        for (index, case) in cases.iter().enumerate() {
            if case.wakes.len() > 32 {
                return Err(RuntimeError::Invalid(format!(
                    "harness sandbox case {index} exceeds the 32-wake bound"
                )));
            }
            let case_started = Instant::now();
            let mut fixtures = SandboxFixturePool::new(&case.fixtures)?;
            let mut ledger_hashes = Vec::new();
            let mut wakes = case.wakes.iter();
            let outcome = self.execute_harness(
                tenant,
                harness,
                &case.input,
                &mut fixtures,
                &mut ledger_hashes,
                &mut wakes,
                1,
                index,
            )?;
            let (actual, reactions, parked) = match outcome {
                NestedHarnessOutcome::Completed { value, reactions } => {
                    if case.expect_park {
                        return Err(RuntimeError::Invalid(format!(
                            "harness sandbox case {index} completed but expected to park"
                        )));
                    }
                    if wakes.next().is_some() {
                        return Err(RuntimeError::Invalid(format!(
                            "harness sandbox case {index} completed before consuming all wakes"
                        )));
                    }
                    (value, reactions, false)
                }
                NestedHarnessOutcome::Parked { value, reactions } => {
                    if !case.expect_park {
                        return Err(RuntimeError::Invalid(format!(
                            "harness sandbox case {index} parked without a declared wake"
                        )));
                    }
                    (value, reactions, true)
                }
            };
            fixtures.ensure_consumed(index)?;
            total_reactions = total_reactions
                .checked_add(reactions)
                .ok_or_else(|| RuntimeError::Invalid("reaction count overflow".into()))?;
            if total_reactions > limits.max_reactions {
                return Err(RuntimeError::Kernel {
                    code: ReasonCode::BudgetCalls.code().into(),
                    detail: "harness sandbox reaction budget exceeded".into(),
                });
            }
            let total_wall_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            if total_wall_ms > limits.max_wall_ms {
                return Err(RuntimeError::Kernel {
                    code: ReasonCode::BudgetMs.code().into(),
                    detail: "harness sandbox wall budget exceeded".into(),
                });
            }
            let actual_sol = json_to_sol(&actual)?;
            reports.push(SandboxCaseReport {
                input_hash: value_hash(&json_to_sol(&case.input)?),
                bag_hash: value_hash(&actual_sol),
                ledger_hash: value_hash(&SolValue::list(
                    ledger_hashes.into_iter().map(SolValue::str),
                )),
                reactions,
                elapsed_ms: u64::try_from(case_started.elapsed().as_millis()).unwrap_or(u64::MAX),
                agreed: json_contains(&actual, &case.expected),
                parked,
            });
        }
        let identity = serde_json::json!({"tenant":tenant,"harness":harness});
        Ok(SandboxReport {
            namespace_hash: value_hash(&json_to_sol(&identity)?),
            cases: reports,
            total_reactions,
            total_wall_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_harness(
        &self,
        tenant: &str,
        harness: &crate::HarnessBody,
        input: &serde_json::Value,
        fixtures: &mut SandboxFixturePool,
        ledger_hashes: &mut Vec<String>,
        wakes: &mut std::slice::Iter<'_, serde_json::Value>,
        depth: usize,
        case_index: usize,
    ) -> Result<NestedHarnessOutcome, RuntimeError> {
        if depth > 32 {
            return Err(RuntimeError::Invalid(
                "nested harness exceeds depth 32".into(),
            ));
        }
        let parent = input.as_object().ok_or_else(|| {
            RuntimeError::Invalid("harness input must be an object keyed by slots".into())
        })?;
        let mut outputs: HashMap<String, serde_json::Value> = HashMap::new();
        let mut reactions = 0usize;
        for node in &harness.nodes {
            let mut args = serde_json::Map::new();
            for seam in harness.seams.iter().filter(|seam| {
                matches!(&seam.to, crate::HarnessSink::NodeInput { node: sink, .. } if sink == &node.nid)
            }) {
                let crate::HarnessSink::NodeInput { slot, .. } = &seam.to else {
                    unreachable!()
                };
                let value = source_value(&seam.from, parent, &outputs)?;
                args.insert(slot.clone(), self.apply_converter(tenant, seam, value)?);
            }
            let (id, version) = parse_artifact_pin(&node.artifact)?;
            let repository = crate::ArtifactRepository::open(self.store.clone())
                .map_err(crate::artifact_error)?;
            let artifact = repository
                .get(tenant, id, version)
                .map_err(crate::artifact_error)?
                .ok_or_else(|| RuntimeError::NotFound(node.artifact.clone()))?;
            if !matches!(
                artifact.status,
                crate::ArtifactStatus::Canary | crate::ArtifactStatus::Promoted
            ) {
                return Err(RuntimeError::Conflict(format!(
                    "nested artifact `{}` is not executable",
                    node.artifact
                )));
            }
            let args = serde_json::Value::Object(args);
            validate_artifact_input(&self.store, tenant, &artifact.artifact, &args)?;
            let outcome = match artifact.artifact.class {
                crate::ArtifactClass::Flow => {
                    let flow = flow_from_artifact(tenant, &artifact.artifact)?;
                    self.execute_flow(
                        &flow,
                        &args,
                        fixtures,
                        ledger_hashes,
                        wakes,
                        case_index,
                        &node.nid,
                    )?
                }
                crate::ArtifactClass::Harness => {
                    let nested: crate::HarnessBody = serde_json::from_value(
                        artifact
                            .artifact
                            .body
                            .get("harness")
                            .cloned()
                            .ok_or_else(|| {
                                RuntimeError::Internal("harness artifact body is corrupt".into())
                            })?,
                    )
                    .map_err(|error| RuntimeError::Internal(error.to_string()))?;
                    self.execute_harness(
                        tenant,
                        &nested,
                        &args,
                        fixtures,
                        ledger_hashes,
                        wakes,
                        depth + 1,
                        case_index,
                    )?
                }
                other => {
                    return Err(RuntimeError::Invalid(format!(
                        "harness node `{}` references non-executable class {other:?}",
                        node.nid
                    )))
                }
            };
            let (output, used) = match outcome {
                NestedHarnessOutcome::Completed { value, reactions } => (value, reactions),
                NestedHarnessOutcome::Parked {
                    value,
                    reactions: used,
                } => {
                    return Ok(NestedHarnessOutcome::Parked {
                        value,
                        reactions: reactions.saturating_add(used).saturating_add(1),
                    });
                }
            };
            crate::ImprintRegistry::open(self.store.clone())
                .map_err(crate::artifact_error)?
                .validate_value(tenant, &artifact.artifact.interface.output, &output)
                .map_err(crate::artifact_error)?;
            outputs.insert(node.nid.clone(), output);
            reactions = reactions.saturating_add(used).saturating_add(1);
        }
        let seam = harness
            .seams
            .iter()
            .find(|seam| matches!(seam.to, crate::HarnessSink::ParentOutput))
            .ok_or_else(|| RuntimeError::Internal("harness lacks parent output seam".into()))?;
        let output = source_value(&seam.from, parent, &outputs)?;
        Ok(NestedHarnessOutcome::Completed {
            value: self.apply_converter(tenant, seam, output)?,
            reactions,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_flow(
        &self,
        flow: &FlowPush,
        input: &serde_json::Value,
        fixtures: &mut SandboxFixturePool,
        ledger_hashes: &mut Vec<String>,
        wakes: &mut std::slice::Iter<'_, serde_json::Value>,
        case_index: usize,
        node: &str,
    ) -> Result<NestedHarnessOutcome, RuntimeError> {
        let program_text = serde_json::to_string(&flow.program)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        let program = compile(&program_text)?;
        let tenant = format!("sandbox.harness.{case_index}.{node}");
        let mut registry = fixtures.registry(flow, &tenant, Some(&self.store))?;
        aelio_kernel::plan::plan_with_registry(&program, &registry, &tenant)?;
        let mut instance = Instance::with_store(
            program,
            &mut registry,
            Box::new(MemoryStore::new()),
            InstanceConfig {
                tenant,
                instance_id: format!("case-{case_index}-{node}"),
                flow_id: flow.flow_id.clone(),
                flow_rev: flow.flow_rev.clone(),
                event_key_secret: [0; 32],
            },
        )?;
        let mut outcome = instance.start(json_to_sol(input)?)?;
        let (bag, parked) = loop {
            match outcome {
                TurnOutcome::Completed { bag, .. } => break (bag, false),
                TurnOutcome::Parked(suspension) => {
                    if let Some(wake) = wakes.next() {
                        outcome = instance.resume_stored(json_to_sol(wake)?)?;
                    } else {
                        break (suspension.bag, true);
                    }
                }
            }
        };
        let ledger = SolValue::list(
            instance
                .ledger()
                .entries()
                .iter()
                .map(aelio_kernel::ledger::Entry::to_sol),
        );
        ledger_hashes.push(value_hash(&ledger));
        let value = sol_to_json(&bag)?;
        let reactions = instance.ledger().entries().len();
        Ok(if parked {
            NestedHarnessOutcome::Parked { value, reactions }
        } else {
            NestedHarnessOutcome::Completed { value, reactions }
        })
    }

    fn apply_converter(
        &self,
        tenant: &str,
        seam: &crate::HarnessSeam,
        value: serde_json::Value,
    ) -> Result<serde_json::Value, RuntimeError> {
        let registry =
            crate::ImprintRegistry::open(self.store.clone()).map_err(crate::artifact_error)?;
        registry
            .validate_value(tenant, &seam.from_imprint, &value)
            .map_err(crate::artifact_error)?;
        let converted = if let Some(pin) = &seam.converter {
            let (id, version) = parse_artifact_pin(pin)?;
            let artifact = crate::ArtifactRepository::open(self.store.clone())
                .map_err(crate::artifact_error)?
                .get(tenant, id, version)
                .map_err(crate::artifact_error)?
                .or_else(|| {
                    crate::ArtifactRepository::open(self.store.clone())
                        .ok()?
                        .get(crate::mint::VENDOR_ARTIFACT_TENANT, id, version)
                        .ok()?
                })
                .ok_or_else(|| RuntimeError::NotFound(pin.clone()))?;
            if artifact.artifact.class != crate::ArtifactClass::Glu
                || !matches!(
                    artifact.status,
                    crate::ArtifactStatus::Canary | crate::ArtifactStatus::Promoted
                )
            {
                return Err(RuntimeError::Conflict(format!(
                    "converter `{pin}` is not executable"
                )));
            }
            let rules = aelio_convert::parse_rules(
                artifact
                    .artifact
                    .body
                    .get("rules")
                    .ok_or_else(|| RuntimeError::Internal("Glu body lacks rules".into()))?,
            )
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
            let output =
                aelio_convert::apply_rules(&rules, &json_to_sol(&value)?).map_err(|error| {
                    RuntimeError::Invalid(format!(
                        "converter rule {} failed ({}): {}",
                        error.rule_index,
                        error.code.code(),
                        error.detail
                    ))
                })?;
            sol_to_json(&output)?
        } else {
            value
        };
        registry
            .validate_value(tenant, &seam.to_imprint, &converted)
            .map_err(crate::artifact_error)?;
        Ok(converted)
    }
}

type FixtureQueues = HashMap<String, Arc<Mutex<VecDeque<SandboxFixtureCall>>>>;

pub(crate) struct SandboxFixturePool {
    queues: FixtureQueues,
}

impl SandboxFixturePool {
    pub(crate) fn new(fixtures: &[SandboxFixtureCall]) -> Result<Self, RuntimeError> {
        let mut queues: FixtureQueues = HashMap::new();
        for fixture in fixtures {
            validate_fixture(fixture)?;
            queues
                .entry(fixture.target.clone())
                .or_insert_with(|| Arc::new(Mutex::new(VecDeque::new())))
                .lock()
                .map_err(|error| RuntimeError::Internal(error.to_string()))?
                .push_back(fixture.clone());
        }
        Ok(Self { queues })
    }

    pub(crate) fn registry(
        &mut self,
        flow: &FlowPush,
        tenant: &str,
        imprint_store: Option<&EmbeddedStore>,
    ) -> Result<Registry, RuntimeError> {
        fixture_registry_from_queues(flow, tenant, &mut self.queues, imprint_store)
    }

    pub(crate) fn ensure_consumed(&self, case_index: usize) -> Result<(), RuntimeError> {
        ensure_fixtures_consumed(case_index, &self.queues)
    }
}

fn fixture_registry(
    flow: &FlowPush,
    tenant: &str,
    fixtures: &[SandboxFixtureCall],
    imprint_store: Option<&EmbeddedStore>,
) -> Result<(Registry, FixtureQueues), RuntimeError> {
    let mut pool = SandboxFixturePool::new(fixtures)?;
    let registry = pool.registry(flow, tenant, imprint_store)?;
    let queues = pool.queues;
    if let Some(unknown) = queues
        .keys()
        .find(|target| !flow.targets.iter().any(|declared| &declared.id == *target))
    {
        return Err(RuntimeError::Invalid(format!(
            "fixture names undeclared target `{unknown}`"
        )));
    }
    Ok((registry, queues))
}

fn validate_fixture(fixture: &SandboxFixtureCall) -> Result<(), RuntimeError> {
    if fixture.target.is_empty() {
        return Err(RuntimeError::Invalid(
            "fixture target must not be empty".into(),
        ));
    }
    if let Some(hash) = &fixture.expected_args_hash {
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(RuntimeError::Invalid(
                "fixture expected_args_hash must be a BLAKE3 hex digest".into(),
            ));
        }
    }
    Ok(())
}

fn fixture_registry_from_queues(
    flow: &FlowPush,
    tenant: &str,
    queues: &mut FixtureQueues,
    imprint_store: Option<&EmbeddedStore>,
) -> Result<Registry, RuntimeError> {
    let composers = build_prompt_composers(flow)?;
    let mut registry = Registry::default();
    for target in &flow.targets {
        let declaration = target.declaration(tenant);
        let target_id = target.id.clone();
        let input_imprint = target.input_imprint.clone();
        let output_imprint = target.output_imprint.clone();
        let boundary_store = imprint_store.cloned();
        let boundary_tenant = flow.tenant.clone();
        if let Some(composer) = composers.get(&target.id) {
            let composer = Arc::clone(composer);
            registry
                .register_contextual_declared(declaration, move |args, _context| {
                    verify_declared_value(
                        boundary_store.as_ref(),
                        &boundary_tenant,
                        args,
                        &input_imprint,
                        &target_id,
                        "input",
                    )?;
                    let composed = composer
                        .registry
                        .compose(&composer.template, args)
                        .map_err(|detail| ErrV1::new(ReasonCode::Shape, &target_id, detail))?;
                    let output = SolValue::map([
                        ("prompt", SolValue::str(composed.text)),
                        ("prompt_hash", SolValue::str(composed.prompt_hash)),
                        ("template", SolValue::str(composed.template)),
                    ]);
                    verify_declared_value(
                        boundary_store.as_ref(),
                        &boundary_tenant,
                        &output,
                        &output_imprint,
                        &target_id,
                        "output",
                    )?;
                    Ok(output)
                })
                .map_err(RuntimeError::Invalid)?;
            continue;
        }
        let queue = queues.entry(target.id.clone()).or_default().clone();
        if matches!(target.class, TargetClassSpec::Model) {
            let boundary_store = imprint_store.cloned();
            let boundary_tenant = flow.tenant.clone();
            registry
                .register_contextual_model_declared(declaration, move |args, _context| {
                    verify_declared_value(
                        boundary_store.as_ref(),
                        &boundary_tenant,
                        args,
                        &input_imprint,
                        &target_id,
                        "input",
                    )?;
                    let fixture = consume_fixture(&queue, &target_id, args)?;
                    let output = aelio_kernel::json_from(&fixture.output).map_err(|error| {
                        ErrV1::new(ReasonCode::Shape, &target_id, error.to_string())
                    })?;
                    verify_declared_value(
                        boundary_store.as_ref(),
                        &boundary_tenant,
                        &output,
                        &output_imprint,
                        &target_id,
                        "output",
                    )?;
                    Ok(Invocation {
                        output,
                        usage_tokens: fixture.usage_tokens,
                    })
                })
                .map_err(RuntimeError::Invalid)?;
        } else {
            let boundary_store = imprint_store.cloned();
            let boundary_tenant = flow.tenant.clone();
            registry
                .register_contextual_declared(declaration, move |args, _context| {
                    verify_declared_value(
                        boundary_store.as_ref(),
                        &boundary_tenant,
                        args,
                        &input_imprint,
                        &target_id,
                        "input",
                    )?;
                    let fixture = consume_fixture(&queue, &target_id, args)?;
                    let output = aelio_kernel::json_from(&fixture.output).map_err(|error| {
                        ErrV1::new(ReasonCode::Shape, &target_id, error.to_string())
                    })?;
                    verify_declared_value(
                        boundary_store.as_ref(),
                        &boundary_tenant,
                        &output,
                        &output_imprint,
                        &target_id,
                        "output",
                    )?;
                    Ok(output)
                })
                .map_err(RuntimeError::Invalid)?;
        }
    }
    Ok(registry)
}

fn consume_fixture(
    queue: &Arc<Mutex<VecDeque<SandboxFixtureCall>>>,
    target: &str,
    args: &SolValue,
) -> Result<SandboxFixtureCall, ErrV1> {
    let mut queue = queue
        .lock()
        .map_err(|error| ErrV1::new(ReasonCode::Internal, target, error.to_string()))?;
    let fixture = queue.pop_front().ok_or_else(|| {
        ErrV1::new(
            ReasonCode::ToolPermanent,
            target,
            "sandbox call has no declared fixture",
        )
    })?;
    if let Some(expected) = &fixture.expected_args_hash {
        let actual = value_hash(args);
        if actual != *expected {
            return Err(ErrV1::new(
                ReasonCode::GuardViolation,
                target,
                format!("fixture argument hash mismatch: expected {expected}, got {actual}"),
            ));
        }
    }
    Ok(fixture)
}

fn ensure_fixtures_consumed(case_index: usize, queues: &FixtureQueues) -> Result<(), RuntimeError> {
    for (target, queue) in queues {
        let remaining = queue
            .lock()
            .map_err(|error| RuntimeError::Internal(error.to_string()))?
            .len();
        if remaining != 0 {
            return Err(RuntimeError::Invalid(format!(
                "sandbox case {case_index} did not consume {remaining} fixture(s) for `{target}`"
            )));
        }
    }
    Ok(())
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

fn source_value(
    source: &crate::HarnessSource,
    parent: &serde_json::Map<String, serde_json::Value>,
    outputs: &HashMap<String, serde_json::Value>,
) -> Result<serde_json::Value, RuntimeError> {
    match source {
        crate::HarnessSource::ParentInput { slot } => parent
            .get(slot)
            .cloned()
            .ok_or_else(|| RuntimeError::Invalid(format!("missing parent input `{slot}`"))),
        crate::HarnessSource::NodeOutput { node } => outputs
            .get(node)
            .cloned()
            .ok_or_else(|| RuntimeError::Internal(format!("node `{node}` has no output"))),
    }
}

fn validate_artifact_input(
    store: &EmbeddedStore,
    tenant: &str,
    artifact: &crate::Artifact,
    args: &serde_json::Value,
) -> Result<(), RuntimeError> {
    let map = args
        .as_object()
        .ok_or_else(|| RuntimeError::Invalid("artifact args must be an object".into()))?;
    let registry = crate::ImprintRegistry::open(store.clone()).map_err(crate::artifact_error)?;
    for input in &artifact.interface.inputs {
        match map.get(&input.name) {
            Some(value) => registry
                .validate_value(tenant, &input.imprint, value)
                .map_err(crate::artifact_error)?,
            None if input.required => {
                return Err(RuntimeError::Invalid(format!(
                    "artifact args lack required input `{}`",
                    input.name
                )))
            }
            None => {}
        }
    }
    if let Some(unknown) = map.keys().find(|name| {
        !artifact
            .interface
            .inputs
            .iter()
            .any(|input| input.name == **name)
    }) {
        return Err(RuntimeError::Invalid(format!(
            "artifact args contain undeclared input `{unknown}`"
        )));
    }
    Ok(())
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
            .ok_or_else(|| RuntimeError::Internal("flow body lacks program".into()))?,
        targets: serde_json::from_value(
            body.get("targets")
                .cloned()
                .ok_or_else(|| RuntimeError::Internal("flow body lacks targets".into()))?,
        )
        .map_err(|error| RuntimeError::Internal(error.to_string()))?,
        prompts: serde_json::from_value(
            body.get("prompts")
                .cloned()
                .ok_or_else(|| RuntimeError::Internal("flow body lacks prompts".into()))?,
        )
        .map_err(|error| RuntimeError::Internal(error.to_string()))?,
    })
}

fn json_contains(actual: &serde_json::Value, expected: &serde_json::Value) -> bool {
    match (actual, expected) {
        (serde_json::Value::Object(actual), serde_json::Value::Object(expected)) => {
            expected.iter().all(|(key, value)| {
                actual
                    .get(key)
                    .is_some_and(|actual| json_contains(actual, value))
            })
        }
        (serde_json::Value::Array(actual), serde_json::Value::Array(expected)) => {
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| json_contains(actual, expected))
        }
        _ => actual == expected,
    }
}
