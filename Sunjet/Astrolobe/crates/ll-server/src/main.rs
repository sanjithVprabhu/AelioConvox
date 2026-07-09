//! The `ll-server` binary: opens (or creates) a single-node database and serves it over HTTP.
//!
//! Configuration is via environment variables:
//!
//! - `LL_DATA_DIR`  — database directory (default `./lldata`, created if absent).
//! - `LL_BIND`      — listen address (default `127.0.0.1:8080`).
//! - `LL_API_KEYS`  — comma-separated bearer keys. If unset/empty the server runs in **open
//!   mode** (no auth) and logs a warning — fine for local dev, never for production.
//! - `ANTHROPIC_API_KEY` — if set, enables the `/nl` natural-language endpoint.
//! - `ANTHROPIC_MODEL`   — optional model override for NL (default `claude-sonnet-4-6`).
//! - `LL_EMBED_KEY`      — if set, enables embedding (the `semantic` clause + embed-on-insert).
//! - `LL_EMBED_URL`      — embeddings endpoint (default Voyage AI; any OpenAI-style works).
//! - `LL_EMBED_MODEL`    — embedding model (default `voyage-3-lite`; must match column dim).

use std::net::SocketAddr;
use std::sync::Arc;

use ll_query::Database;
use ll_server::embed::HttpEmbedder;
use ll_server::nl::AnthropicClient;
use ll_server::{router, AppState};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var("LL_DATA_DIR").unwrap_or_else(|_| "./lldata".to_string());
    let bind = std::env::var("LL_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    let api_keys: Vec<String> = std::env::var("LL_API_KEYS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    std::fs::create_dir_all(&data_dir)?;
    let db = Database::open(&data_dir)?;
    let mut state = AppState::new(db, api_keys);

    // Enable natural-language queries when an Anthropic key is present.
    let nl_enabled = match std::env::var("ANTHROPIC_API_KEY") {
        Ok(key) if !key.is_empty() => {
            let model = std::env::var("ANTHROPIC_MODEL").ok();
            state = state.with_llm(Arc::new(AnthropicClient::new(key, model)));
            true
        }
        _ => false,
    };

    // Enable embedding (semantic clause + embed-on-insert) when an embedding key is present.
    let embed_enabled = match std::env::var("LL_EMBED_KEY") {
        Ok(key) if !key.is_empty() => {
            let url = std::env::var("LL_EMBED_URL")
                .unwrap_or_else(|_| "https://api.voyageai.com/v1/embeddings".to_string());
            let model = std::env::var("LL_EMBED_MODEL").unwrap_or_else(|_| "voyage-3-lite".to_string());
            state = state.with_embedder(Arc::new(HttpEmbedder::new(url, key, model)));
            true
        }
        _ => false,
    };

    if state.is_open() {
        eprintln!(
            "WARNING: LL_API_KEYS is unset — running in OPEN MODE with no authentication. \
             Set LL_API_KEYS=key1,key2 before exposing this server."
        );
    }
    eprintln!("natural-language endpoint (/v1/tables/{{t}}/nl): {}", if nl_enabled { "enabled" } else { "disabled (set ANTHROPIC_API_KEY)" });
    eprintln!("embedding (semantic clause + embed-on-insert): {}", if embed_enabled { "enabled" } else { "disabled (set LL_EMBED_KEY)" });

    let addr: SocketAddr = bind.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("ll-server listening on http://{addr}  (data dir: {data_dir})");

    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Resolve when the process receives Ctrl-C, so in-flight requests can finish before exit.
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    eprintln!("\nll-server shutting down");
}
