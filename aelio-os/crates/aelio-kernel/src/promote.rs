//! Phase 5.5 / 7.1 — harness draft → sandbox → admin promote (gated).
//!
//! Live chat must never treat an untested draft as installed law.
//! Promotion requires: validate contract → compile Sol → sandbox execute → admin flag.

use crate::compile;
use crate::driver::{Instance, TurnOutcome};
use crate::error::{ErrV1, ReasonCode};
use crate::os_contract::{ArtifactOriginV1, HarnessContractV1};
use crate::registry::{EffectClass, Registry};
use crate::sol_harness_lib::{SolHarnessContract, CONTRACT_TABLE};
use crate::stdlib_targets::registry_with_p0_stdlib;
use aelio_sol::SolValue;
use aelio_store::{PutIfAbsent, Store, StoreError};
use serde::{Deserialize, Serialize};

pub const DRAFT_TABLE: &str = "harness_drafts_v1";
pub const PROMOTE_AUDIT_TABLE: &str = "harness_promote_audit_v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftStatus {
    Draft,
    SandboxPassed,
    SandboxFailed,
    Promoted,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessDraftV1 {
    pub id: String,
    pub version: String,
    pub status: DraftStatus,
    pub contract: HarnessContractV1,
    #[serde(default)]
    pub sandbox_notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PromoteReport {
    pub id: String,
    pub content_hash: String,
    pub outcome: PromoteOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromoteOutcome {
    Promoted,
    AlreadyPresent,
    Rejected { reason: String },
}

fn encode_draft(d: &HarnessDraftV1) -> Result<SolValue, StoreError> {
    let json = serde_json::to_string(d)
        .map_err(|e| StoreError::Internal(format!("draft serialize: {e}")))?;
    Ok(SolValue::map([
        ("kind", SolValue::str("harness_draft_v1")),
        ("json", SolValue::str(json)),
        ("status", SolValue::str(format!("{:?}", d.status).to_lowercase())),
        ("id", SolValue::str(&d.id)),
    ]))
}

fn decode_draft(v: &SolValue) -> Result<HarnessDraftV1, StoreError> {
    let map = v
        .as_map()
        .ok_or_else(|| StoreError::Internal("draft envelope map".into()))?;
    let json = map
        .get("json")
        .and_then(|x| match x {
            SolValue::Str(s) => Some(s.as_str()),
            _ => None,
        })
        .ok_or_else(|| StoreError::Internal("draft missing json".into()))?;
    serde_json::from_str(json).map_err(|e| StoreError::Internal(format!("draft decode: {e}")))
}

/// Save a draft (put-if-absent by id@version).
pub fn save_draft(
    store: &mut dyn Store,
    tenant: &str,
    mut draft: HarnessDraftV1,
) -> Result<PutIfAbsent, StoreError> {
    draft
        .contract
        .validate()
        .map_err(|e| StoreError::Internal(e))?;
    if draft.id != draft.contract.id {
        return Err(StoreError::Internal(
            "draft.id must match contract.id".into(),
        ));
    }
    draft.content_hash = Some(draft.contract.content_hash());
    draft.status = DraftStatus::Draft;
    let key = format!("{}@{}", draft.id, draft.version);
    store.put_if_absent(tenant, DRAFT_TABLE, &key, encode_draft(&draft)?)
}

/// Compile + plan + execute sandbox vectors against leaf registry.
pub fn sandbox_draft(
    store: &mut dyn Store,
    tenant: &str,
    id: &str,
    version: &str,
    vectors: &[(SolValue, /* expect_field */ &str)],
) -> Result<HarnessDraftV1, StoreError> {
    let key = format!("{id}@{version}");
    let row = store
        .get(tenant, DRAFT_TABLE, &key)?
        .ok_or(StoreError::NotFound)?;
    let mut draft = decode_draft(&row.value)?;
    draft.sandbox_notes.clear();

    if let Err(e) = draft.contract.validate() {
        draft.status = DraftStatus::SandboxFailed;
        draft.sandbox_notes.push(format!("validate: {e}"));
        upsert_draft(store, tenant, &key, &draft, row.version)?;
        return Ok(draft);
    }

    let program_json = serde_json::to_string(&draft.contract.program)
        .map_err(|e| StoreError::Internal(e.to_string()))?;
    let program = match compile(&program_json) {
        Ok(p) => p,
        Err(e) => {
            draft.status = DraftStatus::SandboxFailed;
            draft.sandbox_notes.push(format!("compile: {}", e.detail));
            upsert_draft(store, tenant, &key, &draft, row.version)?;
            return Ok(draft);
        }
    };

    let mut registry = sandbox_registry();
    let mut ok = true;
    if vectors.is_empty() {
        // Empty bag smoke run.
        let mut inst = Instance::new(program.clone(), &mut registry);
        match inst.start(SolValue::Null) {
            Ok(TurnOutcome::Completed { .. }) | Ok(TurnOutcome::Parked(_)) => {
                draft.sandbox_notes.push("smoke: completed or parked".into());
            }
            Err(e) => {
                ok = false;
                draft.sandbox_notes.push(format!("smoke err: {}", e.detail));
            }
        }
    } else {
        for (i, (bag, expect_field)) in vectors.iter().enumerate() {
            let mut reg = sandbox_registry();
            let mut inst = Instance::new(program.clone(), &mut reg);
            match inst.start(bag.clone()) {
                Ok(TurnOutcome::Completed { bag: out, .. }) => {
                    let has = out
                        .as_map()
                        .is_some_and(|m| m.contains_key(*expect_field) || expect_field.is_empty());
                    if !has && !expect_field.is_empty() {
                        ok = false;
                        draft
                            .sandbox_notes
                            .push(format!("vector[{i}]: missing field `{expect_field}`"));
                    } else {
                        draft.sandbox_notes.push(format!("vector[{i}]: ok"));
                    }
                }
                Ok(TurnOutcome::Parked(_)) => {
                    draft.sandbox_notes.push(format!("vector[{i}]: parked ok"));
                }
                Err(e) => {
                    ok = false;
                    draft
                        .sandbox_notes
                        .push(format!("vector[{i}]: {}", e.detail));
                }
            }
        }
    }

    draft.status = if ok {
        DraftStatus::SandboxPassed
    } else {
        DraftStatus::SandboxFailed
    };
    upsert_draft(store, tenant, &key, &draft, row.version)?;
    Ok(draft)
}

/// Admin-only promote: draft must be SandboxPassed; installs into CONTRACT_TABLE.
pub fn admin_promote(
    store: &mut dyn Store,
    tenant: &str,
    id: &str,
    version: &str,
    admin_principal: &str,
) -> Result<PromoteReport, StoreError> {
    if admin_principal.trim().is_empty() {
        return Ok(PromoteReport {
            id: id.into(),
            content_hash: String::new(),
            outcome: PromoteOutcome::Rejected {
                reason: "admin_principal required".into(),
            },
        });
    }
    let key = format!("{id}@{version}");
    let row = store
        .get(tenant, DRAFT_TABLE, &key)?
        .ok_or(StoreError::NotFound)?;
    let mut draft = decode_draft(&row.value)?;
    if draft.status != DraftStatus::SandboxPassed {
        return Ok(PromoteReport {
            id: id.into(),
            content_hash: draft.content_hash.clone().unwrap_or_default(),
            outcome: PromoteOutcome::Rejected {
                reason: format!("draft status {:?} (need sandbox_passed)", draft.status),
            },
        });
    }

    let hash = draft.contract.content_hash();
    let program_json = serde_json::to_string(&draft.contract.program)
        .map_err(|e| StoreError::Internal(e.to_string()))?;
    // Force origin tenant/learned for non-vendor; keep vendor if set.
    let origin = match draft.contract.origin {
        ArtifactOriginV1::Vendor => "vendor",
        ArtifactOriginV1::Tenant => "tenant",
        ArtifactOriginV1::Learned => "learned",
    };
    let store_value = SolValue::map([
        ("id", SolValue::str(&draft.id)),
        ("kind", SolValue::str("sol_harness")),
        ("summary", SolValue::str(&draft.contract.description)),
        ("program_json", SolValue::str(&program_json)),
        ("version", SolValue::str(version)),
        ("content_hash", SolValue::str(&hash)),
        ("origin", SolValue::str(origin)),
        ("promoted_by", SolValue::str(admin_principal)),
    ]);

    let outcome = match store.put_if_absent(tenant, CONTRACT_TABLE, id, store_value)? {
        PutIfAbsent::Inserted { .. } => PromoteOutcome::Promoted,
        PutIfAbsent::Existing(_) => PromoteOutcome::AlreadyPresent,
    };

    draft.status = DraftStatus::Promoted;
    upsert_draft(store, tenant, &key, &draft, row.version)?;

    let audit = SolValue::map([
        ("id", SolValue::str(id)),
        ("version", SolValue::str(version)),
        ("hash", SolValue::str(&hash)),
        ("admin", SolValue::str(admin_principal)),
        (
            "outcome",
            SolValue::str(match &outcome {
                PromoteOutcome::Promoted => "promoted",
                PromoteOutcome::AlreadyPresent => "already_present",
                PromoteOutcome::Rejected { .. } => "rejected",
            }),
        ),
    ]);
    let _ = store.put_if_absent(
        tenant,
        PROMOTE_AUDIT_TABLE,
        &format!("{id}@{version}:{admin_principal}"),
        audit,
    )?;

    Ok(PromoteReport {
        id: id.into(),
        content_hash: hash,
        outcome,
    })
}

fn upsert_draft(
    store: &mut dyn Store,
    tenant: &str,
    key: &str,
    draft: &HarnessDraftV1,
    expected_version: u64,
) -> Result<(), StoreError> {
    let value = encode_draft(draft)?;
    store.cas(tenant, DRAFT_TABLE, key, expected_version, value)?;
    Ok(())
}

fn sandbox_registry() -> Registry {
    let mut r = registry_with_p0_stdlib();
    // Effect stub for tool harness sandboxes.
    r.register("tool.act_stub@1", EffectClass::External, |_| {
        Ok(SolValue::map([
            ("ok", SolValue::Bool(true)),
            ("tool", SolValue::str("act_stub")),
        ]))
    });
    r.register("tool.send_otp@1", EffectClass::External, |args| {
        let phone = args
            .as_map()
            .and_then(|m| m.get("phone"))
            .cloned()
            .unwrap_or_else(|| SolValue::str(""));
        Ok(SolValue::map([
            ("ok", SolValue::Bool(true)),
            ("phone", phone),
            ("otp_sent", SolValue::Bool(true)),
        ]))
    });
    r.register("memory.search@1", EffectClass::Read, |_| {
        Ok(SolValue::map([("snippet", SolValue::str("prior note"))]))
    });
    r.register("context.attach@1", EffectClass::Write, |args| {
        let snippet = args
            .as_map()
            .and_then(|m| m.get("snippet"))
            .cloned()
            .unwrap_or_else(|| SolValue::str(""));
        Ok(SolValue::map([("note", snippet)]))
    });
    r
}

/// Build a draft from a HarnessContractV1 (helper for authors/tests).
pub fn draft_from_contract(contract: HarnessContractV1) -> Result<HarnessDraftV1, ErrV1> {
    contract
        .validate()
        .map_err(|e| ErrV1::new(ReasonCode::Shape, "draft", e))?;
    let hash = contract.content_hash();
    Ok(HarnessDraftV1 {
        id: contract.id.clone(),
        version: contract.version.clone(),
        status: DraftStatus::Draft,
        contract,
        sandbox_notes: vec![],
        content_hash: Some(hash),
    })
}

/// Load promoted Sol harness program JSON from the live contract table.
pub fn load_promoted_program(
    store: &dyn Store,
    tenant: &str,
    id: &str,
) -> Result<Option<SolHarnessContract>, StoreError> {
    crate::sol_harness_lib::load_contract(store, tenant, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os_contract::{
        BudgetContractV1, DeterminismV1, EffectClassV1, SuspendabilityV1,
    };
    use aelio_store::MemoryStore;
    use serde_json::json;

    fn sample_contract() -> HarnessContractV1 {
        HarnessContractV1 {
            id: "tool.run_once".into(),
            version: "1.0.0".into(),
            artifact_hash: None,
            origin: ArtifactOriginV1::Tenant,
            description: "Run tool.act_stub once under Once".into(),
            input_imprint: "aelio.unit@1".into(),
            output_imprint: "aelio.tool.result@1".into(),
            errors: vec![],
            effect: EffectClassV1::External,
            determinism: DeterminismV1::LedgeredNondeterministic,
            suspendability: SuspendabilityV1::Never,
            children_allow: vec![],
            budgets: BudgetContractV1 {
                steps: Some(20),
                calls: Some(2),
                wall_ms: Some(2000),
                depth: Some(2),
                fanout: Some(1),
            },
            program: json!({
                "nid": "root",
                "op": "Once",
                "body": {
                    "nid": "send",
                    "op": "Call",
                    "id": "tool.act_stub@1",
                    "args": {},
                    "into": "sent"
                }
            }),
            authoring_source: None,
            tags: vec!["tools".into()],
            required_capabilities: vec!["tool.act_stub".into()],
        }
    }

    #[test]
    fn draft_sandbox_promote_happy_path() {
        let mut store = MemoryStore::new();
        let draft = draft_from_contract(sample_contract()).unwrap();
        save_draft(&mut store, "t1", draft).unwrap();
        let sandboxed = sandbox_draft(
            &mut store,
            "t1",
            "tool.run_once",
            "1.0.0",
            &[(SolValue::Null, "sent")],
        )
        .unwrap();
        assert_eq!(sandboxed.status, DraftStatus::SandboxPassed);

        let report = admin_promote(&mut store, "t1", "tool.run_once", "1.0.0", "admin@aelio")
            .unwrap();
        assert_eq!(report.outcome, PromoteOutcome::Promoted);

        let live = load_promoted_program(&store, "t1", "tool.run_once")
            .unwrap()
            .expect("promoted");
        assert_eq!(live.id, "tool.run_once");
        compile(&live.program_json).unwrap();
    }

    #[test]
    fn promote_without_sandbox_rejected() {
        let mut store = MemoryStore::new();
        let draft = draft_from_contract(sample_contract()).unwrap();
        save_draft(&mut store, "t1", draft).unwrap();
        let report = admin_promote(&mut store, "t1", "tool.run_once", "1.0.0", "admin")
            .unwrap();
        assert!(matches!(report.outcome, PromoteOutcome::Rejected { .. }));
    }
}
