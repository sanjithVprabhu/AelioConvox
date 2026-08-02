use crate::adaptive_bridge::{AdaptiveConsumption, AdaptiveDecisionConsumer};
use aelio_agent::adaptive::{
    AdaptiveArtifactHost, AdaptiveArtifactOutputV1, AdaptiveArtifactTurnV1,
    AdaptiveDecisionEnvelopeV1, AdaptiveDecisionV1, AdaptiveFlowDemandV1,
};
use aelio_agent::{AelioError, ReasonCode};
use aelio_runtime::{Runtime, SubjectArtifactStart, TurnReply};

pub(crate) struct RuntimeAdaptiveArtifactHost {
    consumer: AdaptiveDecisionConsumer,
    last_trace: Option<String>,
}

impl RuntimeAdaptiveArtifactHost {
    pub(crate) fn new(runtime: Runtime) -> Self {
        Self {
            consumer: AdaptiveDecisionConsumer::new(runtime),
            last_trace: None,
        }
    }

    fn capture_trace(
        &mut self,
        tenant: &str,
        instance_id: &str,
        artifact: &aelio_agent::adaptive::ArtifactPinV1,
    ) {
        self.last_trace = self
            .consumer
            .runtime()
            .invocation_trace(tenant, instance_id, &artifact.id, artifact.version)
            .ok()
            .and_then(|trace| serde_json::to_string(&trace).ok());
    }
}

impl AdaptiveArtifactHost for RuntimeAdaptiveArtifactHost {
    fn verify_executable_pin(
        &mut self,
        tenant: &str,
        artifact: &aelio_agent::adaptive::ArtifactPinV1,
    ) -> Result<(), AelioError> {
        self.consumer.verify_executable_pin(tenant, artifact)
    }

    fn invoke(
        &mut self,
        tenant: &str,
        instance_id: &str,
        decision: &AdaptiveDecisionEnvelopeV1,
    ) -> Result<AdaptiveArtifactTurnV1, AelioError> {
        let artifact = match &decision.selected {
            AdaptiveDecisionV1::Invoke { artifact, .. } => Some(artifact.clone()),
            _ => None,
        };
        let result = self
            .consumer
            .consume_atomic(tenant, instance_id, decision.clone());
        if let Some(artifact) = artifact {
            self.capture_trace(tenant, instance_id, &artifact);
        }
        match result? {
            AdaptiveConsumption::Invoked(TurnReply::Completed { bag, .. }) => {
                Ok(AdaptiveArtifactTurnV1 {
                    output: AdaptiveArtifactOutputV1::from_value(bag)?,
                    suspended: false,
                })
            }
            AdaptiveConsumption::Invoked(TurnReply::Parked { .. }) => Err(AelioError::new(
                ReasonCode::Internal,
                "an artifact proven atomic parked at runtime",
            )),
            _ => Err(AelioError::new(
                ReasonCode::Validation,
                "adaptive invocation host received a non-invocation decision",
            )),
        }
    }

    fn has_active_subject(&mut self, tenant: &str, subject: &str) -> Result<bool, AelioError> {
        self.consumer
            .runtime()
            .active_subject_artifact(tenant, subject)
            .map(|active| active.is_some())
            .map_err(map_runtime_error)
    }

    fn resume_subject(
        &mut self,
        tenant: &str,
        subject: &str,
        projected_input: serde_json::Value,
    ) -> Result<Option<AdaptiveArtifactTurnV1>, AelioError> {
        let Some((record, _)) = self
            .consumer
            .runtime()
            .active_subject_artifact(tenant, subject)
            .map_err(map_runtime_error)?
        else {
            return Ok(None);
        };
        let reply = self
            .consumer
            .runtime()
            .resume_subject_artifact(tenant, subject, projected_input)
            .map_err(map_runtime_error)?;
        self.capture_trace(
            tenant,
            &record.instance_id,
            &aelio_agent::adaptive::ArtifactPinV1 {
                id: record.artifact_id,
                version: record.artifact_version,
                hash: record.artifact_hash,
            },
        );
        let (bag, suspended) = match reply {
            TurnReply::Completed { bag, .. } => (bag, false),
            TurnReply::Parked { bag, .. } => (bag, true),
        };
        Ok(Some(AdaptiveArtifactTurnV1 {
            output: AdaptiveArtifactOutputV1::from_value(bag)?,
            suspended,
        }))
    }

    fn start_subject(
        &mut self,
        tenant: &str,
        subject: &str,
        instance_id: &str,
        decision: &AdaptiveDecisionEnvelopeV1,
    ) -> Result<AdaptiveArtifactTurnV1, AelioError> {
        decision.validate()?;
        let AdaptiveDecisionV1::Invoke {
            artifact,
            projected_input,
        } = &decision.selected
        else {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "subject artifact host requires an invocation decision",
            ));
        };
        self.consumer.verify_executable_pin(tenant, artifact)?;
        let reply = self
            .consumer
            .runtime()
            .start_subject_artifact(SubjectArtifactStart {
                subject: subject.into(),
                tenant: tenant.into(),
                instance_id: instance_id.into(),
                artifact_id: artifact.id.clone(),
                artifact_version: artifact.version,
                artifact_hash: artifact.hash.clone(),
                input: projected_input.clone(),
            })
            .map_err(map_runtime_error)?;
        self.capture_trace(tenant, instance_id, artifact);
        let (bag, suspended) = match reply {
            TurnReply::Completed { bag, .. } => (bag, false),
            TurnReply::Parked { bag, .. } => (bag, true),
        };
        Ok(AdaptiveArtifactTurnV1 {
            output: AdaptiveArtifactOutputV1::from_value(bag)?,
            suspended,
        })
    }

    fn record_flow_demand(
        &mut self,
        tenant: &str,
        demand: &AdaptiveFlowDemandV1,
    ) -> Result<(), AelioError> {
        demand.validate()?;
        self.consumer
            .runtime()
            .record_capability_request(
                tenant,
                aelio_runtime::CapabilityRequestDraft {
                    normalized_need: format!(
                        "materialize authored flow {}@{} into one admitted runtime artifact",
                        demand.flow_id, demand.flow_version
                    ),
                    inputs: vec![aelio_runtime::ArtifactInput {
                        name: "turn".into(),
                        imprint: "aelio.turn.input@1".into(),
                        required: true,
                        sensitivity: "internal".into(),
                    }],
                    output: "aelio.turn.output@1".into(),
                    // The legacy FlowSpec has only an effectful boolean and cannot safely
                    // distinguish Write from External. Materialization must resolve exact tools
                    // before a reviewed BuildSpec receives an effect ceiling.
                    allowed_effects: vec![],
                    requester: "adaptive.flow".into(),
                    reason: aelio_runtime::CapabilityReason::InterfaceNotClosed,
                    evidence_refs: vec![],
                },
            )
            .map(|_| ())
            .map_err(map_runtime_error)
    }

    fn take_decision_trace(&mut self) -> Option<String> {
        self.last_trace.take()
    }
}

fn map_runtime_error(error: aelio_runtime::RuntimeError) -> AelioError {
    let code = match &error {
        aelio_runtime::RuntimeError::Invalid(_) => ReasonCode::Validation,
        aelio_runtime::RuntimeError::Conflict(_) => ReasonCode::Conflict,
        aelio_runtime::RuntimeError::NotFound(_) => ReasonCode::NotFound,
        aelio_runtime::RuntimeError::Overloaded => ReasonCode::RateLimited,
        aelio_runtime::RuntimeError::Kernel { .. } => ReasonCode::ToolError,
        aelio_runtime::RuntimeError::Store(_) | aelio_runtime::RuntimeError::Host(_) => {
            ReasonCode::Unavailable
        }
        aelio_runtime::RuntimeError::Internal(_) => ReasonCode::Internal,
    };
    AelioError::new(code, error.to_string())
}
