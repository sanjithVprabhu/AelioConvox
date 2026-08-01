//! Mint runtime — draft via root axiom, validate App J, store in Aelio DB, recall via Prism.
//!
//! PROVISIONAL (FLAGS F-019): Mint is the prompt factory inside Aelio. Root is human-only.

use crate::{
    artifact_error, Artifact, ArtifactActor, ArtifactClass, ArtifactEffect, ArtifactInput,
    ArtifactInterface, ArtifactPins, ArtifactRepository, ArtifactStatus, ArtifactTier, Provenance,
    RuntimeError,
};
use aelio_db_query::{
    execute_prism, ColumnKind, Database, HashEmbedder, PrismEmbedder, PrismHit, Value,
};
use aelio_prompt::{
    install_root, mint_artifact, root_id, MintDrafter, MintError, MintRequest, MintResult,
    PromptArtifact, TemplateRegistry,
};
use aelio_query::{
    parse_prism, recall, CollectionSchema, PrismColKind, PrismColumn, RecallModality, WhereOp,
    WherePred,
};
use aelio_store::EmbeddedStore;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MINT_TABLE: &str = "minted_prompts";
pub const VENDOR_ARTIFACT_TENANT: &str = "aelio.vendor";
const EMBED_DIM: usize = 8;
const EMBED_MODEL: &str = "hash-8";

/// Durable Mint shelf: unified artifacts plus a tenant-filtered Prism search projection.
pub struct MintShelf {
    dir: PathBuf,
    db: Mutex<Database>,
    artifacts: Mutex<ArtifactRepository<EmbeddedStore>>,
}

impl MintShelf {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, RuntimeError> {
        let dir = dir.as_ref().to_path_buf();
        let artifact_store = EmbeddedStore::open(dir.join("artifact_registry"))
            .map_err(|error| RuntimeError::Store(format!("{error:?}")))?;
        Self::open_with_store(dir, artifact_store)
    }

    pub fn open_with_store(
        dir: impl AsRef<Path>,
        artifact_store: EmbeddedStore,
    ) -> Result<Self, RuntimeError> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir).map_err(|e| RuntimeError::Store(e.to_string()))?;
        let mut db = if dir.join("wal.log").exists() {
            Database::open(&dir).map_err(|e| RuntimeError::Store(e.to_string()))?
        } else {
            Database::create(&dir).map_err(|e| RuntimeError::Store(e.to_string()))?
        };
        ensure_table(&mut db)?;
        let mut registry = TemplateRegistry::default();
        let root = install_root(&mut registry).map_err(mint_err)?;
        let mut artifacts = ArtifactRepository::open(artifact_store).map_err(artifact_error)?;
        artifacts
            .put_proposed(
                VENDOR_ARTIFACT_TENANT,
                prompt_as_artifact(&root, VENDOR_ARTIFACT_TENANT)?,
                ArtifactActor::System,
            )
            .map_err(artifact_error)?;
        // Ensure root row exists (idempotent).
        if find_by_key(&db, VENDOR_ARTIFACT_TENANT, &root.key())?.is_none() {
            insert_artifact(&mut db, &root, VENDOR_ARTIFACT_TENANT)?;
        }
        Ok(Self {
            dir,
            db: Mutex::new(db),
            artifacts: Mutex::new(artifacts),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn store(&self, artifact: &PromptArtifact, tenant: &str) -> Result<(), RuntimeError> {
        if artifact.is_axiom || artifact.key() == root_id() {
            return Err(RuntimeError::Invalid(
                "Mint must not overwrite root in the shelf".into(),
            ));
        }
        aelio_prompt::validate_artifact(artifact).map_err(mint_err)?;
        self.artifacts
            .lock()
            .map_err(|_| RuntimeError::Store("artifact repository lock poisoned".into()))?
            .put_proposed(
                tenant,
                prompt_as_artifact(artifact, tenant)?,
                ArtifactActor::System,
            )
            .map_err(artifact_error)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| RuntimeError::Store("lock".into()))?;
        if find_by_key(&db, tenant, &artifact.key())?.is_some() {
            return Err(RuntimeError::Invalid(format!(
                "artifact `{}` already exists on the shelf",
                artifact.key()
            )));
        }
        insert_artifact(&mut db, artifact, tenant)?;
        Ok(())
    }

    pub fn get(&self, tenant: &str, id_at_v: &str) -> Result<Option<PromptArtifact>, RuntimeError> {
        let db = self
            .db
            .lock()
            .map_err(|_| RuntimeError::Store("lock".into()))?;
        find_by_key(&db, tenant, id_at_v)
    }

    pub fn artifact_status(
        &self,
        tenant: &str,
        id: &str,
        version: u32,
    ) -> Result<Option<ArtifactStatus>, RuntimeError> {
        self.artifacts
            .lock()
            .map_err(|_| RuntimeError::Store("artifact repository lock poisoned".into()))?
            .get(tenant, id, version)
            .map(|record| record.map(|record| record.status))
            .map_err(artifact_error)
    }

    pub fn recall(
        &self,
        tenant: &str,
        need: &str,
        modality: RecallModality,
        limit: u64,
    ) -> Result<Vec<PrismHit>, RuntimeError> {
        let schema = mint_collection_schema();
        let mut q = recall(&schema, need, modality).map_err(RuntimeError::Invalid)?;
        q.where_clauses.push(WherePred {
            col: "tenant".into(),
            op: WhereOp::Eq,
            value: aelio_sol::SolValue::str(tenant),
        });
        let requested = limit.clamp(1, 100) as usize;
        // Over-fetch within the global hard cap, then enforce lifecycle eligibility against the
        // authoritative artifact row. False negatives are safe; cross-tenant/proposed leaks are not.
        q.limit = 100;
        let db = self
            .db
            .lock()
            .map_err(|_| RuntimeError::Store("lock".into()))?;
        let models = models();
        let emb = HashEmbedder { dim: EMBED_DIM };
        let hits = execute_prism(&db, &q, &models, Some(&emb))
            .map_err(|e| RuntimeError::Store(e.to_string()))?;
        drop(db);
        let artifacts = self
            .artifacts
            .lock()
            .map_err(|_| RuntimeError::Store("artifact repository lock poisoned".into()))?;
        let mut eligible = Vec::new();
        for hit in hits {
            let (Some(Value::Utf8(id)), Some(Value::Utf8(version))) =
                (hit.fields.get("id"), hit.fields.get("version"))
            else {
                continue;
            };
            let Ok(version) = version.parse::<u32>() else {
                continue;
            };
            let status = artifacts
                .get(tenant, id, version)
                .map_err(artifact_error)?
                .map(|record| record.status);
            if matches!(
                status,
                Some(ArtifactStatus::Canary | ArtifactStatus::Promoted)
            ) {
                eligible.push(hit);
                if eligible.len() >= requested {
                    break;
                }
            }
        }
        Ok(eligible)
    }

    pub fn get_by_objective_key(
        &self,
        tenant: &str,
        id: &str,
        version: &str,
    ) -> Result<Option<PromptArtifact>, RuntimeError> {
        self.get(tenant, &format!("{id}@{version}"))
    }
}

pub(crate) fn prompt_as_artifact(
    prompt: &PromptArtifact,
    requester: &str,
) -> Result<Artifact, RuntimeError> {
    let version = prompt.version.parse::<u32>().map_err(|_| {
        RuntimeError::Invalid("prompt version must be a positive base-10 artifact version".into())
    })?;
    if version == 0 || prompt.version.starts_with('0') {
        return Err(RuntimeError::Invalid(
            "prompt version must be a positive canonical base-10 artifact version".into(),
        ));
    }
    let inputs = prompt
        .slots
        .iter()
        .map(|slot| ArtifactInput {
            name: slot.name.clone(),
            imprint: format!("aelio.sol.{}@1", slot.ty),
            required: slot.required,
            sensitivity: slot.sensitivity.clone(),
        })
        .collect();
    let mut prompts = prompt.layers.clone();
    if !prompt.root_version.is_empty() {
        prompts.push(format!("aelio.mint.root@{}", prompt.root_version));
    }
    prompts.sort();
    prompts.dedup();
    let examples = prompt
        .exemplars
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
    Artifact::new(
        prompt.id.clone(),
        version,
        ArtifactClass::Prompt,
        if prompt.is_axiom {
            ArtifactTier::Locked
        } else {
            ArtifactTier::Reviewed
        },
        "1",
        env!("CARGO_PKG_VERSION"),
        ArtifactInterface {
            inputs,
            output: prompt.output_imprint.clone(),
        },
        prompt.description.clone(),
        vec!["mint".into()],
        vec![ArtifactEffect::Pure],
        ArtifactPins {
            prompts,
            models: vec![prompt.model.id.clone()],
            ..ArtifactPins::default()
        },
        examples,
        serde_json::json!({"prompt": prompt}),
        Provenance {
            built_by: (!prompt.root_version.is_empty())
                .then(|| format!("aelio.mint.root@{}", prompt.root_version)),
            requester: Some(requester.into()),
            metadata: if requester == VENDOR_ARTIFACT_TENANT {
                serde_json::Map::from_iter([(
                    "origin".into(),
                    serde_json::Value::String("vendor".into()),
                )])
            } else {
                serde_json::Map::new()
            },
            ..Provenance::default()
        },
    )
    .map_err(artifact_error)
}

pub(crate) fn install_vendor_prompt(
    store: aelio_store::EmbeddedStore,
    prompt: &PromptArtifact,
) -> Result<(), RuntimeError> {
    aelio_prompt::validate_artifact(prompt).map_err(mint_err)?;
    let mut repository = ArtifactRepository::open(store).map_err(artifact_error)?;
    repository
        .put_vendor_promoted(
            VENDOR_ARTIFACT_TENANT,
            prompt_as_artifact(prompt, VENDOR_ARTIFACT_TENANT)?,
        )
        .map(|_| ())
        .map_err(artifact_error)
}

fn mint_err(e: MintError) -> RuntimeError {
    match e {
        MintError::Invalid(s) | MintError::Drafter(s) => RuntimeError::Invalid(s),
        MintError::Refused(s) => RuntimeError::Invalid(format!("refused: {s}")),
    }
}

fn ensure_table(db: &mut Database) -> Result<(), RuntimeError> {
    if db.columns(MINT_TABLE).is_some() {
        return Ok(());
    }
    db.create_table(
        MINT_TABLE,
        &[
            ("id", ColumnKind::Utf8),
            ("version", ColumnKind::Utf8),
            ("tenant", ColumnKind::Utf8),
            ("description", ColumnKind::Text),
            ("objective", ColumnKind::Text),
            ("body", ColumnKind::Text),
            ("how_to_input", ColumnKind::Utf8),
            ("output_imprint", ColumnKind::Utf8),
            ("artifact_json", ColumnKind::Utf8),
            ("embedding", ColumnKind::Vector(EMBED_DIM as u16)),
            ("active", ColumnKind::Bool),
            ("is_axiom", ColumnKind::Bool),
            ("composed_hash", ColumnKind::Utf8),
        ],
    )
    .map_err(|e| RuntimeError::Store(e.to_string()))?;
    Ok(())
}

fn models() -> BTreeMap<String, String> {
    [("embedding".into(), EMBED_MODEL.into())]
        .into_iter()
        .collect()
}

fn mint_collection_schema() -> CollectionSchema {
    let mut columns = BTreeMap::new();
    columns.insert(
        "id".into(),
        PrismColumn {
            kind: PrismColKind::Utf8,
        },
    );
    columns.insert(
        "version".into(),
        PrismColumn {
            kind: PrismColKind::Utf8,
        },
    );
    columns.insert(
        "tenant".into(),
        PrismColumn {
            kind: PrismColKind::Utf8,
        },
    );
    columns.insert(
        "description".into(),
        PrismColumn {
            kind: PrismColKind::Text,
        },
    );
    columns.insert(
        "objective".into(),
        PrismColumn {
            kind: PrismColKind::Text,
        },
    );
    columns.insert(
        "body".into(),
        PrismColumn {
            kind: PrismColKind::Text,
        },
    );
    columns.insert(
        "how_to_input".into(),
        PrismColumn {
            kind: PrismColKind::Utf8,
        },
    );
    columns.insert(
        "output_imprint".into(),
        PrismColumn {
            kind: PrismColKind::Utf8,
        },
    );
    columns.insert(
        "artifact_json".into(),
        PrismColumn {
            kind: PrismColKind::Utf8,
        },
    );
    columns.insert(
        "embedding".into(),
        PrismColumn {
            kind: PrismColKind::Vector {
                dim: EMBED_DIM as u16,
                model: EMBED_MODEL.into(),
            },
        },
    );
    columns.insert(
        "active".into(),
        PrismColumn {
            kind: PrismColKind::Bool,
        },
    );
    columns.insert(
        "is_axiom".into(),
        PrismColumn {
            kind: PrismColKind::Bool,
        },
    );
    columns.insert(
        "composed_hash".into(),
        PrismColumn {
            kind: PrismColKind::Utf8,
        },
    );
    CollectionSchema {
        name: MINT_TABLE.into(),
        columns,
        default_select: vec![
            "id".into(),
            "version".into(),
            "description".into(),
            "how_to_input".into(),
            "output_imprint".into(),
        ],
        default_limit: 10,
    }
}

fn embed_text(art: &PromptArtifact) -> Result<Vec<f32>, RuntimeError> {
    let emb = HashEmbedder { dim: EMBED_DIM };
    let text = format!(
        "{} {} {} {}",
        art.objective, art.description, art.body, art.output_imprint
    );
    emb.embed(EMBED_MODEL, &text).map_err(RuntimeError::Store)
}

fn insert_artifact(
    db: &mut Database,
    art: &PromptArtifact,
    tenant: &str,
) -> Result<(), RuntimeError> {
    let artifact_json =
        serde_json::to_string(art).map_err(|e| RuntimeError::Invalid(e.to_string()))?;
    let how = serde_json::to_string(&art.how_to_input_json())
        .map_err(|e| RuntimeError::Invalid(e.to_string()))?;
    let vector = embed_text(art)?;
    db.insert(
        MINT_TABLE,
        &[
            ("id", Value::Utf8(art.id.clone())),
            ("version", Value::Utf8(art.version.clone())),
            ("tenant", Value::Utf8(tenant.into())),
            ("description", Value::Utf8(art.description.clone())),
            ("objective", Value::Utf8(art.objective.clone())),
            ("body", Value::Utf8(art.body.clone())),
            ("how_to_input", Value::Utf8(how)),
            ("output_imprint", Value::Utf8(art.output_imprint.clone())),
            ("artifact_json", Value::Utf8(artifact_json)),
            ("embedding", Value::Vector(vector)),
            ("active", Value::Bool(true)),
            ("is_axiom", Value::Bool(art.is_axiom)),
            ("composed_hash", Value::Utf8(art.composed_hash.clone())),
        ],
    )
    .map_err(|e| RuntimeError::Store(e.to_string()))?;
    Ok(())
}

fn find_by_key(
    db: &Database,
    tenant: &str,
    id_at_v: &str,
) -> Result<Option<PromptArtifact>, RuntimeError> {
    let (id, version) = id_at_v
        .split_once('@')
        .ok_or_else(|| RuntimeError::Invalid("expected id@version".into()))?;
    let q = parse_prism(&serde_json::json!({
        "from": MINT_TABLE,
        "where": [
            {"col": "id", "op": "eq", "value": id},
            {"col": "version", "op": "eq", "value": version},
            {"col": "tenant", "op": "eq", "value": tenant},
            {"col": "active", "op": "eq", "value": true}
        ],
        "select": ["artifact_json"],
        "limit": 1
    }))
    .map_err(RuntimeError::Invalid)?;
    let hits =
        execute_prism(db, &q, &models(), None).map_err(|e| RuntimeError::Store(e.to_string()))?;
    let Some(hit) = hits.into_iter().next() else {
        return Ok(None);
    };
    let Some(Value::Utf8(json)) = hit.fields.get("artifact_json") else {
        return Ok(None);
    };
    let art: PromptArtifact =
        serde_json::from_str(json).map_err(|e| RuntimeError::Store(e.to_string()))?;
    Ok(Some(art))
}

struct HostMintDrafter<'a> {
    runtime: &'a crate::Runtime,
}

impl MintDrafter for HostMintDrafter<'_> {
    fn name(&self) -> &'static str {
        "host"
    }

    fn draft(
        &self,
        root_text: &str,
        request: &MintRequest,
    ) -> Result<aelio_prompt::MintDraft, MintError> {
        let user = serde_json::json!({
            "objective": request.objective,
            "system_input": request.system_input,
            "output": request.output,
            "input": request.input,
            "id_hint": request.id,
        });
        let composed = format!("{root_text}\n\nMINT REQUEST:\n{user}");
        let prompt_hash = blake3::hash(composed.as_bytes()).to_hex().to_string();
        let content = self
            .runtime
            .complete_model_text(
                &request.tenant,
                &composed,
                &prompt_hash,
                "aelio.model.mint@1",
                16_384,
                0.0,
                &format!("mint:{prompt_hash}"),
            )
            .map_err(|error| MintError::Drafter(error.to_string()))?;
        let mut draft: aelio_prompt::MintDraft =
            serde_json::from_str(&content).map_err(|e| MintError::Drafter(e.to_string()))?;
        draft.model = aelio_prompt::ModelPin {
            id: "aelio.model.mint@1".into(),
            params: [("temperature".into(), serde_json::json!(0.0))]
                .into_iter()
                .collect(),
        };
        Ok(draft)
    }
}

struct HostPromptEvaluator<'a> {
    runtime: &'a crate::Runtime,
    tenant: &'a str,
}

impl crate::PromptEvaluator for HostPromptEvaluator<'_> {
    fn evaluate(
        &self,
        artifact: &PromptArtifact,
        rendered: &str,
        _slots: &BTreeMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value, RuntimeError> {
        let temperature = artifact
            .model
            .params
            .get("temperature")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let prompt_hash = blake3::hash(rendered.as_bytes()).to_hex().to_string();
        let content = self.runtime.complete_model_text(
            self.tenant,
            rendered,
            &prompt_hash,
            &artifact.model.id,
            16_384,
            temperature,
            &format!("prompt-gate:{}:{prompt_hash}", artifact.key()),
        )?;
        serde_json::from_str(&content)
            .map_err(|error| RuntimeError::Host(format!("prompt gate output is not JSON: {error}")))
    }
}

impl crate::Runtime {
    /// Live Mint authoring through the configured TypeScript provider boundary.
    pub fn mint_prompt_with_host(
        &self,
        request: &MintRequest,
        store: bool,
    ) -> Result<MintResult, RuntimeError> {
        let shelf = store.then(|| self.mint_shelf()).transpose()?;
        mint_prompt(shelf.as_ref(), request, &HostMintDrafter { runtime: self })
    }

    /// Evaluate an immutable prompt through the same provider boundary before gate evidence is
    /// recorded.
    pub fn gate_prompt_with_host(
        &self,
        tenant: &str,
        prompt: &PromptArtifact,
        cases: &[crate::PromptGateCase],
        deployer_approval: Option<String>,
    ) -> Result<crate::PromptGateResult, RuntimeError> {
        self.gate_prompt(
            tenant,
            prompt,
            cases,
            &HostPromptEvaluator {
                runtime: self,
                tenant,
            },
            deployer_approval,
        )
    }
}

/// Mint a prompt artifact; optionally persist on a [`MintShelf`] under the runtime data dir.
pub fn mint_prompt(
    shelf: Option<&MintShelf>,
    request: &MintRequest,
    drafter: &dyn MintDrafter,
) -> Result<MintResult, RuntimeError> {
    let mut result = mint_artifact(request, drafter).map_err(mint_err)?;
    if let Some(shelf) = shelf {
        shelf.store(&result.artifact, &request.tenant)?;
        result.stored = true;
    }
    Ok(result)
}

/// Convenience: open/create shelf under `{data_dir}/mint`.
pub fn open_mint_shelf(data_dir: impl AsRef<Path>) -> Result<MintShelf, RuntimeError> {
    MintShelf::open(data_dir.as_ref().join("mint"))
}

/// Prism helper for callers with an open shelf path.
pub fn recall_minted(
    shelf: &MintShelf,
    tenant: &str,
    need: &str,
) -> Result<Vec<PrismHit>, RuntimeError> {
    shelf.recall(tenant, need, RecallModality::Hybrid, 10)
}

pub use aelio_prompt::MockMintDrafter;

#[cfg(test)]
mod tests {
    use super::*;
    use aelio_prompt::{root_id, MintRequest, MintSlot};
    use std::collections::BTreeMap;

    #[test]
    fn mint_store_and_prism_recall() {
        let dir = tempfile::tempdir().unwrap();
        let shelf = MintShelf::open(dir.path()).unwrap();
        let root = shelf
            .get(VENDOR_ARTIFACT_TENANT, &root_id())
            .unwrap()
            .expect("root on shelf");
        assert!(root.is_axiom);

        let req = MintRequest {
            tenant: "demo".into(),
            objective: "classify an incoming customer message".into(),
            system_input: vec![MintSlot {
                name: "message_text".into(),
                ty: "str".into(),
                required: true,
                sensitivity: "pii".into(),
            }],
            output: BTreeMap::from([
                ("class".into(), "str".into()),
                ("confidence".into(), "float".into()),
            ]),
            input: None,
            id: Some("classify.customer".into()),
            version: "1".into(),
        };
        let result = mint_prompt(Some(&shelf), &req, &MockMintDrafter).unwrap();
        assert!(result.stored);
        assert_eq!(result.artifact.key(), "classify.customer@1");

        let got = shelf
            .get("demo", "classify.customer@1")
            .unwrap()
            .expect("stored coin");
        assert!(got.body.contains("{{message_text}}"));
        assert_eq!(
            shelf.artifact_status("demo", &got.id, 1).unwrap(),
            Some(ArtifactStatus::Proposed)
        );
        shelf.store(&result.artifact, "other-tenant").unwrap();
        assert!(shelf
            .get("other-tenant", "classify.customer@1")
            .unwrap()
            .is_some());
        assert!(shelf
            .get("unrelated-tenant", "classify.customer@1")
            .unwrap()
            .is_none());
        assert!(!got.how_to_input_json()["slots"]
            .as_array()
            .unwrap()
            .is_empty());

        let hits = shelf
            .recall(
                "demo",
                "customer message classify",
                RecallModality::Fulltext,
                5,
            )
            .unwrap();
        assert!(hits.is_empty(), "proposed prompts must not be recalled");
    }

    #[test]
    fn mint_refuses_overwrite_root() {
        let dir = tempfile::tempdir().unwrap();
        let shelf = MintShelf::open(dir.path()).unwrap();
        let mut root = aelio_prompt::root_artifact();
        root.is_axiom = false;
        root.id = "aelio.mint.root".into();
        assert!(shelf.store(&root, "demo").is_err());
    }
}
