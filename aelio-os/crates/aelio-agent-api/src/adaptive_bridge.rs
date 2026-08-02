//! Fail-closed bridge from adaptive selection to the authoritative artifact runtime.
//!
//! This module deliberately has no selection logic. It only validates a sealed decision, verifies
//! the selected immutable artifact against the repository, and then either invokes that artifact
//! through `aelio-runtime` or records non-executable build demand.

use aelio_agent::adaptive::{
    AbstainReasonV1, AdaptiveDecisionEnvelopeV1, AdaptiveDecisionV1, CapabilityReasonV1,
};
use aelio_agent::{AelioError, ReasonCode};
use aelio_runtime::{
    ArtifactClass, ArtifactEffect, ArtifactStatus, CapabilityReason, CapabilityRequest,
    CapabilityRequestDraft, HarnessBody, ProcedureArtifactDraft, Runtime, TurnReply, TurnSubmit,
};
use std::collections::HashSet;

const VENDOR_TENANT: &str = "aelio.vendor";

#[derive(Debug)]
pub enum AdaptiveConsumption {
    Invoked(TurnReply),
    Reply(aelio_render::RenderFrame),
    Demand(CapabilityRequest),
    Abstained(AbstainReasonV1),
}

#[derive(Clone)]
pub struct AdaptiveDecisionConsumer {
    runtime: Runtime,
}

impl AdaptiveDecisionConsumer {
    pub fn new(runtime: Runtime) -> Self {
        Self { runtime }
    }

    pub(crate) fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    pub fn verify_atomic_pin(
        &self,
        tenant: &str,
        artifact: &aelio_agent::adaptive::ArtifactPinV1,
    ) -> Result<(), AelioError> {
        artifact.validate()?;
        self.ensure_atomic(
            tenant,
            &artifact.id,
            artifact.version,
            Some(&artifact.hash),
            0,
            &mut HashSet::new(),
        )
    }

    pub fn verify_executable_pin(
        &self,
        tenant: &str,
        artifact: &aelio_agent::adaptive::ArtifactPinV1,
    ) -> Result<(), AelioError> {
        artifact.validate()?;
        let repository = self.runtime.artifact_repository().map_err(runtime_error)?;
        let tenant_record = repository
            .get(tenant, &artifact.id, artifact.version)
            .map_err(artifact_error)?;
        let record = if let Some(record) = tenant_record {
            record
        } else {
            repository
                .get(VENDOR_TENANT, &artifact.id, artifact.version)
                .map_err(artifact_error)?
                .ok_or_else(|| {
                    AelioError::new(
                        ReasonCode::NotFound,
                        format!("selected artifact {} was not found", artifact.key()),
                    )
                })?
        };
        if record.artifact.hash != artifact.hash {
            return Err(AelioError::new(
                ReasonCode::Conflict,
                format!(
                    "selected artifact {} hash no longer matches",
                    artifact.key()
                ),
            ));
        }
        if !matches!(
            record.status,
            ArtifactStatus::Canary | ArtifactStatus::Promoted
        ) {
            return Err(AelioError::new(
                ReasonCode::Denied,
                format!("selected artifact {} is not admitted", artifact.key()),
            ));
        }
        if !matches!(
            record.artifact.class,
            ArtifactClass::Flow | ArtifactClass::Harness | ArtifactClass::Procedure
        ) {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!("selected artifact {} is not executable", artifact.key()),
            ));
        }
        Ok(())
    }

    /// Consume a sealed decision without adding authority to it. `instance_id` is supplied by the
    /// durable caller and must remain stable across retries and continuation wakes.
    pub fn consume(
        &self,
        tenant: &str,
        instance_id: &str,
        decision: AdaptiveDecisionEnvelopeV1,
    ) -> Result<AdaptiveConsumption, AelioError> {
        decision.validate()?;
        match decision.selected {
            AdaptiveDecisionV1::Invoke {
                artifact,
                projected_input,
            } => {
                let repository = self.runtime.artifact_repository().map_err(runtime_error)?;
                let tenant_record = repository
                    .get(tenant, &artifact.id, artifact.version)
                    .map_err(artifact_error)?;
                let (resolved_tenant, record) = if let Some(record) = tenant_record {
                    (tenant, record)
                } else {
                    let record = repository
                        .get(VENDOR_TENANT, &artifact.id, artifact.version)
                        .map_err(artifact_error)?
                        .ok_or_else(|| {
                            AelioError::new(
                                ReasonCode::NotFound,
                                format!("selected artifact {} was not found", artifact.key()),
                            )
                        })?;
                    (VENDOR_TENANT, record)
                };
                if record.artifact.hash != artifact.hash {
                    return Err(AelioError::new(
                        ReasonCode::Conflict,
                        format!(
                            "selected artifact {} hash no longer matches",
                            artifact.key()
                        ),
                    ));
                }
                if !matches!(
                    record.status,
                    ArtifactStatus::Canary | ArtifactStatus::Promoted
                ) {
                    return Err(AelioError::new(
                        ReasonCode::Denied,
                        format!("selected artifact {} is not admitted", artifact.key()),
                    ));
                }
                if !matches!(
                    record.artifact.class,
                    ArtifactClass::Flow | ArtifactClass::Harness | ArtifactClass::Procedure
                ) {
                    return Err(AelioError::new(
                        ReasonCode::Validation,
                        format!("selected artifact {} is not executable", artifact.key()),
                    ));
                }
                self.runtime
                    .invoke_pinned_artifact(TurnSubmit {
                        tenant: resolved_tenant.to_owned(),
                        instance_id: instance_id.to_owned(),
                        flow_id: artifact.id,
                        flow_rev: artifact.version.to_string(),
                        input: projected_input,
                    })
                    .map(AdaptiveConsumption::Invoked)
                    .map_err(runtime_error)
            }
            AdaptiveDecisionV1::Reply { frame } => Ok(AdaptiveConsumption::Reply(frame)),
            AdaptiveDecisionV1::Insufficient { request } => {
                let reason = match request.reason {
                    CapabilityReasonV1::NoCandidate
                    | CapabilityReasonV1::MissingTool
                    | CapabilityReasonV1::MissingFlow
                    | CapabilityReasonV1::MissingDataset => CapabilityReason::MissingCapability,
                    CapabilityReasonV1::PolicyUnavailable => CapabilityReason::InterfaceNotClosed,
                };
                self.runtime
                    .record_capability_request(
                        tenant,
                        CapabilityRequestDraft {
                            normalized_need: request.need,
                            inputs: vec![],
                            output: "aelio.turn.output@1".into(),
                            allowed_effects: Vec::<ArtifactEffect>::new(),
                            requester: "adaptive.turn".into(),
                            reason,
                            evidence_refs: vec![request.situation_hash],
                        },
                    )
                    .map(AdaptiveConsumption::Demand)
                    .map_err(runtime_error)
            }
            AdaptiveDecisionV1::Abstain { reason } => Ok(AdaptiveConsumption::Abstained(reason)),
        }
    }

    /// Hot-turn consumption currently admits only pure artifacts proven not to contain a Park or
    /// a nested suspendable dependency. This gives the durable agent turn one completion boundary;
    /// suspendable artifacts use the runtime's explicit public continuation surface instead.
    pub fn consume_atomic(
        &self,
        tenant: &str,
        instance_id: &str,
        decision: AdaptiveDecisionEnvelopeV1,
    ) -> Result<AdaptiveConsumption, AelioError> {
        decision.validate()?;
        if let AdaptiveDecisionV1::Invoke { artifact, .. } = &decision.selected {
            self.verify_atomic_pin(tenant, artifact)?;
        }
        self.consume(tenant, instance_id, decision)
    }

    fn ensure_atomic(
        &self,
        tenant: &str,
        id: &str,
        version: u32,
        expected_hash: Option<&str>,
        depth: usize,
        visited: &mut HashSet<String>,
    ) -> Result<(), AelioError> {
        if depth > 32 {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "atomic artifact dependency depth exceeds 32",
            ));
        }
        let repository = self.runtime.artifact_repository().map_err(runtime_error)?;
        let tenant_record = repository
            .get(tenant, id, version)
            .map_err(artifact_error)?;
        let (resolved_tenant, record) = if let Some(record) = tenant_record {
            (tenant, record)
        } else {
            let record = repository
                .get(VENDOR_TENANT, id, version)
                .map_err(artifact_error)?
                .ok_or_else(|| {
                    AelioError::new(
                        ReasonCode::NotFound,
                        format!("selected artifact {id}@{version} was not found"),
                    )
                })?;
            (VENDOR_TENANT, record)
        };
        if expected_hash.is_some_and(|hash| hash != record.artifact.hash) {
            return Err(AelioError::new(
                ReasonCode::Conflict,
                format!("selected artifact {id}@{version} hash no longer matches"),
            ));
        }
        if !matches!(
            record.status,
            ArtifactStatus::Canary | ArtifactStatus::Promoted
        ) {
            return Err(AelioError::new(
                ReasonCode::Denied,
                format!("selected artifact {id}@{version} is not admitted"),
            ));
        }
        if record
            .artifact
            .effects
            .iter()
            .any(|effect| *effect != ArtifactEffect::Pure)
        {
            return Err(AelioError::new(
                ReasonCode::Denied,
                "hot-turn adaptive invocation is limited to pure artifacts until principal grants are carried by the closed decision",
            ));
        }
        let key = format!("{resolved_tenant}:{id}@{version}");
        if !visited.insert(key) {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "atomic artifact dependencies contain a cycle",
            ));
        }
        let result = match record.artifact.class {
            ArtifactClass::Flow => {
                if contains_park(&record.artifact.body)
                    || record
                        .artifact
                        .body
                        .get("targets")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|targets| {
                            targets.iter().any(|target| {
                                target.get("class").and_then(serde_json::Value::as_str)
                                    == Some("flow")
                            })
                        })
                {
                    Err(AelioError::new(
                        ReasonCode::Denied,
                        "suspendable or nested Flow is not eligible for atomic hot-turn invocation",
                    ))
                } else {
                    Ok(())
                }
            }
            ArtifactClass::Harness => {
                let harness: HarnessBody = serde_json::from_value(record.artifact.body.clone())
                    .map_err(|_| {
                        AelioError::new(ReasonCode::Validation, "invalid Harness artifact body")
                    })?;
                for node in harness.nodes {
                    let (child_id, child_version) = parse_pin(&node.artifact)?;
                    self.ensure_atomic(
                        resolved_tenant,
                        child_id,
                        child_version,
                        None,
                        depth + 1,
                        visited,
                    )?;
                }
                Ok(())
            }
            ArtifactClass::Procedure => {
                let procedure: ProcedureArtifactDraft =
                    serde_json::from_value(record.artifact.body.clone()).map_err(|_| {
                        AelioError::new(ReasonCode::Validation, "invalid Procedure artifact body")
                    })?;
                let (child_id, child_version) = parse_pin(&procedure.implementation)?;
                self.ensure_atomic(
                    resolved_tenant,
                    child_id,
                    child_version,
                    None,
                    depth + 1,
                    visited,
                )
            }
            _ => Err(AelioError::new(
                ReasonCode::Validation,
                format!("selected artifact {id}@{version} is not executable"),
            )),
        };
        visited.remove(&format!("{resolved_tenant}:{id}@{version}"));
        result
    }
}

fn contains_park(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.get("op").and_then(serde_json::Value::as_str) == Some("Park")
                || map.values().any(contains_park)
        }
        serde_json::Value::Array(values) => values.iter().any(contains_park),
        _ => false,
    }
}

fn parse_pin(pin: &str) -> Result<(&str, u32), AelioError> {
    let (id, version) = pin.rsplit_once('@').ok_or_else(|| {
        AelioError::new(ReasonCode::Validation, "artifact dependency is not pinned")
    })?;
    let version = version.parse::<u32>().map_err(|_| {
        AelioError::new(
            ReasonCode::Validation,
            "artifact dependency version is invalid",
        )
    })?;
    if id.is_empty() || version == 0 {
        return Err(AelioError::new(
            ReasonCode::Validation,
            "artifact dependency pin is invalid",
        ));
    }
    Ok((id, version))
}

fn artifact_error(error: aelio_runtime::ArtifactError) -> AelioError {
    use aelio_runtime::ArtifactError;
    let reason = match error {
        ArtifactError::Invalid(_) | ArtifactError::HashMismatch { .. } => ReasonCode::Validation,
        ArtifactError::Conflict(_) | ArtifactError::IllegalTransition(_) => ReasonCode::Conflict,
        ArtifactError::NotFound(_) => ReasonCode::NotFound,
        ArtifactError::Store(_) | ArtifactError::Corrupt(_) => ReasonCode::Unavailable,
    };
    AelioError::new(reason, error.to_string())
}

fn runtime_error(error: aelio_runtime::RuntimeError) -> AelioError {
    use aelio_runtime::RuntimeError;
    let reason = match &error {
        RuntimeError::Invalid(_) => ReasonCode::Validation,
        RuntimeError::Conflict(_) => ReasonCode::Conflict,
        RuntimeError::NotFound(_) => ReasonCode::NotFound,
        RuntimeError::Overloaded => ReasonCode::RateLimited,
        RuntimeError::Kernel { code, .. } if code.contains("denied") => ReasonCode::Denied,
        RuntimeError::Kernel { .. } => ReasonCode::ToolError,
        RuntimeError::Store(_) | RuntimeError::Host(_) => ReasonCode::Unavailable,
        RuntimeError::Internal(_) => ReasonCode::Internal,
    };
    AelioError::new(reason, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aelio_agent::adaptive::{
        AdaptiveDecisionEnvelopeV1, ArtifactCandidateV1, ArtifactPinV1, CandidateReasonV1,
        CapabilityRequestV1,
    };
    use aelio_runtime::{FlowPush, RuntimeConfig, SandboxCase, SandboxLimits, DEFAULT_QUEUE_DEPTH};

    fn runtime() -> (tempfile::TempDir, Runtime) {
        let directory = tempfile::tempdir().unwrap();
        let runtime = Runtime::open(RuntimeConfig {
            data_dir: directory.path().into(),
            host_url: None,
            host_token: None,
            event_key_secret: [44; 32],
            queue_depth: DEFAULT_QUEUE_DEPTH,
        })
        .unwrap();
        (directory, runtime)
    }

    fn flow() -> FlowPush {
        FlowPush {
            tenant: "tenant-a".into(),
            flow_id: "procedure.greeting".into(),
            flow_rev: "1".into(),
            program: serde_json::json!({
                "nid":"reply","op":"Const","v":{"text":"Hello from the admitted artifact"}
            }),
            targets: vec![],
            prompts: vec![],
        }
    }

    fn invoke(pin: ArtifactPinV1) -> AdaptiveDecisionEnvelopeV1 {
        AdaptiveDecisionEnvelopeV1::seal(
            vec![ArtifactCandidateV1 {
                artifact: pin.clone(),
                score: 1.0,
                reason: CandidateReasonV1::ExactSituation,
            }],
            AdaptiveDecisionV1::Invoke {
                artifact: pin,
                projected_input: serde_json::json!({"utterance":"hello"}),
            },
        )
        .unwrap()
    }

    fn pin(runtime: &Runtime) -> ArtifactPinV1 {
        let record = runtime
            .artifact_repository()
            .unwrap()
            .get("tenant-a", "procedure.greeting", 1)
            .unwrap()
            .unwrap();
        ArtifactPinV1 {
            id: record.artifact.id,
            version: record.artifact.version,
            hash: record.artifact.hash,
        }
    }

    #[test]
    fn exact_admitted_hash_invokes_and_retry_replays() {
        let (_directory, runtime) = runtime();
        let flow = flow();
        runtime.push_flow(flow.clone()).unwrap();
        let cases = (0..20)
            .map(|index| SandboxCase {
                input: serde_json::json!({"case":index}),
                wakes: vec![],
                expect_park: false,
                expected: serde_json::json!({"text":"Hello from the admitted artifact"}),
                fixtures: vec![],
            })
            .collect::<Vec<_>>();
        let gated = runtime
            .gate_flow(
                &flow,
                &cases,
                SandboxLimits::default(),
                Some("deployer:test".into()),
            )
            .unwrap();
        assert_eq!(gated.record.status, ArtifactStatus::Canary);

        let consumer = AdaptiveDecisionConsumer::new(runtime.clone());
        let decision = invoke(pin(&runtime));
        let first = consumer
            .consume("tenant-a", "adaptive-instance-1", decision.clone())
            .unwrap();
        let replay = consumer
            .consume("tenant-a", "adaptive-instance-1", decision)
            .unwrap();
        assert!(matches!(
            first,
            AdaptiveConsumption::Invoked(TurnReply::Completed { .. })
        ));
        assert!(matches!(
            replay,
            AdaptiveConsumption::Invoked(TurnReply::Completed { .. })
        ));
    }

    #[test]
    fn hash_mismatch_and_unadmitted_artifact_fail_closed() {
        let (_directory, runtime) = runtime();
        runtime.push_flow(flow()).unwrap();
        let consumer = AdaptiveDecisionConsumer::new(runtime.clone());

        let denied = consumer
            .consume("tenant-a", "adaptive-instance-2", invoke(pin(&runtime)))
            .unwrap_err();
        assert_eq!(denied.code, ReasonCode::Denied);

        let mut wrong_pin = pin(&runtime);
        wrong_pin.hash = "f".repeat(64);
        let mismatch = consumer
            .consume("tenant-a", "adaptive-instance-3", invoke(wrong_pin))
            .unwrap_err();
        assert_eq!(mismatch.code, ReasonCode::Conflict);
    }

    #[test]
    fn insufficiency_records_deduplicated_non_executable_demand() {
        let (_directory, runtime) = runtime();
        let consumer = AdaptiveDecisionConsumer::new(runtime);
        let decision = AdaptiveDecisionEnvelopeV1::seal(
            vec![],
            AdaptiveDecisionV1::Insufficient {
                request: CapabilityRequestV1 {
                    need: "locate a compatible invoice-list procedure".into(),
                    reason: CapabilityReasonV1::NoCandidate,
                    situation_hash: "a".repeat(64),
                },
            },
        )
        .unwrap();
        let first = consumer
            .consume("tenant-a", "unused-instance", decision.clone())
            .unwrap();
        let second = consumer
            .consume("tenant-a", "unused-instance", decision)
            .unwrap();
        let AdaptiveConsumption::Demand(first) = first else {
            panic!("insufficiency must become demand")
        };
        let AdaptiveConsumption::Demand(second) = second else {
            panic!("insufficiency must become demand")
        };
        assert_eq!(first.request_id, second.request_id);
        assert_eq!(first.demand_count, 1);
        assert_eq!(second.demand_count, 2);
        assert_eq!(second.status, aelio_runtime::CapabilityStatus::Open);
    }

    #[test]
    fn suspendable_artifact_is_rejected_before_hot_turn_execution() {
        let (_directory, runtime) = runtime();
        let flow = FlowPush {
            tenant: "tenant-a".into(),
            flow_id: "procedure.waiting".into(),
            flow_rev: "1".into(),
            program: serde_json::json!({
                "nid":"wait","op":"Park","until":{"kind":"event"},"into":"wake"
            }),
            targets: vec![],
            prompts: vec![],
        };
        runtime.push_flow(flow.clone()).unwrap();
        let cases = (0..20)
            .map(|index| SandboxCase {
                input: serde_json::json!({"case":index}),
                wakes: vec![],
                expect_park: true,
                expected: serde_json::json!({"case":index}),
                fixtures: vec![],
            })
            .collect::<Vec<_>>();
        runtime
            .gate_flow(
                &flow,
                &cases,
                SandboxLimits::default(),
                Some("deployer:test".into()),
            )
            .unwrap();
        let record = runtime
            .artifact_repository()
            .unwrap()
            .get("tenant-a", "procedure.waiting", 1)
            .unwrap()
            .unwrap();
        let decision = invoke(ArtifactPinV1 {
            id: record.artifact.id,
            version: record.artifact.version,
            hash: record.artifact.hash,
        });
        let error = AdaptiveDecisionConsumer::new(runtime)
            .consume_atomic("tenant-a", "must-not-exist", decision)
            .unwrap_err();
        assert_eq!(error.code, ReasonCode::Denied);
    }
}
