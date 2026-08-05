//! Filesystem library install / hash (Phase 1.6–1.7 skeleton).
//!
//! Reads `aelio-os/library/manifest.json` and installs harness `contract.json`
//! bodies into the Sol harness contract table with content-hash metadata.

use crate::os_contract::{HarnessContractV1, ArtifactOriginV1};
use crate::sol_harness_lib::{SolHarnessContract, CONTRACT_TABLE};
use aelio_sol::SolValue;
use aelio_store::{PutIfAbsent, Store, StoreError};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct InstallEntry {
    pub id: String,
    pub version: String,
    pub content_hash: String,
    pub outcome: InstallOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    Inserted,
    AlreadyPresent { existing_hash: Option<String> },
    HashMismatch { existing_hash: String },
}

#[derive(Debug, Clone, Default)]
pub struct InstallReport {
    pub entries: Vec<InstallEntry>,
}

/// Resolve library root from common locations relative to CARGO_MANIFEST_DIR or cwd.
pub fn resolve_library_root(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        if p.join("manifest.json").exists() {
            return Some(p.to_path_buf());
        }
    }
    let candidates = [
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../library"),
        PathBuf::from("library"),
        PathBuf::from("aelio-os/library"),
    ];
    candidates.into_iter().find(|c| c.join("manifest.json").exists())
}

/// Install all harnesses listed in manifest.json into the store.
pub fn install_from_manifest(
    store: &mut dyn Store,
    tenant: &str,
    library_root: &Path,
) -> Result<InstallReport, StoreError> {
    let manifest_path = library_root.join("manifest.json");
    let text = fs::read_to_string(&manifest_path)
        .map_err(|e| StoreError::Internal(format!("read manifest: {e}")))?;
    let manifest: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| StoreError::Internal(format!("parse manifest: {e}")))?;
    let harnesses = manifest
        .get("harnesses")
        .and_then(|v| v.as_array())
        .ok_or_else(|| StoreError::Internal("manifest.harnesses required".into()))?;

    let mut report = InstallReport::default();
    for h in harnesses {
        let id = h
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| StoreError::Internal("harness.id required".into()))?;
        let version = h
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("0.0.0");
        let rel = h
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| StoreError::Internal(format!("harness {id} missing path")))?;
        let contract_path = library_root.join(rel).join("contract.json");
        let program_path = library_root.join(rel).join("program.sol.json");

        let (program_json, content_hash, description) = if contract_path.exists() {
            let ctext = fs::read_to_string(&contract_path)
                .map_err(|e| StoreError::Internal(format!("read {}: {e}", contract_path.display())))?;
            let contract: HarnessContractV1 = serde_json::from_str(&ctext)
                .map_err(|e| StoreError::Internal(format!("parse contract {id}: {e}")))?;
            contract
                .validate()
                .map_err(|e| StoreError::Internal(format!("invalid contract {id}: {e}")))?;
            let hash = contract.content_hash();
            let prog = serde_json::to_string(&contract.program)
                .map_err(|e| StoreError::Internal(e.to_string()))?;
            (prog, hash, contract.description)
        } else if program_path.exists() {
            let prog = fs::read_to_string(&program_path)
                .map_err(|e| StoreError::Internal(format!("read {}: {e}", program_path.display())))?;
            // Validate compiles later by caller; hash the program text.
            let hash = format!("blake3:{}", blake3::hash(prog.as_bytes()).to_hex());
            (prog, hash, format!("installed from {}", rel))
        } else {
            return Err(StoreError::Internal(format!(
                "no contract.json or program.sol.json under {}",
                library_root.join(rel).display()
            )));
        };

        let store_value = SolValue::map([
            ("id", SolValue::str(id)),
            ("kind", SolValue::str("sol_harness")),
            ("summary", SolValue::str(&description)),
            ("program_json", SolValue::str(&program_json)),
            ("version", SolValue::str(version)),
            ("content_hash", SolValue::str(&content_hash)),
            (
                "origin",
                SolValue::str(match ArtifactOriginV1::Vendor {
                    ArtifactOriginV1::Vendor => "vendor",
                    ArtifactOriginV1::Tenant => "tenant",
                    ArtifactOriginV1::Learned => "learned",
                }),
            ),
        ]);

        let outcome = match store.put_if_absent(tenant, CONTRACT_TABLE, id, store_value)? {
            PutIfAbsent::Inserted { .. } => InstallOutcome::Inserted,
            PutIfAbsent::Existing(row) => {
                let existing_hash = row
                    .value
                    .as_map()
                    .and_then(|m| m.get("content_hash"))
                    .and_then(|v| match v {
                        SolValue::Str(s) => Some(s.clone()),
                        _ => None,
                    });
                match &existing_hash {
                    Some(h) if h == &content_hash => InstallOutcome::AlreadyPresent {
                        existing_hash: existing_hash.clone(),
                    },
                    Some(h) => InstallOutcome::HashMismatch {
                        existing_hash: h.clone(),
                    },
                    None => InstallOutcome::AlreadyPresent {
                        existing_hash: None,
                    },
                }
            }
        };

        report.entries.push(InstallEntry {
            id: id.into(),
            version: version.into(),
            content_hash,
            outcome,
        });
    }
    Ok(report)
}

/// Load a stored contract including optional content_hash field.
pub fn load_installed_hash(
    store: &dyn Store,
    tenant: &str,
    id: &str,
) -> Result<Option<String>, StoreError> {
    match store.get(tenant, CONTRACT_TABLE, id)? {
        None => Ok(None),
        Some(row) => Ok(row.value.as_map().and_then(|m| {
            m.get("content_hash").and_then(|v| match v {
                SolValue::Str(s) => Some(s.clone()),
                _ => None,
            })
        })),
    }
}

/// Export in-memory seed library contracts into SolHarnessContract store shape (no FS).
pub fn install_seed_sol_library(
    store: &mut dyn Store,
    tenant: &str,
) -> Result<usize, StoreError> {
    let mut n = 0;
    for c in crate::sol_harness_library() {
        let _ = store.put_if_absent(tenant, CONTRACT_TABLE, &c.id, c.as_store_value())?;
        n += 1;
    }
    Ok(n)
}

pub fn sol_contract_from_harness_file(
    contract_path: &Path,
) -> Result<SolHarnessContract, StoreError> {
    let text = fs::read_to_string(contract_path)
        .map_err(|e| StoreError::Internal(format!("read: {e}")))?;
    let contract: HarnessContractV1 = serde_json::from_str(&text)
        .map_err(|e| StoreError::Internal(format!("parse: {e}")))?;
    contract
        .validate()
        .map_err(|e| StoreError::Internal(e))?;
    Ok(SolHarnessContract::seed(
        contract.id,
        contract.description,
        serde_json::to_string(&contract.program)
            .map_err(|e| StoreError::Internal(e.to_string()))?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aelio_store::MemoryStore;

    #[test]
    fn install_manifest_idempotent_and_hashed() {
        let root = resolve_library_root(None).expect("library root");
        let mut store = MemoryStore::new();
        let report = install_from_manifest(&mut store, "tenant-lib", &root).unwrap();
        assert!(
            report.entries.iter().any(|e| e.id == "workflow.average"),
            "expected workflow.average in install"
        );
        assert!(
            report.entries.iter().any(|e| e.id == "conductor.root"),
            "expected conductor.root in install"
        );
        assert!(report
            .entries
            .iter()
            .all(|e| matches!(e.outcome, InstallOutcome::Inserted)));

        let report2 = install_from_manifest(&mut store, "tenant-lib", &root).unwrap();
        assert!(report2
            .entries
            .iter()
            .all(|e| matches!(e.outcome, InstallOutcome::AlreadyPresent { .. })));

        let hash = load_installed_hash(&store, "tenant-lib", "workflow.average")
            .unwrap()
            .expect("hash present");
        assert!(hash.starts_with("blake3:"));
    }
}
