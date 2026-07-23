//! Durable memory taxonomy and retrieval projections.

use serde::{Deserialize, Serialize};

use crate::embedding::Embedder;
use crate::recall::{Recall, RecallHit};
use crate::storage::{AelioStore, LogicalTable, PutIfAbsent, RecordEnvelope};
use crate::{AelioError, AelioResult, ReasonCode};

/// Recognize an explicit user instruction to persist a fact. Ordinary conversation is never
/// silently promoted into durable personal memory.
pub fn explicit_fact(utterance: &str) -> Option<&str> {
    let trimmed = utterance.trim();
    let lower = trimmed.to_ascii_lowercase();
    for prefix in ["remember that ", "remember: "] {
        if lower.starts_with(prefix) {
            let fact = trimmed.get(prefix.len()..)?.trim();
            return (!fact.is_empty()).then_some(fact);
        }
    }
    None
}

/// Recognize the matching explicit instruction to retire a previously stored exact fact.
pub fn explicit_forget_fact(utterance: &str) -> Option<&str> {
    let trimmed = utterance.trim();
    let lower = trimmed.to_ascii_lowercase();
    for prefix in ["forget that ", "forget: "] {
        if lower.starts_with(prefix) {
            let fact = trimmed.get(prefix.len()..)?.trim();
            return (!fact.is_empty()).then_some(fact);
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Episodic,
    Factual,
    Entity,
    OpenLoop,
    Procedural,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryProvenance {
    pub source_kind: String,
    pub source_id: String,
    pub observed_at_ms: i64,
    pub actor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidTime {
    pub from_ms: i64,
    pub to_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecayPolicy {
    pub half_life_ms: Option<i64>,
    pub floor: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub kind: MemoryKind,
    pub text: String,
    pub provenance: MemoryProvenance,
    pub confidence: f32,
    pub valid_time: ValidTime,
    pub expires_at_ms: Option<i64>,
    pub decay: DecayPolicy,
    pub entity_id: Option<String>,
    pub open_loop_state: Option<String>,
    pub procedure_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemoryShape {
    Episodic {
        event: String,
        observed_at_ms: i64,
    },
    Factual {
        fact: String,
        confidence: f32,
    },
    Entity {
        entity_id: String,
        summary: String,
    },
    OpenLoop {
        item: String,
        state: String,
    },
    Procedural {
        instruction: String,
        procedure_version: String,
    },
}

#[derive(Clone)]
pub struct MemoryStore {
    tenant_id: String,
    store: AelioStore,
}

impl MemoryStore {
    pub fn new(tenant_id: impl Into<String>, store: AelioStore) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            store,
        }
    }

    pub fn remember(
        &mut self,
        user_id: &str,
        memory: Memory,
        embedder: &dyn Embedder,
        now_ms: i64,
    ) -> AelioResult<PutIfAbsent> {
        validate(&memory)?;
        let embedding = embedder.embed(&memory.text)?;
        self.store.put_if_absent(
            &self.tenant_id,
            LogicalTable::Memories,
            &RecordEnvelope {
                key: memory.id.clone(),
                kind: kind_name(memory.kind).into(),
                status: "active".into(),
                owner: user_id.into(),
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
                expires_at_ms: memory.expires_at_ms,
                value: memory,
            },
            Some(&embedding),
        )
    }

    pub fn exact(
        &self,
        user_id: &str,
        id: &str,
        now_ms: i64,
    ) -> AelioResult<Option<(MemoryShape, f32)>> {
        let recall = Recall::new(&self.tenant_id, self.store.clone());
        recall
            .exact::<Memory>(LogicalTable::Memories, id, Some(user_id), now_ms)
            .map(|hit| hit.and_then(|hit| project_hit(hit, now_ms)))
    }

    pub fn semantic(
        &self,
        user_id: &str,
        query: &str,
        embedder: &dyn Embedder,
        now_ms: i64,
        k: usize,
    ) -> AelioResult<Vec<(MemoryShape, f32)>> {
        let vector = embedder.embed(query)?;
        let recall = Recall::new(&self.tenant_id, self.store.clone());
        let hits = recall.semantic(
            LogicalTable::Memories,
            &vector,
            Some(user_id),
            now_ms,
            k.saturating_mul(2),
        )?;
        Ok(project_hits(hits, now_ms, k))
    }

    pub fn lexical(
        &self,
        user_id: &str,
        query: &str,
        now_ms: i64,
        k: usize,
    ) -> AelioResult<Vec<(MemoryShape, f32)>> {
        let recall = Recall::new(&self.tenant_id, self.store.clone());
        let hits = recall.lexical(
            LogicalTable::Memories,
            query,
            Some(user_id),
            now_ms,
            k.saturating_mul(2),
        )?;
        Ok(project_hits(hits, now_ms, k))
    }
}

fn validate(memory: &Memory) -> AelioResult<()> {
    if memory.id.trim().is_empty()
        || memory.text.trim().is_empty()
        || !(0.0..=1.0).contains(&memory.confidence)
        || !(0.0..=1.0).contains(&memory.decay.floor)
        || memory
            .valid_time
            .to_ms
            .is_some_and(|to| to <= memory.valid_time.from_ms)
        || memory.decay.half_life_ms.is_some_and(|value| value <= 0)
    {
        return Err(AelioError::new(
            ReasonCode::Validation,
            "invalid memory identity, confidence, valid-time, or decay policy",
        ));
    }
    match memory.kind {
        MemoryKind::Entity if memory.entity_id.is_none() => missing("entity_id"),
        MemoryKind::OpenLoop if memory.open_loop_state.is_none() => missing("open_loop_state"),
        MemoryKind::Procedural if memory.procedure_version.is_none() => {
            missing("procedure_version")
        }
        _ => Ok(()),
    }
}

fn missing(field: &str) -> AelioResult<()> {
    Err(AelioError::new(
        ReasonCode::Validation,
        format!("memory taxonomy requires {field}"),
    ))
}

fn project_hits(hits: Vec<RecallHit<Memory>>, now_ms: i64, k: usize) -> Vec<(MemoryShape, f32)> {
    let mut projected: Vec<_> = hits
        .into_iter()
        .filter_map(|hit| project_hit(hit, now_ms))
        .collect();
    projected.sort_by(|left, right| right.1.total_cmp(&left.1));
    projected.truncate(k);
    projected
}

fn project_hit(hit: RecallHit<Memory>, now_ms: i64) -> Option<(MemoryShape, f32)> {
    let memory = hit.record.envelope.value;
    if now_ms < memory.valid_time.from_ms || memory.valid_time.to_ms.is_some_and(|to| now_ms >= to)
    {
        return None;
    }
    let age = now_ms.saturating_sub(memory.provenance.observed_at_ms);
    let decay = memory
        .decay
        .half_life_ms
        .map_or(1.0, |half_life| 0.5_f32.powf(age as f32 / half_life as f32));
    let quality = memory.confidence * decay.max(memory.decay.floor);
    let shape = match memory.kind {
        MemoryKind::Episodic => MemoryShape::Episodic {
            event: memory.text,
            observed_at_ms: memory.provenance.observed_at_ms,
        },
        MemoryKind::Factual => MemoryShape::Factual {
            fact: memory.text,
            confidence: memory.confidence,
        },
        MemoryKind::Entity => MemoryShape::Entity {
            entity_id: memory.entity_id?,
            summary: memory.text,
        },
        MemoryKind::OpenLoop => MemoryShape::OpenLoop {
            item: memory.text,
            state: memory.open_loop_state?,
        },
        MemoryKind::Procedural => MemoryShape::Procedural {
            instruction: memory.text,
            procedure_version: memory.procedure_version?,
        },
    };
    Some((shape, hit.score * quality))
}

fn kind_name(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Episodic => "episodic",
        MemoryKind::Factual => "factual",
        MemoryKind::Entity => "entity",
        MemoryKind::OpenLoop => "open_loop",
        MemoryKind::Procedural => "procedural",
    }
}
