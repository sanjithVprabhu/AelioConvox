//! Generic execution of typed ability paths.
//!
//! Resolution is data-driven: a path step may name a registered ability, tool id, or
//! capability tag. Effectful steps always pass through `ToolCallBlock`.

use crate::abilities::express::{self, Utterance};
use crate::abilities::invoke::ToolHost;
use crate::abilities::registry::Registry;
use crate::abilities::sig::SignatureRegistry;
use crate::blocks::tool_call::{
    bind_sources_from_maps, run_tool_call_block, ToolCallContext, ToolCallOutcome,
};
use crate::contract::{AbilityContract, AbilityPath, Predicate, TypeSchema};
use crate::ops::effects::EffectEnv;
use crate::policy::{eval_predicate, PolicyCtx};
use crate::tenant::{PersonalitySpec, PolicySpec, ToolSpec};
use crate::types::{AelioError, AelioResult, ReasonCode, Value};
use indexmap::IndexMap;
use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct ExecutionFrame {
    pub slots: IndexMap<String, Value>,
    pub evidence: IndexMap<String, Value>,
    pub state: Option<String>,
    pub direction: Option<String>,
    pub capabilities: Vec<String>,
    pub returning: bool,
    pub dormant: bool,
    pub reply: Option<Utterance>,
    pub claims: Vec<crate::abilities::judge::EvidenceClaim>,
}

#[derive(Debug)]
pub enum PathExecution {
    Done {
        frame: ExecutionFrame,
        invoked_tools: Vec<String>,
    },
    NeedUser {
        frame: ExecutionFrame,
        missing: Vec<String>,
        question: String,
    },
}

pub struct PathExecutionContext<'a> {
    pub registry: &'a Registry,
    pub policies: &'a [PolicySpec],
    pub policy: PolicyCtx,
    pub personality: Option<&'a PersonalitySpec>,
    pub host: &'a mut dyn ToolHost,
    pub signatures: &'a mut SignatureRegistry,
    pub once_seen: &'a mut HashSet<String>,
    pub effects: &'a mut EffectEnv,
    pub user_id: &'a str,
}

pub fn execute_path(
    path: &AbilityPath,
    mut frame: ExecutionFrame,
    context: &mut PathExecutionContext<'_>,
) -> AelioResult<PathExecution> {
    crate::abilities::learn::typecheck(context.registry, path)?;
    let mut invoked_tools = Vec::new();

    for step in &path.steps {
        for (name, value) in &step.args {
            frame
                .slots
                .insert(name.clone(), crate::ops::pure::json_to_value(value));
        }

        let contract = context.registry.abilities.get(&step.ability_id);
        check_preconditions(contract, &frame, &context.policy)?;

        if let Some(tool) = resolve_tool(context.registry, &step.ability_id, contract)? {
            if !frame.capabilities.is_empty()
                && !tool.capability_tags.iter().any(|tool_capability| {
                    frame.capabilities.iter().any(|allowed| {
                        allowed == tool_capability
                            || (allowed.ends_with('*')
                                && tool_capability.starts_with(
                                    allowed.trim_end_matches('*').trim_end_matches('.'),
                                ))
                    })
                })
            {
                return Err(AelioError::new(
                    ReasonCode::Denied,
                    format!(
                        "tool {} is outside the current state's permission envelope",
                        tool.id
                    ),
                ));
            }
            let sources = bind_sources_from_maps(
                frame.slots.clone(),
                context.policy.env.clone(),
                IndexMap::new(),
                // Earlier steps in a composed path publish only sanitized, declared evidence here.
                // Passing it as ToolOutput is the typed A→B seam for multi-hop procedures.
                frame.evidence.clone(),
            );
            let mut policy = policy_for_frame(&context.policy, &frame);
            policy.capability = tool.capability_tags.first().cloned();
            policy.tool = Some(tool.id.clone());
            match run_tool_call_block(
                tool,
                &sources,
                &mut ToolCallContext {
                    policies: context.policies,
                    policy: &policy,
                    host: context.host,
                    signatures: context.signatures,
                    once_seen: context.once_seen,
                    effects: context.effects,
                    user_id: context.user_id,
                },
            )? {
                ToolCallOutcome::NeedUser { missing, question } => {
                    return Ok(PathExecution::NeedUser {
                        frame,
                        missing,
                        question,
                    });
                }
                ToolCallOutcome::Done { receipt } => {
                    let provenance = format!(
                        "tool:{}:sig={}:args={}",
                        receipt.tool_id, receipt.sig_hash, receipt.args_hash
                    );
                    for (name, value) in receipt.extracted.iter().chain(
                        receipt
                            .safe_response
                            .as_map()
                            .into_iter()
                            .flat_map(indexmap::IndexMap::iter),
                    ) {
                        if value.as_str() == Some("[REDACTED]") {
                            continue;
                        }
                        let id = format!("tool.{}.{}", receipt.tool_id, name);
                        if !frame.claims.iter().any(|claim| claim.id == id) {
                            frame.claims.push(crate::abilities::judge::EvidenceClaim {
                                id,
                                value: value.clone(),
                                provenance: provenance.clone(),
                            });
                        }
                    }
                    invoked_tools.push(receipt.tool_id.clone());
                    merge_map(&mut frame.evidence, receipt.extracted);
                    if let Some(values) = receipt.safe_response.as_map() {
                        merge_map(&mut frame.evidence, values.clone());
                    }
                    frame
                        .evidence
                        .insert(format!("tool.{}", receipt.tool_id), Value::Bool(true));
                }
            }
        } else {
            execute_builtin(&step.ability_id, &mut frame, context.personality)?;
        }

        check_postconditions(contract, &frame, &context.policy)?;
    }

    Ok(PathExecution::Done {
        frame,
        invoked_tools,
    })
}

fn resolve_tool<'a>(
    registry: &'a Registry,
    step_id: &str,
    contract: Option<&AbilityContract>,
) -> AelioResult<Option<&'a ToolSpec>> {
    if let Some(tool) = registry.tools.get(step_id) {
        return Ok(Some(tool));
    }
    let by_capability = registry.lookup_tool_by_capability(step_id);
    if by_capability.len() > 1 {
        return Err(AelioError::new(
            ReasonCode::Ambiguous,
            format!("capability {step_id} resolves to multiple tools"),
        ));
    }
    if let Some(tool) = by_capability.first() {
        return Ok(Some(*tool));
    }
    if let Some(dep) = contract.and_then(|ability| ability.tool_deps.first()) {
        return registry.lookup_tool(dep).map(Some);
    }
    Ok(None)
}

fn execute_builtin(
    id: &str,
    frame: &mut ExecutionFrame,
    personality: Option<&PersonalitySpec>,
) -> AelioResult<()> {
    match id {
        "State.Direction" | "State.Read" => {
            if let Some(direction) = &frame.direction {
                frame
                    .evidence
                    .insert("direction".into(), Value::str(direction));
            }
        }
        "Registry.Capabilities" => {
            frame.evidence.insert(
                "capabilities".into(),
                Value::List(frame.capabilities.iter().map(Value::str).collect()),
            );
        }
        "Express.Template" => {
            frame.reply = Some(express::greeting_template(
                personality,
                frame.direction.as_deref(),
                &capability_labels(&frame.capabilities),
                frame.returning,
                frame.dormant,
            ));
        }
        "Express.Synthesize" => {
            let evidence = if frame.capabilities.is_empty() {
                "Hey! How can I help?".to_string()
            } else {
                format!(
                    "Hey! I can help you {}.",
                    capability_labels(&frame.capabilities).join(" or ")
                )
            };
            frame.reply = Some(express::synthesize(&evidence, personality, &[], None).0);
        }
        "Express.Ask" | "Sense.Env" | "Sense.Session" | "Bind.ResolveAll" | "Invoke.Call"
        | "Judge.Confidence" | "Understand.Extract" | "Sum" | "Learn.ProposePath" => {}
        _ if frame.reply.is_some() => {}
        _ => {
            return Err(AelioError::new(
                ReasonCode::NotFound,
                format!("no executor registered for ability {id}"),
            ));
        }
    }
    Ok(())
}

fn check_preconditions(
    contract: Option<&AbilityContract>,
    frame: &ExecutionFrame,
    base: &PolicyCtx,
) -> AelioResult<()> {
    let Some(contract) = contract else {
        return Ok(());
    };
    if !schema_accepts_value(&contract.input, &Value::Map(frame.slots.clone())) {
        return Err(AelioError::new(
            ReasonCode::TypeViolation,
            format!("{} input schema rejected execution frame", contract.id),
        ));
    }
    let policy = policy_for_frame(base, frame);
    if contract
        .preconditions
        .iter()
        .any(|predicate| !eval_predicate(predicate, &policy))
    {
        return Err(AelioError::new(
            ReasonCode::GateNotMet,
            format!("{} precondition failed", contract.id),
        ));
    }
    Ok(())
}

fn check_postconditions(
    contract: Option<&AbilityContract>,
    frame: &ExecutionFrame,
    base: &PolicyCtx,
) -> AelioResult<()> {
    let Some(contract) = contract else {
        return Ok(());
    };
    if !schema_accepts_value(&contract.output, &Value::Map(frame.evidence.clone())) {
        return Err(AelioError::new(
            ReasonCode::TypeViolation,
            format!("{} output schema rejected execution evidence", contract.id),
        ));
    }
    let policy = policy_for_frame(base, frame);
    if contract
        .postconditions
        .iter()
        .any(|predicate| !eval_predicate(predicate, &policy))
    {
        return Err(AelioError::new(
            ReasonCode::GateNotMet,
            format!("{} postcondition failed", contract.id),
        ));
    }
    Ok(())
}

pub fn predicate_met(predicate: &Predicate, frame: &ExecutionFrame, base: &PolicyCtx) -> bool {
    eval_predicate(predicate, &policy_for_frame(base, frame))
}

fn policy_for_frame(base: &PolicyCtx, frame: &ExecutionFrame) -> PolicyCtx {
    let mut policy = base.clone();
    policy.state = frame.state.clone().or_else(|| policy.state.clone());
    policy.slots = frame.slots.clone();
    policy.evidence = frame.evidence.clone();
    policy
}

fn schema_accepts_value(schema: &TypeSchema, value: &Value) -> bool {
    match schema {
        TypeSchema::Any => true,
        TypeSchema::Scalar { tag } => value.type_tag() == *tag,
        TypeSchema::Optional { inner } => value.is_null() || schema_accepts_value(inner, value),
        TypeSchema::List { items, max_items } => value.as_list().is_some_and(|values| {
            values.len() <= *max_items
                && values
                    .iter()
                    .all(|value| schema_accepts_value(items, value))
        }),
        TypeSchema::Record {
            fields,
            allow_additional,
        } => value.as_map().is_some_and(|values| {
            fields.iter().all(|(name, field)| {
                values
                    .get(name)
                    .map(|value| schema_accepts_value(&field.schema, value))
                    .unwrap_or(!field.required)
            }) && (*allow_additional || values.keys().all(|name| fields.contains_key(name)))
        }),
    }
}

fn merge_map(target: &mut IndexMap<String, Value>, source: IndexMap<String, Value>) {
    target.extend(source);
}

fn capability_labels(capabilities: &[String]) -> Vec<String> {
    capabilities
        .iter()
        .map(|capability| capability.replace('.', " "))
        .collect()
}
