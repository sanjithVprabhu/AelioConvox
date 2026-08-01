//! Bounded, tenant/user-scoped evidence assembly for the live turn loop.

use crate::abilities::judge::EvidenceClaim;
use crate::documents::{DocumentChunk, DocumentNamespace, VirtualDocument};
use crate::embedding::{Embedder, HashEmbedder};
use crate::memory::Memory;
use crate::recall::{Recall, RecallHit};
use crate::storage::{AelioStore, LogicalTable};
use crate::types::{AelioResult, Value};
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
pub struct RecallBudget {
    pub max_hits: usize,
    pub max_chars: usize,
}

impl Default for RecallBudget {
    fn default() -> Self {
        Self {
            max_hits: 8,
            max_chars: 8_000,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RecalledEvidence {
    pub claims: Vec<EvidenceClaim>,
    pub truncated: bool,
}

pub trait TurnRecall: Send {
    fn retrieve(
        &mut self,
        user_id: &str,
        query: &str,
        now_ms: i64,
        budget: RecallBudget,
    ) -> AelioResult<RecalledEvidence>;
}

#[derive(Default)]
pub struct NoopTurnRecall;

impl TurnRecall for NoopTurnRecall {
    fn retrieve(
        &mut self,
        _user_id: &str,
        _query: &str,
        _now_ms: i64,
        _budget: RecallBudget,
    ) -> AelioResult<RecalledEvidence> {
        Ok(RecalledEvidence::default())
    }
}

pub struct StoreTurnRecall {
    tenant_id: String,
    store: AelioStore,
    embedder: Arc<dyn Embedder>,
}

impl StoreTurnRecall {
    pub fn new(tenant_id: impl Into<String>, store: AelioStore) -> AelioResult<Self> {
        let dimension = store.embedding_dimension();
        Self::with_embedder(tenant_id, store, Arc::new(HashEmbedder::new(dimension)?))
    }

    pub fn with_embedder(
        tenant_id: impl Into<String>,
        store: AelioStore,
        embedder: Arc<dyn Embedder>,
    ) -> AelioResult<Self> {
        if embedder.dimension() != store.embedding_dimension() {
            return Err(crate::types::AelioError::new(
                crate::types::ReasonCode::Validation,
                "recall embedder dimension does not match the tenant store",
            ));
        }
        Ok(Self {
            tenant_id: tenant_id.into(),
            store,
            embedder,
        })
    }
}

impl TurnRecall for StoreTurnRecall {
    fn retrieve(
        &mut self,
        user_id: &str,
        query: &str,
        now_ms: i64,
        budget: RecallBudget,
    ) -> AelioResult<RecalledEvidence> {
        if budget.max_hits == 0 || budget.max_chars == 0 || query.trim().is_empty() {
            return Ok(RecalledEvidence::default());
        }
        let recall = Recall::new(&self.tenant_id, self.store.clone());
        let candidate_limit = budget.max_hits.saturating_mul(3).min(64);
        let vector = self.embedder.embed(query)?;

        let memory_lexical = recall.lexical::<Memory>(
            LogicalTable::Memories,
            query,
            Some(user_id),
            now_ms,
            candidate_limit,
        )?;
        let memory_semantic = if memory_lexical.len() < budget.max_hits {
            recall.semantic::<Memory>(
                LogicalTable::Memories,
                &vector,
                Some(user_id),
                now_ms,
                candidate_limit,
            )?
        } else {
            Vec::new()
        };
        let memories = recall.fuse(&[memory_lexical, memory_semantic], candidate_limit);

        let mut document_hits = Vec::new();
        for owner in [format!("user:{user_id}"), "tenant".into(), "system".into()] {
            let lexical = recall.lexical::<DocumentChunk>(
                LogicalTable::DocumentChunks,
                query,
                Some(&owner),
                now_ms,
                candidate_limit,
            )?;
            let semantic = if lexical.len() < budget.max_hits {
                recall.semantic::<DocumentChunk>(
                    LogicalTable::DocumentChunks,
                    &vector,
                    Some(&owner),
                    now_ms,
                    candidate_limit,
                )?
            } else {
                Vec::new()
            };
            document_hits.extend(recall.fuse(&[lexical, semantic], candidate_limit));
        }
        document_hits.retain(|hit| self.chunk_is_current(&hit.record.envelope.value));
        document_hits.sort_by(|left, right| right.score.total_cmp(&left.score));

        assemble_claims(memories, document_hits, now_ms, budget)
    }
}

impl StoreTurnRecall {
    fn chunk_is_current(&self, chunk: &DocumentChunk) -> bool {
        let namespace = match &chunk.namespace {
            DocumentNamespace::Tenant => "tenant".into(),
            DocumentNamespace::User(id) => format!("user:{id}"),
            DocumentNamespace::Session(id) => format!("session:{id}"),
            DocumentNamespace::System => "system".into(),
        };
        let key = format!("{namespace}:{}", chunk.document_id);
        self.store
            .get::<VirtualDocument>(&self.tenant_id, LogicalTable::Documents, &key)
            .ok()
            .flatten()
            .is_some_and(|document| document.envelope.value.revision == chunk.document_revision)
    }
}

fn assemble_claims(
    memories: Vec<RecallHit<Memory>>,
    documents: Vec<RecallHit<DocumentChunk>>,
    now_ms: i64,
    budget: RecallBudget,
) -> AelioResult<RecalledEvidence> {
    let mut candidates = Vec::new();
    for hit in memories {
        let memory = hit.record.envelope.value;
        if now_ms < memory.valid_time.from_ms
            || memory.valid_time.to_ms.is_some_and(|to| now_ms >= to)
        {
            continue;
        }
        candidates.push((
            hit.score,
            EvidenceClaim {
                id: format!("memory.{}.v{}", memory.id, hit.record.version),
                value: Value::str(memory.text),
                provenance: format!(
                    "memory:{}:{}:{}",
                    memory.provenance.source_kind,
                    memory.provenance.source_id,
                    memory.provenance.observed_at_ms
                ),
            },
        ));
    }
    for hit in documents {
        let chunk = hit.record.envelope.value;
        candidates.push((
            hit.score,
            EvidenceClaim {
                id: format!(
                    "document.{}.{}.{}.v{}",
                    chunk.document_id, chunk.section_id, chunk.ordinal, chunk.document_revision
                ),
                value: Value::str(chunk.text),
                provenance: format!("document:{}:{}", chunk.document_id, chunk.document_revision),
            },
        ));
    }
    candidates.sort_by(|left, right| right.0.total_cmp(&left.0).then(left.1.id.cmp(&right.1.id)));

    let mut claims = Vec::new();
    let mut chars = 0usize;
    let mut truncated = false;
    for (_, claim) in candidates {
        let claim_chars = crate::ops::pure::value_to_json(&claim.value)
            .to_string()
            .chars()
            .count();
        if chars.saturating_add(claim_chars) > budget.max_chars || claims.len() == budget.max_hits {
            truncated = true;
            break;
        }
        chars += claim_chars;
        claims.push(claim);
    }
    Ok(RecalledEvidence { claims, truncated })
}
