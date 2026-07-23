//! Tenant-scoped Recall ability adapters over `ll-query`.

use std::collections::HashMap;

use serde::de::DeserializeOwned;

use crate::storage::{AelioStore, LogicalTable, RecallFilter, StoredRecord};
use crate::AelioResult;

#[derive(Debug, Clone)]
pub struct RecallHit<T> {
    pub record: StoredRecord<T>,
    pub score: f32,
    pub channels: Vec<RecallChannel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecallChannel {
    Exact,
    Semantic,
    Lexical,
    Graph,
}

#[derive(Clone)]
pub struct Recall {
    tenant_id: String,
    store: AelioStore,
}

impl Recall {
    pub fn new(tenant_id: impl Into<String>, store: AelioStore) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            store,
        }
    }

    pub fn exact<T: DeserializeOwned>(
        &self,
        table: LogicalTable,
        key: &str,
        owner: Option<&str>,
        now_ms: i64,
    ) -> AelioResult<Option<RecallHit<T>>> {
        let record = self.store.get(&self.tenant_id, table, key)?;
        Ok(record
            .filter(|record| {
                record.envelope.status == "active"
                    && owner.is_none_or(|owner| record.envelope.owner == owner)
                    && record
                        .envelope
                        .expires_at_ms
                        .is_none_or(|expires| expires > now_ms)
            })
            .map(|record| RecallHit {
                record,
                score: 1.0,
                channels: vec![RecallChannel::Exact],
            }))
    }

    pub fn semantic<T: DeserializeOwned>(
        &self,
        table: LogicalTable,
        vector: &[f32],
        owner: Option<&str>,
        now_ms: i64,
        k: usize,
    ) -> AelioResult<Vec<RecallHit<T>>> {
        self.store
            .semantic_search_live(
                &self.tenant_id,
                table,
                vector,
                RecallFilter {
                    status: Some("active"),
                    owner,
                    now_ms,
                    k,
                },
            )
            .map(|hits| tagged(hits, RecallChannel::Semantic))
    }

    pub fn lexical<T: DeserializeOwned>(
        &self,
        table: LogicalTable,
        text: &str,
        owner: Option<&str>,
        now_ms: i64,
        k: usize,
    ) -> AelioResult<Vec<RecallHit<T>>> {
        self.store
            .lexical_search(
                &self.tenant_id,
                table,
                text,
                RecallFilter {
                    status: Some("active"),
                    owner,
                    now_ms,
                    k,
                },
            )
            .map(|hits| tagged(hits, RecallChannel::Lexical))
    }

    pub fn graph<T: DeserializeOwned>(
        &self,
        table: LogicalTable,
        seeds: &[u64],
        depth: usize,
        budget: ll_query::GraphBudget,
        now_ms: i64,
        k: usize,
    ) -> AelioResult<Vec<RecallHit<T>>> {
        self.store
            .graph_search(
                &self.tenant_id,
                table,
                seeds,
                depth,
                budget,
                RecallFilter {
                    status: Some("active"),
                    owner: None,
                    now_ms,
                    k,
                },
            )
            .map(|hits| tagged(hits, RecallChannel::Graph))
    }

    /// Deterministic reciprocal-rank fusion, deduplicated by global row id.
    pub fn fuse<T: Clone>(&self, lists: &[Vec<RecallHit<T>>], k: usize) -> Vec<RecallHit<T>> {
        let mut fused = HashMap::<u64, RecallHit<T>>::new();
        for list in lists {
            for (rank, hit) in list.iter().enumerate() {
                let contribution = 1.0 / (60.0 + rank as f32 + 1.0);
                fused
                    .entry(hit.record.row_id)
                    .and_modify(|current| {
                        current.score += contribution;
                        for channel in &hit.channels {
                            if !current.channels.contains(channel) {
                                current.channels.push(*channel);
                            }
                        }
                    })
                    .or_insert_with(|| RecallHit {
                        record: hit.record.clone(),
                        score: contribution,
                        channels: hit.channels.clone(),
                    });
            }
        }
        let mut hits: Vec<_> = fused.into_values().collect();
        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then(left.record.row_id.cmp(&right.record.row_id))
        });
        hits.truncate(k);
        hits
    }

    /// Apply a deterministic, caller-supplied quality score after retrieval.
    pub fn rerank<T>(
        &self,
        mut hits: Vec<RecallHit<T>>,
        k: usize,
        score: impl Fn(&StoredRecord<T>) -> f32,
    ) -> Vec<RecallHit<T>> {
        for hit in &mut hits {
            hit.score += score(&hit.record);
        }
        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then(left.record.row_id.cmp(&right.record.row_id))
        });
        hits.truncate(k);
        hits
    }
}

fn tagged<T>(hits: Vec<(StoredRecord<T>, f32)>, channel: RecallChannel) -> Vec<RecallHit<T>> {
    hits.into_iter()
        .map(|(record, score)| RecallHit {
            record,
            score,
            channels: vec![channel],
        })
        .collect()
}
