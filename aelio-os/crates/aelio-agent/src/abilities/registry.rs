//! Registry.* — tool/flow/procedure lookup. Resolution, not decision.

use crate::contract::{AbilityContract, AbilityPath};
use crate::tenant::{FlowSpec, ToolSpec};
use crate::types::{AelioError, AelioResult, ReasonCode};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

static REGISTRATION_REJECTIONS_TOTAL: AtomicU64 = AtomicU64::new(0);
static PROMOTION_BLOCKED_COMPLETENESS_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Named D3 admission/promotion counters. They intentionally count refusals: a permanently
/// zero count while catalog authors are migrating would mean the gates are not on the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContractMetrics {
    pub registration_rejections_total: u64,
    pub promotion_blocked_completeness_total: u64,
}

pub fn contract_metrics() -> ContractMetrics {
    ContractMetrics {
        registration_rejections_total: REGISTRATION_REJECTIONS_TOTAL.load(Ordering::Relaxed),
        promotion_blocked_completeness_total: PROMOTION_BLOCKED_COMPLETENESS_TOTAL
            .load(Ordering::Relaxed),
    }
}

pub(crate) fn record_promotion_completeness_block() {
    PROMOTION_BLOCKED_COMPLETENESS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcedureSpec {
    pub id: String,
    pub version: String,
    pub tenant_id: String,
    /// Canonical situation key hash for tier-0 exact match.
    pub situation_hash: String,
    pub situation_filter: SituationFilter,
    /// Near-situation embedding for tier-1 scored vector kNN (excludes turn/seen buckets).
    #[serde(default)]
    pub situation_embedding: Vec<f32>,
    pub path: AbilityPath,
    pub contract: AbilityContract,
    pub tool_deps: Vec<String>,
    #[serde(default)]
    pub prompt_deps: Vec<String>,
    pub evidence: ProcedureEvidence,
    pub status: ProcedureStatus,
    pub provenance: ProcedureProvenance,
    pub supersedes: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SituationFilter {
    pub state: Option<String>,
    pub intent_class: Option<String>,
    pub required_slots: Vec<String>,
    pub capability_tags: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcedureEvidence {
    pub observations: u64,
    pub success_rate: f64,
    pub mean_cost: f64,
    pub mean_latency_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureStatus {
    Proposed,
    Candidate,
    Promoted,
    Suspended,
    Retired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcedureProvenance {
    pub origin: String,
    pub proposed_by: String,
    pub approved_by: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct Registry {
    pub tools: IndexMap<String, ToolSpec>,
    pub flows: IndexMap<String, FlowSpec>,
    /// Authored flow id -> exact runtime-owned executable artifact.
    pub flow_artifacts: IndexMap<String, crate::adaptive::ArtifactPinV1>,
    pub procedures: IndexMap<String, ProcedureSpec>,
    /// Legacy procedure id -> exact unified runtime artifact. Absence means the legacy path may be
    /// shadowed but cannot be emitted as an authoritative adaptive invocation.
    pub procedure_artifacts: IndexMap<String, crate::adaptive::ArtifactPinV1>,
    pub abilities: IndexMap<String, AbilityContract>,
    /// capability_tag → tool ids
    pub by_capability: IndexMap<String, Vec<String>>,
}

/// Reserved id prefix for Aelio's first-party standard tools. Only the `std.` prefix distinguishes
/// a tool the runtime executes locally from one it dispatches over the tenant's SDK, so the
/// namespace has to be closed to tenants.
pub const STANDARD_TOOL_PREFIX: &str = "std.";

/// Deep content equality for `ToolSpec` via canonical JSON comparison rather than a derived
/// `PartialEq` — `ToolSpec` nests `Value`/`Sensitivity`/etc. that this stays decoupled from.
/// Only used to decide whether a re-registration is a real contract change worth cascading.
fn tool_spec_content_eq(a: &ToolSpec, b: &ToolSpec) -> bool {
    match (serde_json::to_value(a), serde_json::to_value(b)) {
        (Ok(a), Ok(b)) => a == b,
        // Unable to compare — fail open toward invalidation rather than silently keeping a
        // possibly-stale cascade.
        _ => false,
    }
}

impl Registry {
    /// Registers a tool. `invalidate_tool` (cascade suspension of dependent procedures/abilities)
    /// existed but was never called from anywhere except a test — see `FLAGS.md` F-032. A tool
    /// re-registration under the same id with a changed declared contract is exactly the case
    /// that cascade exists for (a tenant SDK-side deploy changing params, effect class, or
    /// output semantics out from under procedures already promoted against the old contract), so
    /// it now runs synchronously here. Returns the ids of everything suspended/flagged as a
    /// result — empty on a fresh registration or a byte-identical re-registration.
    pub fn register_tool(&mut self, tool: ToolSpec) -> AelioResult<Vec<String>> {
        if tool.contract.is_none() {
            REGISTRATION_REJECTIONS_TOTAL.fetch_add(1, Ordering::Relaxed);
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!("tool `{}` is missing its required ToolContract", tool.id),
            ));
        }
        let content_changed = self
            .tools
            .get(&tool.id)
            .map(|existing| !tool_spec_content_eq(existing, &tool))
            .unwrap_or(false);
        for tag in &tool.capability_tags {
            self.by_capability
                .entry(tag.clone())
                .or_default()
                .push(tool.id.clone());
        }
        let tool_id = tool.id.clone();
        self.tools.insert(tool_id.clone(), tool);
        let demoted = if content_changed {
            self.invalidate_tool(&tool_id)
        } else {
            Vec::new()
        };
        Ok(demoted)
    }

    /// Register a tenant-authored tool. Rejects the reserved `std.` namespace: a tenant tool
    /// named `std.anything` would be silently executed in-process instead of over their SDK,
    /// and would shadow a first-party tool of the same id. Returns the ids invalidated by a
    /// content-changing re-registration, same as `register_tool`.
    pub fn register_tenant_tool(&mut self, tool: ToolSpec) -> AelioResult<Vec<String>> {
        if tool.id.starts_with(STANDARD_TOOL_PREFIX) {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!(
                    "tool id `{}` uses the reserved `{STANDARD_TOOL_PREFIX}` namespace",
                    tool.id
                ),
            ));
        }
        self.register_tool(tool)
    }

    pub fn register_flow(&mut self, flow: FlowSpec) {
        self.flows.insert(flow.id.clone(), flow);
    }

    pub fn register_procedure(&mut self, proc: ProcedureSpec) {
        // Never fabricate a missing vector with an unrelated default embedding space. An empty
        // vector remains eligible for Tier-0 exact lookup and is deliberately excluded from
        // Tier-1 until it is re-baked with the runtime's configured embedder.
        self.procedures.insert(proc.id.clone(), proc);
    }

    pub fn bind_flow_artifact(
        &mut self,
        flow_id: &str,
        artifact: crate::adaptive::ArtifactPinV1,
    ) -> AelioResult<()> {
        if !self.flows.contains_key(flow_id) {
            return Err(AelioError::new(
                ReasonCode::NotFound,
                "cannot bind an artifact to an unknown authored flow",
            ));
        }
        artifact.validate()?;
        if let Some(existing) = self.flow_artifacts.get(flow_id) {
            if existing == &artifact {
                return Ok(());
            }
            return Err(AelioError::new(
                ReasonCode::Conflict,
                "authored flow already has a different immutable artifact binding",
            ));
        }
        self.flow_artifacts.insert(flow_id.into(), artifact);
        Ok(())
    }

    pub fn flow_artifact(&self, flow_id: &str) -> Option<&crate::adaptive::ArtifactPinV1> {
        self.flow_artifacts.get(flow_id)
    }

    pub fn bind_procedure_artifact(
        &mut self,
        procedure_id: &str,
        artifact: crate::adaptive::ArtifactPinV1,
    ) -> AelioResult<()> {
        if !self.procedures.contains_key(procedure_id) {
            return Err(AelioError::new(
                ReasonCode::NotFound,
                "cannot bind an artifact to an unknown procedure",
            ));
        }
        artifact.validate()?;
        if let Some(existing) = self.procedure_artifacts.get(procedure_id) {
            if existing == &artifact {
                return Ok(());
            }
            return Err(AelioError::new(
                ReasonCode::Conflict,
                "procedure already has a different immutable artifact binding",
            ));
        }
        self.procedure_artifacts
            .insert(procedure_id.to_owned(), artifact);
        Ok(())
    }

    pub fn procedure_artifact(
        &self,
        procedure_id: &str,
    ) -> Option<&crate::adaptive::ArtifactPinV1> {
        self.procedure_artifacts.get(procedure_id)
    }

    pub fn register_ability(&mut self, contract: AbilityContract) {
        self.abilities.insert(contract.id.clone(), contract);
    }

    /// Lookup by capability tag — RESOLVES, does not decide.
    pub fn lookup_tool_by_capability(&self, capability: &str) -> Vec<&ToolSpec> {
        let mut out = Vec::new();
        // exact
        if let Some(ids) = self.by_capability.get(capability) {
            for id in ids {
                if let Some(t) = self.tools.get(id) {
                    out.push(t);
                }
            }
        }
        // prefix / glob-ish: auth.otp.*
        if capability.ends_with(".*") || capability.ends_with('*') {
            let prefix = capability.trim_end_matches('*').trim_end_matches('.');
            for (tag, ids) in &self.by_capability {
                if tag.starts_with(prefix) {
                    for id in ids {
                        if let Some(t) = self.tools.get(id) {
                            if !out.iter().any(|x| x.id == t.id) {
                                out.push(t);
                            }
                        }
                    }
                }
            }
        }
        out
    }

    pub fn lookup_tool(&self, id: &str) -> AelioResult<&ToolSpec> {
        self.tools
            .get(id)
            .ok_or_else(|| AelioError::new(ReasonCode::NotFound, format!("tool {id}")))
    }

    pub fn lookup_flow(&self, id: &str) -> AelioResult<&FlowSpec> {
        self.flows
            .get(id)
            .ok_or_else(|| AelioError::new(ReasonCode::NotFound, format!("flow {id}")))
    }

    pub fn capabilities(&self) -> Vec<String> {
        self.by_capability.keys().cloned().collect()
    }

    pub fn reachable_capabilities(&self, state_envelope: &[String]) -> Vec<String> {
        if state_envelope.is_empty() {
            return self.capabilities();
        }
        state_envelope.to_vec()
    }

    /// Cascade invalidation on tool change.
    pub fn invalidate_tool(&mut self, tool_id: &str) -> Vec<String> {
        let mut demoted = Vec::new();
        for proc in self.procedures.values_mut() {
            if proc.tool_deps.iter().any(|d| d == tool_id) {
                proc.status = ProcedureStatus::Suspended;
                demoted.push(proc.id.clone());
            }
        }
        for ab in self.abilities.values_mut() {
            if ab.tool_deps.iter().any(|d| d == tool_id) {
                demoted.push(ab.id.clone());
            }
        }
        demoted
    }

    /// Cascade invalidation on prompt change. A prompt edit changes an ability's behavior, so it
    /// must demote every procedure that was learned against that prompt exactly like a tool change
    /// does (decisions C6/N4). Without this the tool cascade is only half the invalidation surface:
    /// thresholds and procedures silently stale after a prompt edit.
    pub fn invalidate_prompt(&mut self, prompt_hash: &str) -> Vec<String> {
        let mut demoted = Vec::new();
        for proc in self.procedures.values_mut() {
            if proc.prompt_deps.iter().any(|hash| hash == prompt_hash)
                || proc.contract.prompt_hash.as_deref() == Some(prompt_hash)
            {
                proc.status = ProcedureStatus::Suspended;
                demoted.push(proc.id.clone());
            }
        }
        for ab in self.abilities.values_mut() {
            if ab.prompt_hash.as_deref() == Some(prompt_hash) {
                demoted.push(ab.id.clone());
            }
        }
        demoted
    }

    pub fn lookup_procedure_by_hash(&self, situation_hash: &str) -> Option<&ProcedureSpec> {
        self.procedures
            .values()
            .filter(|p| p.situation_hash == situation_hash && p.status == ProcedureStatus::Promoted)
            .max_by(|a, b| {
                a.evidence
                    .success_rate
                    .partial_cmp(&b.evidence.success_rate)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Sensitivity;

    #[test]
    fn capability_lookup_resolves() {
        let mut reg = Registry::default();
        reg.register_tool(ToolSpec {
            id: "send_otp".into(),
            name: "send_otp".into(),
            version: "1".into(),
            capability_tags: vec!["auth.otp.send".into()],
            contract: Some(crate::tenant::ToolContract::complete_read("test_result")),
            effect: None,
            effectful: true,
            idempotent: false,
            dry_run_available: true,
            params: vec![],
            output_semantics: crate::tenant::OutputSpec {
                fields: IndexMap::new(),
                role_hint: None,
            },
            continuations: vec!["auth.otp.verify".into()],
            errors: vec![],
        })
        .unwrap();
        let hits = reg.lookup_tool_by_capability("auth.otp.*");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "send_otp");
        assert!(reg
            .lookup_tool_by_capability("auth.otp.send.unrelated")
            .is_empty());
        // sensitivity unused but kept for compile
        let _ = Sensitivity::None;
    }

    /// D7: absent contract fields are a hard registration refusal, never a legacy inference.
    #[test]
    fn registration_refuses_a_tool_without_a_contract() {
        let mut reg = Registry::default();
        let error = reg
            .register_tool(ToolSpec {
                id: "missing_contract".into(),
                name: "missing_contract".into(),
                version: "1".into(),
                capability_tags: vec![],
                contract: None,
                effect: None,
                effectful: false,
                idempotent: true,
                dry_run_available: true,
                params: vec![],
                output_semantics: crate::tenant::OutputSpec {
                    fields: IndexMap::new(),
                    role_hint: None,
                },
                continuations: vec![],
                errors: vec![],
            })
            .expect_err("D7 requires a ToolContract");
        assert_eq!(error.code, ReasonCode::Validation);
        assert!(error.message.contains("ToolContract"));
        assert_eq!(reg.tools.len(), 0);
        assert!(contract_metrics().registration_rejections_total >= 1);
    }

    /// The `std.` prefix is the only thing distinguishing a locally-executed first-party tool
    /// from one dispatched over the tenant's SDK, so tenants must not be able to claim it.
    #[test]
    fn tenant_tool_cannot_claim_the_std_namespace() {
        let mut reg = Registry::default();
        let spec = ToolSpec {
            id: "std.math.average".into(),
            name: "hijack".into(),
            version: "1".into(),
            capability_tags: vec!["math".into()],
            contract: Some(crate::tenant::ToolContract::complete_read("test_result")),
            effect: None,
            effectful: true,
            idempotent: false,
            dry_run_available: false,
            params: vec![],
            output_semantics: crate::tenant::OutputSpec {
                fields: IndexMap::new(),
                role_hint: None,
            },
            continuations: vec![],
            errors: vec![],
        };
        let error = reg
            .register_tenant_tool(spec)
            .expect_err("reserved namespace must be rejected");
        assert_eq!(error.code, ReasonCode::Validation);
        assert!(reg.tools.is_empty(), "rejected tool must not be registered");
    }

    fn otp_tool(version: &str, capability_tags: Vec<String>) -> ToolSpec {
        ToolSpec {
            id: "send_otp".into(),
            name: "send_otp".into(),
            version: version.into(),
            capability_tags,
            contract: Some(crate::tenant::ToolContract::complete_read("test_result")),
            effect: None,
            effectful: true,
            idempotent: false,
            dry_run_available: true,
            params: vec![],
            output_semantics: crate::tenant::OutputSpec {
                fields: IndexMap::new(),
                role_hint: None,
            },
            continuations: vec!["auth.otp.verify".into()],
            errors: vec![],
        }
    }

    /// F-032: `invalidate_tool`'s cascade existed but was never wired to registration itself, so
    /// a routine tenant SDK-side redeploy of a tool's contract could leave stale promoted
    /// procedures pointing at a superseded contract until something else happened to call
    /// `invalidate_tool` by hand. Registration must trigger the cascade synchronously.
    #[test]
    fn content_changing_reregistration_synchronously_invalidates_dependents() {
        let mut reg = Registry::default();
        let demoted = reg
            .register_tool(otp_tool("1", vec!["auth.otp.send".into()]))
            .expect("contracted tool registers");
        assert!(demoted.is_empty(), "fresh registration invalidates nothing");

        reg.register_procedure(ProcedureSpec {
            id: "otp_proc".into(),
            version: "1".into(),
            tenant_id: "t".into(),
            situation_hash: "sigma".into(),
            situation_filter: SituationFilter::default(),
            situation_embedding: vec![],
            path: AbilityPath::seq(["send_otp"]),
            contract: AbilityContract::pure("otp_proc"),
            tool_deps: vec!["send_otp".into()],
            prompt_deps: vec![],
            evidence: ProcedureEvidence::default(),
            status: ProcedureStatus::Promoted,
            provenance: ProcedureProvenance {
                origin: "test".into(),
                proposed_by: "test".into(),
                approved_by: None,
            },
            supersedes: None,
        });

        // Byte-identical re-registration must not cascade — nothing about the contract changed.
        let demoted = reg
            .register_tool(otp_tool("1", vec!["auth.otp.send".into()]))
            .expect("contracted tool registers");
        assert!(
            demoted.is_empty(),
            "identical re-registration must not cascade"
        );
        assert_eq!(
            reg.procedures.get("otp_proc").unwrap().status,
            ProcedureStatus::Promoted
        );

        // Version bump (a real contract change) must cascade synchronously, with no separate
        // manual `invalidate_tool` call required.
        let demoted = reg
            .register_tool(otp_tool("2", vec!["auth.otp.send".into()]))
            .expect("contracted tool registers");
        assert_eq!(demoted, vec!["otp_proc".to_string()]);
        assert_eq!(
            reg.procedures.get("otp_proc").unwrap().status,
            ProcedureStatus::Suspended
        );
    }

    #[test]
    fn prompt_change_cascades_like_tool_change() {
        let mut reg = Registry::default();
        let mut contract = AbilityContract::pure("proc1");
        contract.prompt_hash = Some("hash_v1".into());
        contract.postconditions = vec![];
        reg.register_procedure(ProcedureSpec {
            id: "proc1".into(),
            version: "1".into(),
            tenant_id: "t".into(),
            situation_hash: "sigma".into(),
            situation_filter: SituationFilter::default(),
            situation_embedding: vec![],
            path: AbilityPath::seq(["Express.Synthesize"]),
            contract,
            tool_deps: vec![],
            prompt_deps: vec!["hash_v1".into()],
            evidence: ProcedureEvidence::default(),
            status: ProcedureStatus::Promoted,
            provenance: ProcedureProvenance {
                origin: "test".into(),
                proposed_by: "test".into(),
                approved_by: None,
            },
            supersedes: None,
        });

        // Unrelated prompt edit leaves it promoted.
        assert!(reg.invalidate_prompt("hash_other").is_empty());
        assert_eq!(
            reg.procedures.get("proc1").unwrap().status,
            ProcedureStatus::Promoted
        );

        // The prompt it was learned against changes → it is demoted, symmetric with tool_deps.
        let demoted = reg.invalidate_prompt("hash_v1");
        assert_eq!(demoted, vec!["proc1".to_string()]);
        assert_eq!(
            reg.procedures.get("proc1").unwrap().status,
            ProcedureStatus::Suspended
        );
    }
}
