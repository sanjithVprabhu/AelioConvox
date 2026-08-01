//! Embedding boundary used by recall-facing stores.

use sha2::{Digest, Sha256};

use crate::{AelioError, AelioResult, ReasonCode};

pub trait Embedder: Send + Sync {
    fn dimension(&self) -> usize;
    fn embed(&self, text: &str) -> AelioResult<Vec<f32>>;

    /// Stable identity of the vector space, not merely its dimension. Persisted vectors from two
    /// equal-dimensional models are not interchangeable.
    fn space_id(&self) -> String {
        format!("{}:{}", std::any::type_name::<Self>(), self.dimension())
    }

    /// Whether cosine distance in this embedding space is meaningful for semantic equivalence.
    /// Deterministic hash embeddings are useful for offline replay and lexical-ish bucketing, but
    /// must never authorize a novel synonym or polarity decision.
    fn supports_semantic_equivalence(&self) -> bool {
        false
    }
}

/// Stable local embedder for tests and deterministic offline operation.
#[derive(Debug, Clone, Copy)]
pub struct HashEmbedder {
    dimension: usize,
}

impl HashEmbedder {
    pub fn new(dimension: usize) -> AelioResult<Self> {
        if dimension == 0 {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "embedding dimension must be positive",
            ));
        }
        Ok(Self { dimension })
    }
}

impl Embedder for HashEmbedder {
    fn dimension(&self) -> usize {
        self.dimension
    }

    fn embed(&self, text: &str) -> AelioResult<Vec<f32>> {
        let mut vector = vec![0.0_f32; self.dimension];
        for token in text.split_whitespace().map(str::to_lowercase) {
            let digest = Sha256::digest(token.as_bytes());
            let index =
                u64::from_le_bytes(digest[..8].try_into().unwrap()) as usize % self.dimension;
            let sign = if digest[8] & 1 == 0 { 1.0 } else { -1.0 };
            vector[index] += sign;
        }
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        if norm > 0.0 {
            for value in &mut vector {
                *value /= norm;
            }
        }
        Ok(vector)
    }

    fn space_id(&self) -> String {
        format!("aelio.hash-v1:{}", self.dimension)
    }
}
