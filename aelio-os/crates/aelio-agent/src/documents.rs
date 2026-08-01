//! Virtual, tenant-scoped document namespaces backed by Aelio DB rows.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::embedding::Embedder;
use crate::policy::{require_allow, PolicyActionRisk, PolicyCtx};
use crate::recall::Recall;
use crate::storage::{
    AelioStore, CompareSwap, LogicalTable, PutIfAbsent, RecordEnvelope, StoredRecord,
};
use crate::tenant::PolicySpec;
use crate::{AelioError, AelioResult, ReasonCode};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum DocumentNamespace {
    Tenant,
    User(String),
    Session(String),
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentSection {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualDocument {
    pub id: String,
    pub namespace: DocumentNamespace,
    pub title: String,
    pub sections: Vec<DocumentSection>,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentChunk {
    pub document_id: String,
    pub namespace: DocumentNamespace,
    pub section_id: String,
    pub ordinal: usize,
    pub text: String,
    pub document_revision: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct DocumentQuota {
    pub max_documents: usize,
    pub max_bytes: usize,
    pub max_chunks_per_document: usize,
    pub chunk_bytes: usize,
}

#[derive(Clone, Copy)]
pub struct DocumentMutationContext<'a> {
    pub policies: &'a [PolicySpec],
    pub policy: &'a PolicyCtx,
    pub embedder: &'a dyn Embedder,
    pub now_ms: i64,
}

impl Default for DocumentQuota {
    fn default() -> Self {
        Self {
            max_documents: 1_000,
            max_bytes: 10 * 1024 * 1024,
            max_chunks_per_document: 1_024,
            chunk_bytes: 1_024,
        }
    }
}

#[derive(Clone)]
pub struct VirtualDocumentStore {
    tenant_id: String,
    store: AelioStore,
    quota: DocumentQuota,
}

impl VirtualDocumentStore {
    pub fn new(
        tenant_id: impl Into<String>,
        store: AelioStore,
        quota: DocumentQuota,
    ) -> AelioResult<Self> {
        if quota.max_documents == 0
            || quota.max_bytes == 0
            || quota.max_chunks_per_document == 0
            || quota.chunk_bytes == 0
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "document quotas must be positive",
            ));
        }
        Ok(Self {
            tenant_id: tenant_id.into(),
            store,
            quota,
        })
    }

    pub fn write(
        &mut self,
        mut document: VirtualDocument,
        context: &DocumentMutationContext<'_>,
    ) -> AelioResult<PutIfAbsent> {
        validate_document(&document)?;
        authorize_document_write(context.policies, context.policy, "document.write")?;
        self.enforce_quota(&document)?;
        document.revision = 1;
        let key = document_key(&document.namespace, &document.id);
        let result = self.store.put_if_absent(
            &self.tenant_id,
            LogicalTable::Documents,
            &document_envelope(&key, &document, context.now_ms, context.now_ms),
            Some(&context.embedder.embed(&document_text(&document))?),
        )?;
        if matches!(result, PutIfAbsent::Inserted { .. }) {
            self.write_chunks(&document, context.embedder, context.now_ms)?;
        }
        Ok(result)
    }

    pub fn section(
        &self,
        namespace: &DocumentNamespace,
        document_id: &str,
        section_id: &str,
    ) -> AelioResult<Option<DocumentSection>> {
        let key = document_key(namespace, document_id);
        let document: Option<StoredRecord<VirtualDocument>> =
            self.store
                .get(&self.tenant_id, LogicalTable::Documents, &key)?;
        Ok(document.and_then(|record| {
            record
                .envelope
                .value
                .sections
                .into_iter()
                .find(|section| section.id == section_id)
        }))
    }

    pub fn patch_section(
        &mut self,
        namespace: &DocumentNamespace,
        document_id: &str,
        section: DocumentSection,
        expected_version: u64,
        context: &DocumentMutationContext<'_>,
    ) -> AelioResult<CompareSwap> {
        validate_id(&section.id)?;
        authorize_document_write(context.policies, context.policy, "document.patch")?;
        let key = document_key(namespace, document_id);
        let current: StoredRecord<VirtualDocument> = self
            .store
            .get(&self.tenant_id, LogicalTable::Documents, &key)?
            .ok_or_else(|| AelioError::new(ReasonCode::NotFound, "document not found"))?;
        if current.version != expected_version {
            return Ok(CompareSwap::Conflict {
                current_version: current.version,
            });
        }
        let mut document = current.envelope.value;
        let target = document
            .sections
            .iter_mut()
            .find(|candidate| candidate.id == section.id)
            .ok_or_else(|| AelioError::new(ReasonCode::NotFound, "section not found"))?;
        *target = section;
        document.revision = document.revision.saturating_add(1);
        self.enforce_quota(&document)?;
        let result = self.store.compare_swap(
            &self.tenant_id,
            LogicalTable::Documents,
            current.row_id,
            expected_version,
            &document_envelope(
                &key,
                &document,
                current.envelope.created_at_ms,
                context.now_ms,
            ),
            Some(&context.embedder.embed(&document_text(&document))?),
        )?;
        if matches!(result, CompareSwap::Updated { .. }) {
            self.write_chunks(&document, context.embedder, context.now_ms)?;
        }
        Ok(result)
    }

    pub fn search(
        &self,
        namespace: &DocumentNamespace,
        query: &str,
        semantic_vector: Option<&[f32]>,
        now_ms: i64,
        k: usize,
    ) -> AelioResult<Vec<DocumentChunk>> {
        let owner = namespace_key(namespace);
        let recall = Recall::new(&self.tenant_id, self.store.clone());
        let lexical = recall.lexical::<DocumentChunk>(
            LogicalTable::DocumentChunks,
            query,
            Some(&owner),
            now_ms,
            k.saturating_mul(3),
        )?;
        let semantic = if let Some(vector) = semantic_vector {
            recall.semantic::<DocumentChunk>(
                LogicalTable::DocumentChunks,
                vector,
                Some(&owner),
                now_ms,
                k.saturating_mul(3),
            )?
        } else {
            Vec::new()
        };
        let candidates = recall.fuse(&[lexical, semantic], k.saturating_mul(3));
        let mut live = Vec::new();
        for hit in candidates {
            let chunk = hit.record.envelope.value;
            let key = document_key(&chunk.namespace, &chunk.document_id);
            let document: Option<StoredRecord<VirtualDocument>> =
                self.store
                    .get(&self.tenant_id, LogicalTable::Documents, &key)?;
            if document
                .is_some_and(|record| record.envelope.value.revision == chunk.document_revision)
            {
                live.push(chunk);
                if live.len() == k {
                    break;
                }
            }
        }
        Ok(live)
    }

    fn enforce_quota(&self, document: &VirtualDocument) -> AelioResult<()> {
        let bytes = document_text(document).len();
        let chunks = document
            .sections
            .iter()
            .map(|section| section.text.len().div_ceil(self.quota.chunk_bytes))
            .sum::<usize>();
        let documents: Vec<StoredRecord<VirtualDocument>> = self.store.list(
            &self.tenant_id,
            LogicalTable::Documents,
            Some("active"),
            self.quota.max_documents.saturating_add(1),
        )?;
        let exists = documents
            .iter()
            .any(|record| record.envelope.key == document_key(&document.namespace, &document.id));
        if (!exists && documents.len() >= self.quota.max_documents)
            || bytes > self.quota.max_bytes
            || chunks > self.quota.max_chunks_per_document
        {
            return Err(AelioError::new(
                ReasonCode::BudgetExceeded,
                "virtual document quota exceeded",
            ));
        }
        Ok(())
    }

    fn write_chunks(
        &mut self,
        document: &VirtualDocument,
        embedder: &dyn Embedder,
        now_ms: i64,
    ) -> AelioResult<()> {
        for section in &document.sections {
            for (ordinal, text) in chunk_text(&section.text, self.quota.chunk_bytes)
                .into_iter()
                .enumerate()
            {
                let chunk = DocumentChunk {
                    document_id: document.id.clone(),
                    namespace: document.namespace.clone(),
                    section_id: section.id.clone(),
                    ordinal,
                    text,
                    document_revision: document.revision,
                };
                let key = format!(
                    "{}:{}:{}:{}",
                    document_key(&document.namespace, &document.id),
                    document.revision,
                    section.id,
                    ordinal
                );
                let embedding = embedder.embed(&chunk.text)?;
                let _ = self.store.put_if_absent(
                    &self.tenant_id,
                    LogicalTable::DocumentChunks,
                    &RecordEnvelope {
                        key,
                        kind: "document_chunk".into(),
                        status: "active".into(),
                        owner: namespace_key(&document.namespace),
                        created_at_ms: now_ms,
                        updated_at_ms: now_ms,
                        expires_at_ms: None,
                        value: chunk,
                    },
                    Some(&embedding),
                )?;
            }
        }
        Ok(())
    }
}

fn authorize_document_write(
    policies: &[PolicySpec],
    policy: &PolicyCtx,
    capability: &str,
) -> AelioResult<()> {
    let mut context = policy.clone();
    context.capability = Some(capability.into());
    context.action_risk = PolicyActionRisk::DocumentWrite;
    require_allow(policies, &context)
}

fn validate_document(document: &VirtualDocument) -> AelioResult<()> {
    validate_id(&document.id)?;
    if document.title.trim().is_empty() || document.sections.is_empty() {
        return Err(AelioError::new(
            ReasonCode::Validation,
            "document title and sections are required",
        ));
    }
    let mut sections = HashSet::new();
    if document
        .sections
        .iter()
        .any(|section| validate_id(&section.id).is_err() || !sections.insert(&section.id))
    {
        return Err(AelioError::new(
            ReasonCode::Validation,
            "section ids must be valid and unique",
        ));
    }
    Ok(())
}

fn validate_id(id: &str) -> AelioResult<()> {
    if id.trim().is_empty()
        || id.contains('/')
        || id.contains('\\')
        || id.contains("..")
        || id.chars().any(char::is_control)
    {
        return Err(AelioError::new(
            ReasonCode::Validation,
            "virtual document ids are opaque names, not filesystem paths",
        ));
    }
    Ok(())
}

fn document_key(namespace: &DocumentNamespace, id: &str) -> String {
    format!("{}:{id}", namespace_key(namespace))
}

fn namespace_key(namespace: &DocumentNamespace) -> String {
    match namespace {
        DocumentNamespace::Tenant => "tenant".into(),
        DocumentNamespace::User(id) => format!("user:{id}"),
        DocumentNamespace::Session(id) => format!("session:{id}"),
        DocumentNamespace::System => "system".into(),
    }
}

fn document_envelope(
    key: &str,
    document: &VirtualDocument,
    created_at_ms: i64,
    updated_at_ms: i64,
) -> RecordEnvelope<VirtualDocument> {
    RecordEnvelope {
        key: key.into(),
        kind: "virtual_document".into(),
        status: "active".into(),
        owner: namespace_key(&document.namespace),
        created_at_ms,
        updated_at_ms,
        expires_at_ms: None,
        value: document.clone(),
    }
}

fn document_text(document: &VirtualDocument) -> String {
    let mut text = document.title.clone();
    for section in &document.sections {
        text.push('\n');
        text.push_str(&section.text);
    }
    text
}

fn chunk_text(text: &str, max_bytes: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let additional = usize::from(!current.is_empty()) + word.len();
        if !current.is_empty() && current.len().saturating_add(additional) > max_bytes {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}
