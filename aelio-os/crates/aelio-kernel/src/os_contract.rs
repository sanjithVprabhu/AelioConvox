//! Production process-model contracts for the Conductor / Harness OS.
//!
//! Authority: `Blueprint/conductor-harness-os-production-plan.md` §4 and §18.1.
//! These types are the normative serde surface for events, harness contracts,
//! Conductor actions, instance trees, joins, and result envelopes.
//!
//! They intentionally do **not** execute programs. Execution remains kernel Sol
//! (`instr` / `driver`) and registered `Call` targets.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ── Shared primitives ────────────────────────────────────────────────────────

/// Semantic version pin used on artifacts and Call targets (`id@version` companion).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionPinV1 {
    pub id: String,
    pub version: String,
    /// Content hash of the immutable body (blake3 hex or `blake3:…`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_hash: Option<String>,
}

impl VersionPinV1 {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("pin id must be non-empty".into());
        }
        if self.version.trim().is_empty() {
            return Err("pin version must be non-empty".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectClassV1 {
    Pure,
    Read,
    Write,
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeterminismV1 {
    Deterministic,
    LedgeredNondeterministic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuspendabilityV1 {
    Never,
    ParkAllowed,
    MustPark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactOriginV1 {
    Vendor,
    Tenant,
    Learned,
}

// ── HarnessContractV1 ────────────────────────────────────────────────────────

/// Every executable harness/program artifact must declare this contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessContractV1 {
    pub id: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_hash: Option<String>,
    pub origin: ArtifactOriginV1,
    pub description: String,
    pub input_imprint: String,
    pub output_imprint: String,
    #[serde(default)]
    pub errors: Vec<String>,
    pub effect: EffectClassV1,
    pub determinism: DeterminismV1,
    pub suspendability: SuspendabilityV1,
    #[serde(default)]
    pub children_allow: Vec<String>,
    #[serde(default)]
    pub budgets: BudgetContractV1,
    /// Canonical Sol program as JSON (App E instruction tree).
    pub program: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authoring_source: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BudgetContractV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fanout: Option<u64>,
}

impl HarnessContractV1 {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("harness contract id must be non-empty".into());
        }
        if self.version.trim().is_empty() {
            return Err("harness contract version must be non-empty".into());
        }
        if self.description.trim().is_empty() {
            return Err("harness contract description must be non-empty".into());
        }
        if self.input_imprint.trim().is_empty() || self.output_imprint.trim().is_empty() {
            return Err("input/output imprints required".into());
        }
        if !self.program.is_object() {
            return Err("program must be a Sol JSON object".into());
        }
        let op = self
            .program
            .get("op")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if op.is_empty() {
            return Err("program.op required".into());
        }
        if self.program.get("nid").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
            return Err("program.nid required".into());
        }
        Ok(())
    }

    /// Stable content fingerprint over the immutable body fields (excludes free-form metadata noise).
    pub fn content_hash(&self) -> String {
        let body = serde_json::json!({
            "id": self.id,
            "version": self.version,
            "origin": self.origin,
            "description": self.description,
            "input_imprint": self.input_imprint,
            "output_imprint": self.output_imprint,
            "errors": self.errors,
            "effect": self.effect,
            "determinism": self.determinism,
            "suspendability": self.suspendability,
            "children_allow": self.children_allow,
            "budgets": self.budgets,
            "program": self.program,
            "required_capabilities": self.required_capabilities,
        });
        let bytes = serde_json::to_vec(&body).unwrap_or_default();
        format!("blake3:{}", blake3::hash(&bytes).to_hex())
    }

    pub fn pin(&self) -> VersionPinV1 {
        VersionPinV1 {
            id: self.id.clone(),
            version: self.version.clone(),
            artifact_hash: Some(self.content_hash()),
        }
    }
}

// ── NormalizedEventV1 ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedEventV1 {
    pub event_id: String,
    pub event_type: String,
    pub tenant_id: String,
    pub subject_id: String,
    pub channel: String,
    pub occurred_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(default)]
    pub identity: EventIdentityV1,
    #[serde(default)]
    pub delivery: EventDeliveryV1,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventIdentityV1 {
    #[serde(default = "default_assurance")]
    pub assurance: String,
}

impl Default for EventIdentityV1 {
    fn default() -> Self {
        Self {
            assurance: default_assurance(),
        }
    }
}

fn default_assurance() -> String {
    "anonymous".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct EventDeliveryV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_message_id: Option<String>,
}

impl NormalizedEventV1 {
    pub fn validate(&self) -> Result<(), String> {
        for (label, value) in [
            ("event_id", self.event_id.as_str()),
            ("event_type", self.event_type.as_str()),
            ("tenant_id", self.tenant_id.as_str()),
            ("subject_id", self.subject_id.as_str()),
            ("channel", self.channel.as_str()),
            ("occurred_at", self.occurred_at.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(format!("{label} must be non-empty"));
            }
        }
        if !self.event_type.contains('.') {
            return Err("event_type should be namespaced (e.g. user.message)".into());
        }
        match self.identity.assurance.as_str() {
            "anonymous" | "verified" | "service" => {}
            other => return Err(format!("unknown identity.assurance `{other}`")),
        }
        Ok(())
    }
}

// ── ConductorActionV1 ────────────────────────────────────────────────────────

/// Exactly one validated scheduling decision per Conductor cycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConductorActionV1 {
    QuickReply {
        response_plan: serde_json::Value,
    },
    Spawn {
        harness_pin: VersionPinV1,
        input: serde_json::Value,
        #[serde(default)]
        join_policy: JoinPolicyV1,
        #[serde(default)]
        priority: u32,
    },
    SpawnMany {
        children: Vec<SpawnChildV1>,
        join_policy: JoinPolicyV1,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        merge_plan: Option<serde_json::Value>,
    },
    Resume {
        instance_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wake: Option<serde_json::Value>,
    },
    Continue {
        instance_id: String,
    },
    Suspend {
        wait_condition: serde_json::Value,
    },
    Cancel {
        instance_id: String,
        reason: String,
    },
    Escape {
        scope: String,
        condition: serde_json::Value,
    },
    CloseConversation {
        reason: String,
    },
    Ignore {
        reason: String,
    },
    DeadLetter {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diagnostics: Option<serde_json::Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnChildV1 {
    pub nid: String,
    pub harness_pin: VersionPinV1,
    pub input: serde_json::Value,
}

impl ConductorActionV1 {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Spawn {
                harness_pin,
                join_policy,
                ..
            } => {
                harness_pin.validate()?;
                join_policy.validate()?;
            }
            Self::SpawnMany {
                children,
                join_policy,
                ..
            } => {
                if children.is_empty() {
                    return Err("SpawnMany requires at least one child".into());
                }
                if children.len() > 64 {
                    return Err("SpawnMany fanout exceeds 64".into());
                }
                let mut nids = std::collections::HashSet::new();
                for child in children {
                    if child.nid.trim().is_empty() {
                        return Err("child nid must be non-empty".into());
                    }
                    if !nids.insert(child.nid.as_str()) {
                        return Err(format!("duplicate child nid `{}`", child.nid));
                    }
                    child.harness_pin.validate()?;
                }
                join_policy.validate()?;
            }
            Self::Resume { instance_id, .. }
            | Self::Continue { instance_id }
            | Self::Cancel { instance_id, .. } => {
                if instance_id.trim().is_empty() {
                    return Err("instance_id must be non-empty".into());
                }
            }
            Self::Escape { scope, .. } => {
                if scope.trim().is_empty() {
                    return Err("escape scope must be non-empty".into());
                }
            }
            Self::QuickReply { .. }
            | Self::Suspend { .. }
            | Self::CloseConversation { .. }
            | Self::Ignore { .. }
            | Self::DeadLetter { .. } => {}
        }
        Ok(())
    }
}

// ── Join / instance / result ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JoinPolicyV1 {
    All,
    AllSettled,
    Any,
    Race,
    Quorum { n: u32 },
    Supervise,
}

impl Default for JoinPolicyV1 {
    fn default() -> Self {
        Self::All
    }
}

impl JoinPolicyV1 {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Quorum { n } if *n == 0 => Err("quorum n must be >= 1".into()),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceStateV1 {
    Ready,
    Running,
    Waiting,
    Completed,
    Failed,
    Cancelled,
    DeadLettered,
}

/// Durable process record (plan §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceRecordV1 {
    pub instance_id: String,
    pub root_instance_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_instance_id: Option<String>,
    pub tenant_id: String,
    pub subject_id: String,
    pub artifact_pin: VersionPinV1,
    pub state: InstanceStateV1,
    #[serde(default)]
    pub child_ids: Vec<String>,
    #[serde(default)]
    pub join_policy: JoinPolicyV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bag_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_pin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ResultEnvelopeV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger_head: Option<String>,
}

impl InstanceRecordV1 {
    pub fn validate(&self) -> Result<(), String> {
        if self.instance_id.trim().is_empty() {
            return Err("instance_id required".into());
        }
        if self.root_instance_id.trim().is_empty() {
            return Err("root_instance_id required".into());
        }
        if self.tenant_id.trim().is_empty() || self.subject_id.trim().is_empty() {
            return Err("tenant_id and subject_id required".into());
        }
        self.artifact_pin.validate()?;
        self.join_policy.validate()?;
        if let Some(result) = &self.result {
            result.validate()?;
        }
        Ok(())
    }
}

/// Durable join edge for multi-child composition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildJoinV1 {
    pub parent_instance_id: String,
    pub join_id: String,
    pub policy: JoinPolicyV1,
    /// Ordered child nids for deterministic result maps.
    pub child_nids: Vec<String>,
    pub child_instance_ids: Vec<String>,
    #[serde(default)]
    pub completed: BTreeMap<String, ResultEnvelopeV1>,
    #[serde(default)]
    pub cancelled: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<ResultEnvelopeV1>,
}

impl ChildJoinV1 {
    pub fn validate(&self) -> Result<(), String> {
        if self.parent_instance_id.trim().is_empty() || self.join_id.trim().is_empty() {
            return Err("parent_instance_id and join_id required".into());
        }
        if self.child_nids.len() != self.child_instance_ids.len() {
            return Err("child_nids and child_instance_ids length mismatch".into());
        }
        if self.child_nids.is_empty() {
            return Err("join requires at least one child".into());
        }
        self.policy.validate()?;
        for env in self.completed.values() {
            env.validate()?;
        }
        if let Some(t) = &self.terminal {
            t.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResultEnvelopeV1 {
    Ok {
        output: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_imprint: Option<String>,
    },
    Err {
        code: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<serde_json::Value>,
    },
    Cancelled {
        reason: String,
    },
}

impl ResultEnvelopeV1 {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Err { code, message, .. } => {
                if code.trim().is_empty() || message.trim().is_empty() {
                    return Err("error code and message required".into());
                }
            }
            Self::Cancelled { reason } => {
                if reason.trim().is_empty() {
                    return Err("cancel reason required".into());
                }
            }
            Self::Ok { .. } => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn harness_contract_round_trip_and_hash_stable() {
        let c = HarnessContractV1 {
            id: "workflow.average".into(),
            version: "1.0.0".into(),
            artifact_hash: None,
            origin: ArtifactOriginV1::Vendor,
            description: "Average a number list via sum/count/divide".into(),
            input_imprint: "aelio.math.number_list@1".into(),
            output_imprint: "aelio.math.number@1".into(),
            errors: vec!["Math.EmptyInput".into(), "Math.DivideByZero".into()],
            effect: EffectClassV1::Pure,
            determinism: DeterminismV1::Deterministic,
            suspendability: SuspendabilityV1::Never,
            children_allow: vec![
                "math.sum@^1".into(),
                "collection.count@^1".into(),
                "math.divide@^1".into(),
            ],
            budgets: BudgetContractV1 {
                steps: Some(100),
                calls: Some(3),
                wall_ms: Some(1000),
                depth: Some(4),
                fanout: Some(4),
            },
            program: json!({"nid":"root","op":"Const","v":{}}),
            authoring_source: None,
            tags: vec!["math".into(), "p0".into()],
            required_capabilities: vec![],
        };
        c.validate().unwrap();
        let h1 = c.content_hash();
        let h2 = c.content_hash();
        assert_eq!(h1, h2);
        assert!(h1.starts_with("blake3:"));
        let json = serde_json::to_string(&c).unwrap();
        let back: HarnessContractV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "workflow.average");
        assert_eq!(back.content_hash(), h1);
    }

    #[test]
    fn conductor_spawn_many_validates_unique_nids() {
        let action = ConductorActionV1::SpawnMany {
            children: vec![
                SpawnChildV1 {
                    nid: "sum".into(),
                    harness_pin: VersionPinV1 {
                        id: "math.sum".into(),
                        version: "1.0.0".into(),
                        artifact_hash: None,
                    },
                    input: json!({"list":[1,2]}),
                },
                SpawnChildV1 {
                    nid: "count".into(),
                    harness_pin: VersionPinV1 {
                        id: "collection.count".into(),
                        version: "1.0.0".into(),
                        artifact_hash: None,
                    },
                    input: json!({"list":[1,2]}),
                },
            ],
            join_policy: JoinPolicyV1::All,
            merge_plan: None,
        };
        action.validate().unwrap();
    }

    #[test]
    fn event_requires_namespaced_type() {
        let mut ev = NormalizedEventV1 {
            event_id: "e1".into(),
            event_type: "bad".into(),
            tenant_id: "t".into(),
            subject_id: "u".into(),
            channel: "web".into(),
            occurred_at: "2026-08-05T00:00:00Z".into(),
            causation_id: None,
            correlation_id: None,
            payload: json!({"text":"hi"}),
            identity: EventIdentityV1::default(),
            delivery: EventDeliveryV1::default(),
            metadata: BTreeMap::new(),
        };
        assert!(ev.validate().is_err());
        ev.event_type = "user.message".into();
        ev.validate().unwrap();
    }
}
