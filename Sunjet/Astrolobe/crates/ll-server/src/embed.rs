//! Text embedding: turn words into query/row vectors so clients never compute embeddings
//! themselves. This closes the last seam — semantic search (and semantic NL) becomes possible
//! over HTTP, and rows can be inserted with text to embed instead of a raw vector.
//!
//! [`Embedder`] is a trait so the backend is pluggable; [`HttpEmbedder`] targets any
//! OpenAI/Voyage-style `/embeddings` endpoint (`POST {input, model}` → `{data:[{embedding}]}`,
//! bearer auth). Tests inject a deterministic mock.
//!
//! Dimensionality is the operator's responsibility: the embedding model must produce vectors
//! matching the target column's dimension (a mismatch is silently ignored by the vector index,
//! same as any wrong-length vector).

use serde_json::json;

use crate::BoxFuture;

/// A backend that maps texts to embedding vectors (batched).
pub trait Embedder: Send + Sync {
    /// Embed each input text, returning one vector per input in the same order.
    fn embed<'a>(&'a self, texts: Vec<String>) -> BoxFuture<'a, Result<Vec<Vec<f32>>, String>>;
}

/// An embedder backed by an OpenAI/Voyage-compatible HTTP `/embeddings` endpoint.
pub struct HttpEmbedder {
    http: reqwest::Client,
    url: String,
    api_key: String,
    model: String,
}

impl HttpEmbedder {
    /// `url` defaults (at the call site) to Voyage AI; `model` must produce vectors matching
    /// the target column's dimension.
    pub fn new(url: String, api_key: String, model: String) -> Self {
        HttpEmbedder {
            http: reqwest::Client::new(),
            url,
            api_key,
            model,
        }
    }
}

impl Embedder for HttpEmbedder {
    fn embed<'a>(&'a self, texts: Vec<String>) -> BoxFuture<'a, Result<Vec<Vec<f32>>, String>> {
        Box::pin(async move {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let body = json!({ "input": texts, "model": self.model });
            let resp = self
                .http
                .post(&self.url)
                .header("authorization", format!("Bearer {}", self.api_key))
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("embedding request failed: {e}"))?;
            let status = resp.status();
            let v: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| format!("embedding response not JSON: {e}"))?;
            if !status.is_success() {
                return Err(format!("embedding API error {status}: {v}"));
            }
            // `data` is an array of `{ embedding: [...], index: n }`; order by index to be safe.
            let data = v
                .get("data")
                .and_then(|d| d.as_array())
                .ok_or_else(|| format!("embedding response missing data: {v}"))?;
            let mut indexed: Vec<(usize, Vec<f32>)> = Vec::with_capacity(data.len());
            for (fallback, item) in data.iter().enumerate() {
                let idx = item.get("index").and_then(|i| i.as_u64()).map(|i| i as usize).unwrap_or(fallback);
                let emb = item
                    .get("embedding")
                    .and_then(|e| e.as_array())
                    .ok_or_else(|| "embedding item missing 'embedding'".to_string())?
                    .iter()
                    .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                    .collect();
                indexed.push((idx, emb));
            }
            indexed.sort_by_key(|(i, _)| *i);
            Ok(indexed.into_iter().map(|(_, e)| e).collect())
        })
    }
}
