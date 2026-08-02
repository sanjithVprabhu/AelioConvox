//! Deterministic catalog Flow lowering.
//!
//! The semantic `FlowSpec` remains the WHAT layer. This module consumes its optional closed
//! lowering declaration, resolves symbolic capability bindings to the one exact SDK tool version,
//! rewrites a Sol program, and admits the resulting immutable runtime Flow. It never calls a model
//! and never accepts raw target ids from the catalog program.

use std::collections::BTreeMap;

use aelio_agent::adaptive::ArtifactPinV1;
use aelio_agent::tenant::{FlowEscape, FlowLoweringV1, FlowSpec, TenantDecl, ToolEffect, ToolSpec};
use aelio_agent::{AelioError, ReasonCode};
use aelio_runtime::{
    ArtifactStatus, BoundSpec, EffectSpec, FlowPush, OriginSpec, Runtime, SandboxCase,
    SandboxFixtureCall, SandboxLimits, TargetClassSpec, TargetSpec,
};

pub(crate) fn materialize_declared_flows(
    runtime: &Runtime,
    catalog: &mut TenantDecl,
    deployer: &str,
) -> Result<(), AelioError> {
    let flows = catalog.flows.clone();
    for flow in &flows {
        if catalog.flow_artifacts.contains_key(&flow.id) {
            continue;
        }
        let Some(lowering) = flow.lowering.as_ref() else {
            continue;
        };
        let (push, cases) = compile_flow(&catalog.tenant_id, &catalog.tools, flow, lowering)?;
        runtime.push_flow(push.clone()).map_err(runtime_error)?;
        let version = push.flow_rev.parse::<u32>().map_err(|_| {
            AelioError::new(ReasonCode::Validation, "compiled flow version is invalid")
        })?;
        let existing = runtime
            .artifact_repository()
            .map_err(runtime_error)?
            .get(&catalog.tenant_id, &push.flow_id, version)
            .map_err(|error| AelioError::new(ReasonCode::Unavailable, error.to_string()))?
            .ok_or_else(|| {
                AelioError::new(
                    ReasonCode::Internal,
                    "compiled flow disappeared after immutable admission",
                )
            })?;
        if matches!(
            existing.status,
            ArtifactStatus::Canary | ArtifactStatus::Promoted
        ) {
            catalog.flow_artifacts.insert(
                flow.id.clone(),
                ArtifactPinV1 {
                    id: existing.artifact.id,
                    version: existing.artifact.version,
                    hash: existing.artifact.hash,
                },
            );
            continue;
        }
        let gate = runtime
            .gate_flow(
                &push,
                &cases,
                SandboxLimits::default(),
                Some(deployer.to_owned()),
            )
            .map_err(runtime_error)?;
        if !matches!(
            gate.record.status,
            ArtifactStatus::Canary | ArtifactStatus::Promoted
        ) {
            return Err(AelioError::new(
                ReasonCode::Denied,
                format!(
                    "flow {} lowering did not produce enough distinct passing evidence for admission",
                    flow.id
                ),
            ));
        }
        catalog.flow_artifacts.insert(
            flow.id.clone(),
            ArtifactPinV1 {
                id: gate.record.artifact.id,
                version: gate.record.artifact.version,
                hash: gate.record.artifact.hash,
            },
        );
    }
    Ok(())
}

fn compile_flow(
    tenant: &str,
    tools: &[ToolSpec],
    flow: &FlowSpec,
    lowering: &FlowLoweringV1,
) -> Result<(FlowPush, Vec<SandboxCase>), AelioError> {
    let mut resolved = BTreeMap::new();
    let mut targets = BTreeMap::<String, TargetSpec>::new();
    for binding in &lowering.bindings {
        let matches = tools
            .iter()
            .filter(|tool| tool.capability_tags.contains(&binding.capability))
            .collect::<Vec<_>>();
        let [tool] = matches.as_slice() else {
            return Err(AelioError::new(
                if matches.is_empty() {
                    ReasonCode::NotFound
                } else {
                    ReasonCode::Ambiguous
                },
                format!(
                    "flow {} capability {} must resolve to exactly one tool",
                    flow.id, binding.capability
                ),
            ));
        };
        let version = tool.version.parse::<u32>().map_err(|_| {
            AelioError::new(
                ReasonCode::Validation,
                format!("tool {} version must be a positive integer", tool.id),
            )
        })?;
        if version == 0 || tool.version.starts_with('0') {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!("tool {} version must be canonical and positive", tool.id),
            ));
        }
        let target_id = format!("{}@{}", tool.id, version);
        let effect = match tool.effect_class() {
            ToolEffect::Pure => EffectSpec::Pure,
            ToolEffect::Read => EffectSpec::Read,
            ToolEffect::Write => EffectSpec::Write,
            ToolEffect::External => EffectSpec::External,
        };
        let candidate = TargetSpec {
            id: target_id.clone(),
            class: TargetClassSpec::Tool,
            effect,
            input_imprint: "aelio.turn.input@1".into(),
            output_imprint: "aelio.turn.output@1".into(),
            bounded: BoundSpec::Deadline {
                max_ms: binding.deadline_ms,
            },
            policy_tags: tool.capability_tags.clone(),
            origin: OriginSpec::Tenant,
        };
        if let Some(existing) = targets.get(&target_id) {
            let existing_deadline = match existing.bounded {
                BoundSpec::Deadline { max_ms } => max_ms,
                _ => 0,
            };
            if existing_deadline != binding.deadline_ms {
                return Err(AelioError::new(
                    ReasonCode::Conflict,
                    format!(
                        "flow {} binds target {target_id} with conflicting deadlines",
                        flow.id
                    ),
                ));
            }
        } else {
            targets.insert(target_id.clone(), candidate);
        }
        resolved.insert(binding.name.clone(), target_id);
    }

    let mut program = lowering.program.clone();
    rewrite_calls(&mut program, &resolved, &flow.id)?;
    let parks = count_parks(&program, false, &flow.id)?;
    if parks > u64::from(flow.max_attempts) {
        return Err(AelioError::new(
            ReasonCode::BudgetExceeded,
            format!(
                "flow {} lowering can park {parks} times but max_attempts is {}",
                flow.id, flow.max_attempts
            ),
        ));
    }
    if let Some(ttl_secs) = flow.ttl_secs {
        let ttl_ms = ttl_secs.checked_mul(1_000).ok_or_else(|| {
            AelioError::new(ReasonCode::BudgetExceeded, "flow TTL milliseconds overflow")
        })?;
        program = serde_json::json!({
            "nid":format!("__aelio_catalog_ttl_{ttl_ms}"),
            "op":"Timeout",
            "ms":ttl_ms,
            "body":program
        });
    }
    program = apply_escape(program, flow)?;
    let cases = lowering
        .cases
        .iter()
        .map(|case| {
            let fixtures = case
                .fixtures
                .iter()
                .map(|fixture| {
                    let target = resolved.get(&fixture.binding).ok_or_else(|| {
                        AelioError::new(
                            ReasonCode::Validation,
                            format!(
                                "flow {} fixture references unknown binding {}",
                                flow.id, fixture.binding
                            ),
                        )
                    })?;
                    Ok(SandboxFixtureCall {
                        target: target.clone(),
                        expected_args_hash: None,
                        output: fixture.output.clone(),
                        usage_tokens: fixture.usage_tokens,
                    })
                })
                .collect::<Result<Vec<_>, AelioError>>()?;
            Ok(SandboxCase {
                input: case.input.clone(),
                wakes: case.wakes.clone(),
                expect_park: case.expect_park,
                expected: case.expected.clone(),
                fixtures,
            })
        })
        .collect::<Result<Vec<_>, AelioError>>()?;
    Ok((
        FlowPush {
            tenant: tenant.into(),
            flow_id: format!("aelio.catalog.{}", flow.id),
            flow_rev: flow.version.clone(),
            program,
            targets: targets.into_values().collect(),
            prompts: vec![],
        },
        cases,
    ))
}

/// Format 1 owns a deterministic fail-closed terminal for every runtime error. `fallback` is
/// intentionally refused: starting a second suspendable subject artifact requires a runtime-level
/// continuation handoff, and returning a magic object to the adaptive layer would create another
/// instruction interpreter outside the reactor.
fn apply_escape(
    program: serde_json::Value,
    flow: &FlowSpec,
) -> Result<serde_json::Value, AelioError> {
    let text = match &flow.escape {
        FlowEscape::Escalate => {
            "I could not safely complete this flow. Human escalation is required."
        }
        FlowEscape::FreeRange => "I could not safely continue this flow.",
        FlowEscape::Fallback { flow_id } => {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!(
                    "flow {} uses fallback escape to {flow_id}, which FlowLoweringV1 cannot safely hand off; use escalate/free_range or a newer lowering format",
                    flow.id
                ),
            ));
        }
    };
    let prefixes = [
        "Shape", "Type", "Missing", "Budget", "Timeout", "Policy", "Guard", "Tool", "Model",
        "Convert", "Internal",
    ];
    let catch = prefixes
        .into_iter()
        .enumerate()
        .map(|(index, prefix)| {
            (
                prefix.to_owned(),
                serde_json::json!({
                    "nid": format!("__aelio_escape_{index}"),
                    "op": "Const",
                    "v": {"text": text}
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    Ok(serde_json::json!({
        "nid": "__aelio_declared_escape",
        "op": "Try",
        "body": program,
        "catch": catch,
        "err_into": "__aelio_escape_error"
    }))
}

fn rewrite_calls(
    value: &mut serde_json::Value,
    resolved: &BTreeMap<String, String>,
    flow_id: &str,
) -> Result<(), AelioError> {
    match value {
        serde_json::Value::Object(object) => {
            if object.get("op").and_then(serde_json::Value::as_str) == Some("Call") {
                let id = object
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        AelioError::new(
                            ReasonCode::Validation,
                            format!("flow {flow_id} Call requires a symbolic literal id"),
                        )
                    })?
                    .to_owned();
                let name = id.strip_prefix("$cap:").ok_or_else(|| {
                    AelioError::new(
                        ReasonCode::Denied,
                        format!("flow {flow_id} lowering contains a raw target id"),
                    )
                })?;
                let target = resolved.get(name).ok_or_else(|| {
                    AelioError::new(
                        ReasonCode::Validation,
                        format!("flow {flow_id} references unknown binding {name}"),
                    )
                })?;
                object.insert("id".into(), serde_json::Value::String(target.clone()));
            }
            for child in object.values_mut() {
                rewrite_calls(child, resolved, flow_id)?;
            }
        }
        serde_json::Value::Array(array) => {
            for child in array {
                rewrite_calls(child, resolved, flow_id)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn count_parks(
    value: &serde_json::Value,
    inside_loop: bool,
    flow_id: &str,
) -> Result<u64, AelioError> {
    match value {
        serde_json::Value::Object(object) => {
            let op = object.get("op").and_then(serde_json::Value::as_str);
            if op == Some("Park") {
                if inside_loop {
                    return Err(AelioError::new(
                        ReasonCode::Validation,
                        format!(
                            "flow {flow_id} lowering cannot place Park inside Loop in format 1"
                        ),
                    ));
                }
                return Ok(1);
            }
            let nested_loop = inside_loop || op == Some("Loop");
            object.values().try_fold(0_u64, |total, child| {
                count_parks(child, nested_loop, flow_id).and_then(|count| {
                    total.checked_add(count).ok_or_else(|| {
                        AelioError::new(ReasonCode::BudgetExceeded, "flow Park count overflow")
                    })
                })
            })
        }
        serde_json::Value::Array(array) => array.iter().try_fold(0_u64, |total, child| {
            count_parks(child, inside_loop, flow_id).and_then(|count| {
                total.checked_add(count).ok_or_else(|| {
                    AelioError::new(ReasonCode::BudgetExceeded, "flow Park count overflow")
                })
            })
        }),
        _ => Ok(0),
    }
}

fn runtime_error(error: aelio_runtime::RuntimeError) -> AelioError {
    let code = match &error {
        aelio_runtime::RuntimeError::Invalid(_) => ReasonCode::Validation,
        aelio_runtime::RuntimeError::Conflict(_) => ReasonCode::Conflict,
        aelio_runtime::RuntimeError::NotFound(_) => ReasonCode::NotFound,
        aelio_runtime::RuntimeError::Overloaded => ReasonCode::RateLimited,
        aelio_runtime::RuntimeError::Kernel { code, .. } if code.contains("denied") => {
            ReasonCode::Denied
        }
        aelio_runtime::RuntimeError::Kernel { .. } => ReasonCode::ToolError,
        aelio_runtime::RuntimeError::Store(_) | aelio_runtime::RuntimeError::Host(_) => {
            ReasonCode::Unavailable
        }
        aelio_runtime::RuntimeError::Internal(_) => ReasonCode::Internal,
    };
    AelioError::new(code, error.to_string())
}
