//! Closed output of the adaptive layer.
//!
//! Understanding and retrieval may be intelligent; consumption is not. This contract can request
//! one exact artifact invocation, return a pure RenderFrame, report an insufficiency, or abstain.
//! It cannot carry a tool call, Flow state, arbitrary program, or an unpinned executable id.

use crate::types::{AelioError, AelioResult, ReasonCode};
use aelio_render::RenderFrame;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const ADAPTIVE_DECISION_FORMAT: u32 = 1;
pub const MAX_ADAPTIVE_CANDIDATES: usize = 8;
pub const MAX_ADAPTIVE_REPLY_BYTES: usize = 64 * 1024;
const MAX_ID_BYTES: usize = 192;
const MAX_REASON_BYTES: usize = 1_000;
const MAX_PROJECTED_INPUT_BYTES: usize = 1024 * 1024;
const MAX_PROJECTED_DEPTH: usize = 32;
const MAX_PROJECTED_KEYS: usize = 1_024;
const MAX_PROJECTED_ITEMS: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPinV1 {
    pub id: String,
    pub version: u32,
    /// Exact immutable artifact hash, lowercase hexadecimal.
    pub hash: String,
}

impl ArtifactPinV1 {
    pub fn key(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }

    pub fn validate(&self) -> AelioResult<()> {
        validate_id("artifact id", &self.id)?;
        if self.version == 0 {
            return invalid("artifact version must be positive");
        }
        validate_hash("artifact hash", &self.hash)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateReasonV1 {
    ExactSituation,
    NearSituation,
    DeclaredFallback,
    ActiveContinuation,
    AuthoredFlowActivation,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactCandidateV1 {
    pub artifact: ArtifactPinV1,
    /// Kernel-measured score. The model never supplies a confidence field.
    pub score: f64,
    pub reason: CandidateReasonV1,
}

impl ArtifactCandidateV1 {
    fn validate(&self) -> AelioResult<()> {
        self.artifact.validate()?;
        if !self.score.is_finite() || !(-1.0..=1.0).contains(&self.score) {
            return invalid("candidate score must be finite within -1..=1");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityReasonV1 {
    NoCandidate,
    MissingTool,
    MissingFlow,
    MissingDataset,
    PolicyUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRequestV1 {
    pub need: String,
    pub reason: CapabilityReasonV1,
    pub situation_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdaptiveFlowDemandV1 {
    pub flow_id: String,
    pub flow_version: String,
    pub effectful: bool,
}

impl AdaptiveFlowDemandV1 {
    pub fn validate(&self) -> AelioResult<()> {
        validate_id("flow id", &self.flow_id)?;
        if self.flow_version.is_empty()
            || self.flow_version.len() > MAX_ID_BYTES
            || !self
                .flow_version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return invalid("flow version must be a canonical bounded identifier");
        }
        Ok(())
    }
}

impl CapabilityRequestV1 {
    fn validate(&self) -> AelioResult<()> {
        if self.need.trim().is_empty() || self.need.len() > MAX_REASON_BYTES {
            return invalid("capability need must contain 1..=1000 bytes");
        }
        validate_situation_hash(&self.situation_hash)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AbstainReasonV1 {
    Ambiguous,
    Unsafe,
    PolicyDenied,
    UnadmittedArtifact,
    InvalidCandidate,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdaptiveDecisionV1 {
    Invoke {
        artifact: ArtifactPinV1,
        projected_input: serde_json::Value,
    },
    Reply {
        frame: RenderFrame,
    },
    Insufficient {
        request: CapabilityRequestV1,
    },
    Abstain {
        reason: AbstainReasonV1,
    },
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdaptiveDecisionEnvelopeV1 {
    pub format: u32,
    pub candidates: Vec<ArtifactCandidateV1>,
    pub selected: AdaptiveDecisionV1,
    pub decision_hash: String,
}

/// Closed result accepted back from the artifact authority. Runtime artifacts return data, not an
/// instruction for the adaptive layer to interpret. Exactly one of `text` and `frame` is allowed.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdaptiveArtifactOutputV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<RenderFrame>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdaptiveArtifactTurnV1 {
    pub output: AdaptiveArtifactOutputV1,
    pub suspended: bool,
}

impl AdaptiveArtifactOutputV1 {
    pub fn from_value(value: serde_json::Value) -> AelioResult<Self> {
        let output: Self = serde_json::from_value(value)
            .map_err(|error| AelioError::new(ReasonCode::Validation, error.to_string()))?;
        output.validate()?;
        Ok(output)
    }

    pub fn validate(&self) -> AelioResult<()> {
        match (&self.text, &self.frame) {
            (Some(text), None) => {
                if text.trim().is_empty() || text.len() > MAX_ADAPTIVE_REPLY_BYTES {
                    return invalid("adaptive artifact text must contain 1..=65536 bytes");
                }
            }
            (None, Some(frame)) => {
                aelio_render::validate_render_frame(frame, None)
                    .map_err(|error| AelioError::new(ReasonCode::Validation, error.to_string()))?;
            }
            _ => {
                return invalid(
                    "adaptive artifact output must contain exactly one of text or frame",
                )
            }
        }
        Ok(())
    }
}

/// Object-safe authority boundary installed by the production server. The default world has no
/// authority and therefore cannot turn a pin into effects by itself.
pub trait AdaptiveArtifactHost: Send {
    /// Prove that an exact artifact pin is present, admitted, hash-matched, and executable in the
    /// installed authority. Callers use this before publishing catalog state that would make the
    /// artifact selectable; validation must therefore have no execution side effects.
    fn verify_executable_pin(
        &mut self,
        _tenant: &str,
        _artifact: &ArtifactPinV1,
    ) -> AelioResult<()> {
        Err(AelioError::new(
            ReasonCode::Unavailable,
            "the authoritative artifact verifier is not installed",
        ))
    }

    fn invoke(
        &mut self,
        tenant: &str,
        instance_id: &str,
        decision: &AdaptiveDecisionEnvelopeV1,
    ) -> AelioResult<AdaptiveArtifactTurnV1>;

    /// Read-only continuation probe used by the flow gate. It must not wake, advance, replace or
    /// otherwise mutate the subject's active artifact.
    fn has_active_subject(&mut self, _tenant: &str, _subject: &str) -> AelioResult<bool> {
        Ok(false)
    }

    fn resume_subject(
        &mut self,
        _tenant: &str,
        _subject: &str,
        _projected_input: serde_json::Value,
    ) -> AelioResult<Option<AdaptiveArtifactTurnV1>> {
        Ok(None)
    }

    fn start_subject(
        &mut self,
        _tenant: &str,
        _subject: &str,
        _instance_id: &str,
        _decision: &AdaptiveDecisionEnvelopeV1,
    ) -> AelioResult<AdaptiveArtifactTurnV1> {
        Err(AelioError::new(
            ReasonCode::Unavailable,
            "the authoritative subject artifact host is not installed",
        ))
    }

    fn record_flow_demand(
        &mut self,
        _tenant: &str,
        _demand: &AdaptiveFlowDemandV1,
    ) -> AelioResult<()> {
        Err(AelioError::new(
            ReasonCode::Unavailable,
            "the authoritative capability-demand host is not installed",
        ))
    }

    /// Return the metadata-only authoritative ledger narrative produced by the most recent host
    /// operation. Implementations must never include ledger payloads, prompts, PII or secrets.
    fn take_decision_trace(&mut self) -> Option<String> {
        None
    }
}

#[derive(Default)]
pub struct UnavailableAdaptiveArtifactHost;

impl AdaptiveArtifactHost for UnavailableAdaptiveArtifactHost {
    fn invoke(
        &mut self,
        _tenant: &str,
        _instance_id: &str,
        _decision: &AdaptiveDecisionEnvelopeV1,
    ) -> AelioResult<AdaptiveArtifactTurnV1> {
        Err(AelioError::new(
            ReasonCode::Unavailable,
            "the authoritative artifact host is not installed",
        ))
    }
}

impl AdaptiveDecisionEnvelopeV1 {
    pub fn seal(
        candidates: Vec<ArtifactCandidateV1>,
        selected: AdaptiveDecisionV1,
    ) -> AelioResult<Self> {
        let mut envelope = Self {
            format: ADAPTIVE_DECISION_FORMAT,
            candidates,
            selected,
            decision_hash: String::new(),
        };
        envelope.validate_fields()?;
        envelope.decision_hash = envelope.compute_hash()?;
        Ok(envelope)
    }

    pub fn validate(&self) -> AelioResult<()> {
        self.validate_fields()?;
        let expected = self.compute_hash()?;
        if self.decision_hash != expected {
            return invalid("adaptive decision hash does not match its canonical content");
        }
        Ok(())
    }

    pub fn compute_hash(&self) -> AelioResult<String> {
        // Mother §4.3: content identity is BLAKE3 over canonical Sol bytes, never a second
        // hash regime over serde_json text.
        let json = serde_json::json!({
            "format": self.format,
            "candidates": self.candidates,
            "selected": self.selected,
        });
        let sol = aelio_kernel::json_from(&json).map_err(|error| {
            AelioError::new(
                ReasonCode::Validation,
                format!("adaptive decision is not Sol-canonical: {error}"),
            )
        })?;
        Ok(aelio_sol::value_hash(&sol))
    }

    fn validate_fields(&self) -> AelioResult<()> {
        if self.format != ADAPTIVE_DECISION_FORMAT {
            return invalid("unsupported adaptive decision format");
        }
        if self.candidates.len() > MAX_ADAPTIVE_CANDIDATES {
            return invalid("adaptive decision exceeds the eight-candidate bound");
        }
        let mut pins = BTreeSet::new();
        for candidate in &self.candidates {
            candidate.validate()?;
            if !pins.insert((
                candidate.artifact.id.as_str(),
                candidate.artifact.version,
                candidate.artifact.hash.as_str(),
            )) {
                return invalid("adaptive candidate pins must be unique");
            }
        }
        match &self.selected {
            AdaptiveDecisionV1::Invoke {
                artifact,
                projected_input,
            } => {
                artifact.validate()?;
                if !self
                    .candidates
                    .iter()
                    .any(|candidate| candidate.artifact == *artifact)
                {
                    return invalid("selected invocation must be one of the measured candidates");
                }
                validate_projected_input(projected_input)?;
            }
            AdaptiveDecisionV1::Reply { frame } => {
                aelio_render::validate_render_frame(frame, None)
                    .map_err(|error| AelioError::new(ReasonCode::Validation, error.to_string()))?;
            }
            AdaptiveDecisionV1::Insufficient { request } => request.validate()?,
            AdaptiveDecisionV1::Abstain { .. } => {}
        }
        Ok(())
    }
}

/// Convert a measured tier lookup into the only decision shape the reactor may consume. Tier-2/3
/// paths remain authoring/build demand until they have a unified artifact pin; an old promoted
/// path without such a pin is explicitly shadow-only and fails closed.
pub fn from_tier_lookup(
    lookup: &crate::abilities::learn::TierLookup,
    situation_hash: &str,
    projected_input: serde_json::Value,
) -> AelioResult<AdaptiveDecisionEnvelopeV1> {
    match lookup.tier {
        crate::types::LookupTier::Tier0 | crate::types::LookupTier::Tier1 => {
            let Some(artifact) = lookup.artifact.clone() else {
                return AdaptiveDecisionEnvelopeV1::seal(
                    vec![],
                    AdaptiveDecisionV1::Abstain {
                        reason: AbstainReasonV1::UnadmittedArtifact,
                    },
                );
            };
            let reason = if lookup.tier == crate::types::LookupTier::Tier0 {
                CandidateReasonV1::ExactSituation
            } else {
                CandidateReasonV1::NearSituation
            };
            AdaptiveDecisionEnvelopeV1::seal(
                vec![ArtifactCandidateV1 {
                    artifact: artifact.clone(),
                    score: lookup.margin.clamp(-1.0, 1.0),
                    reason,
                }],
                AdaptiveDecisionV1::Invoke {
                    artifact,
                    projected_input,
                },
            )
        }
        crate::types::LookupTier::Tier2 | crate::types::LookupTier::Tier3 => {
            AdaptiveDecisionEnvelopeV1::seal(
                vec![],
                AdaptiveDecisionV1::Insufficient {
                    request: CapabilityRequestV1 {
                        need: "no admitted artifact satisfies the measured situation".into(),
                        reason: CapabilityReasonV1::NoCandidate,
                        situation_hash: situation_hash.into(),
                    },
                },
            )
        }
    }
}

fn validate_projected_input(value: &serde_json::Value) -> AelioResult<()> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| AelioError::new(ReasonCode::Validation, error.to_string()))?;
    if bytes.len() > MAX_PROJECTED_INPUT_BYTES {
        return invalid("projected artifact input exceeds 1 MiB");
    }
    validate_value_shape(value, 0)
}

fn validate_value_shape(value: &serde_json::Value, depth: usize) -> AelioResult<()> {
    if depth > MAX_PROJECTED_DEPTH {
        return invalid("projected artifact input exceeds depth 32");
    }
    match value {
        serde_json::Value::Array(values) => {
            if values.len() > MAX_PROJECTED_ITEMS {
                return invalid("projected artifact input exceeds 10000 list items");
            }
            for value in values {
                validate_value_shape(value, depth + 1)?;
            }
        }
        serde_json::Value::Object(values) => {
            if values.len() > MAX_PROJECTED_KEYS {
                return invalid("projected artifact input exceeds 1024 map keys");
            }
            for (key, value) in values {
                if key.is_empty() || key.len() > MAX_ID_BYTES {
                    return invalid("projected artifact input contains an invalid key");
                }
                validate_value_shape(value, depth + 1)?;
            }
        }
        serde_json::Value::Number(number) => {
            if number.as_f64().is_some_and(|value| !value.is_finite()) {
                return invalid("projected artifact input contains a non-finite number");
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_id(name: &str, value: &str) -> AelioResult<()> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        return invalid(&format!("{name} is not a canonical bounded identifier"));
    }
    Ok(())
}

fn validate_hash(name: &str, value: &str) -> AelioResult<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid(&format!(
            "{name} must be 64 lowercase hexadecimal characters"
        ));
    }
    Ok(())
}

fn validate_situation_hash(value: &str) -> AelioResult<()> {
    if !matches!(value.len(), 32 | 64)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid("situation hash must be 32 or 64 lowercase hexadecimal characters");
    }
    Ok(())
}

fn invalid<T>(message: &str) -> AelioResult<T> {
    Err(AelioError::new(ReasonCode::Validation, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin() -> ArtifactPinV1 {
        ArtifactPinV1 {
            id: "procedure.greeting".into(),
            version: 1,
            hash: "a".repeat(64),
        }
    }

    #[test]
    fn exact_candidate_seals_and_detects_tampering() {
        let artifact = pin();
        let mut decision = AdaptiveDecisionEnvelopeV1::seal(
            vec![ArtifactCandidateV1 {
                artifact: artifact.clone(),
                score: 1.0,
                reason: CandidateReasonV1::ExactSituation,
            }],
            AdaptiveDecisionV1::Invoke {
                artifact,
                projected_input: serde_json::json!({"turn":{"message":"hi"}}),
            },
        )
        .unwrap();
        decision.validate().unwrap();
        let expected = aelio_sol::value_hash(
            &aelio_kernel::json_from(&serde_json::json!({
                "format": decision.format,
                "candidates": decision.candidates,
                "selected": decision.selected,
            }))
            .unwrap(),
        );
        assert_eq!(decision.decision_hash, expected);
        decision.candidates[0].score = 0.9;
        assert!(decision.validate().is_err());
    }

    #[test]
    fn selected_pin_must_be_measured_and_scores_are_finite() {
        let candidate = pin();
        let other = ArtifactPinV1 {
            id: "procedure.other".into(),
            ..candidate.clone()
        };
        assert!(AdaptiveDecisionEnvelopeV1::seal(
            vec![ArtifactCandidateV1 {
                artifact: candidate.clone(),
                score: f64::NAN,
                reason: CandidateReasonV1::NearSituation,
            }],
            AdaptiveDecisionV1::Invoke {
                artifact: candidate,
                projected_input: serde_json::json!({}),
            },
        )
        .is_err());
        assert!(AdaptiveDecisionEnvelopeV1::seal(
            vec![ArtifactCandidateV1 {
                artifact: pin(),
                score: 1.0,
                reason: CandidateReasonV1::ExactSituation,
            }],
            AdaptiveDecisionV1::Invoke {
                artifact: other,
                projected_input: serde_json::json!({}),
            },
        )
        .is_err());
    }

    #[test]
    fn schema_is_closed_and_cannot_carry_a_tool_or_program() {
        let json = serde_json::json!({
            "format":1,
            "candidates":[],
            "selected":{"decision":"abstain","reason":"unsafe","tool_id":"delete_all"},
            "decision_hash":"a".repeat(64)
        });
        assert!(serde_json::from_value::<AdaptiveDecisionEnvelopeV1>(json).is_err());
        let json = serde_json::json!({
            "format":1,
            "candidates":[],
            "selected":{"decision":"insufficient","request":{
                "need":"answer customer","reason":"no_candidate",
                "situation_hash":"b".repeat(64),"program":{"op":"exec"}
            }},
            "decision_hash":"a".repeat(64)
        });
        assert!(serde_json::from_value::<AdaptiveDecisionEnvelopeV1>(json).is_err());
    }

    #[test]
    fn pure_reply_must_be_a_valid_render_frame() {
        let frame = aelio_render::text_frame("turn-1", "frame-1", "Hello");
        AdaptiveDecisionEnvelopeV1::seal(vec![], AdaptiveDecisionV1::Reply { frame }).unwrap();
        let invalid_frame = RenderFrame {
            frame_type: "html".into(),
            frame_id: "frame-2".into(),
            mode: aelio_render::RenderMode::Append,
            turn_id: "turn-2".into(),
            blocks: vec![],
        };
        assert!(AdaptiveDecisionEnvelopeV1::seal(
            vec![],
            AdaptiveDecisionV1::Reply {
                frame: invalid_frame
            },
        )
        .is_err());
    }

    #[test]
    fn tier_lookup_is_shadow_only_until_exact_artifact_binding_exists() {
        let lookup = crate::abilities::learn::TierLookup {
            tier: crate::types::LookupTier::Tier0,
            procedure_id: Some("legacy.greeting".into()),
            path: None,
            margin: 1.0,
            artifact: None,
        };
        let held = from_tier_lookup(&lookup, &"b".repeat(64), serde_json::json!({})).unwrap();
        assert!(matches!(
            held.selected,
            AdaptiveDecisionV1::Abstain {
                reason: AbstainReasonV1::UnadmittedArtifact
            }
        ));
        let admitted = from_tier_lookup(
            &crate::abilities::learn::TierLookup {
                artifact: Some(pin()),
                ..lookup
            },
            &"b".repeat(64),
            serde_json::json!({"turn":{"message":"hi"}}),
        )
        .unwrap();
        assert!(matches!(
            admitted.selected,
            AdaptiveDecisionV1::Invoke { .. }
        ));
    }
}
