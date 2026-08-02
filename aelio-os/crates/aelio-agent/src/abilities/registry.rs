//! Registry.* — tool/flow/procedure lookup. Resolution, not decision.

use crate::contract::{AbilityContract, AbilityPath};
use crate::tenant::{FlowSpec, ToolSpec};
use crate::types::{AelioError, AelioResult, ReasonCode};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

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

impl Registry {
    pub fn register_tool(&mut self, tool: ToolSpec) {
        for tag in &tool.capability_tags {
            self.by_capability
                .entry(tag.clone())
                .or_default()
                .push(tool.id.clone());
        }
        self.tools.insert(tool.id.clone(), tool);
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
        });
        let hits = reg.lookup_tool_by_capability("auth.otp.*");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "send_otp");
        assert!(reg
            .lookup_tool_by_capability("auth.otp.send.unrelated")
            .is_empty());
        // sensitivity unused but kept for compile
        let _ = Sensitivity::None;
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
