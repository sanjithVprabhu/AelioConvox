use std::net::SocketAddr;

use aelio::embedding::{Embedder, HashEmbedder};
use aelio::provider::{
    GatewayEmbedder, HttpProviderConfig, OpenAiCompatibleProvider, TsGatewayProvider,
};
use aelio::runtime::{DurableRuntime, World};
use aelio::storage::AelioStore;
use aelio_server::sdk_bridge::BridgeConfig;
use aelio_server::{router, AppState};
use ll_query::Database;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = std::env::var("AELIO_DATA_DIR").unwrap_or_else(|_| "./aelio-data".to_string());
    let bind = std::env::var("AELIO_BIND").unwrap_or_else(|_| "127.0.0.1:8090".to_string());
    let tenant_id = std::env::var("AELIO_TENANT_ID").unwrap_or_else(|_| "default".to_string());
    let semantic_embedder_configured = std::env::var("AELIO_LLM_EMBED_URL").is_ok()
        || std::env::var("AELIO_LLM_GATEWAY_URL").is_ok();
    let embedding_dim = optional_positive_u16("AELIO_LLM_EMBED_DIM")?
        .or(optional_positive_u16("AELIO_EMBED_DIM")?)
        .unwrap_or(if semantic_embedder_configured {
            1536
        } else {
            32
        });
    let api_keys: Vec<String> = std::env::var("AELIO_API_KEYS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
        .collect();
    let admin_api_keys: Vec<String> = std::env::var("AELIO_ADMIN_API_KEYS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
        .collect();
    let allow_insecure_open =
        std::env::var("AELIO_ALLOW_INSECURE_OPEN").is_ok_and(|value| affirmative(&value));
    let allow_hash_embedder =
        std::env::var("AELIO_ALLOW_HASH_EMBEDDER").is_ok_and(|value| affirmative(&value));
    if api_keys.is_empty() && admin_api_keys.is_empty() && !allow_insecure_open {
        return Err(
            "authentication is required: set AELIO_API_KEYS (and preferably distinct \
             AELIO_ADMIN_API_KEYS), or explicitly set AELIO_ALLOW_INSECURE_OPEN=1 for local \
             development"
                .into(),
        );
    }

    std::fs::create_dir_all(&data_dir)?;
    let db = if std::path::Path::new(&data_dir).join("catalog.bin").exists() {
        Database::open(&data_dir)?
    } else {
        Database::create(&data_dir)?
    };
    let store = AelioStore::new(db, embedding_dim)?;
    let mut world = World::empty_tenant(&tenant_id)?;
    if std::env::var("AELIO_LLM_GATEWAY_URL").is_ok() {
        world.set_llm_provider(Box::new(TsGatewayProvider::from_env()?));
    } else if let Ok(endpoint) = std::env::var("AELIO_LLM_ENDPOINT") {
        let model = std::env::var("AELIO_LLM_MODEL")
            .map_err(|_| "AELIO_LLM_MODEL is required when AELIO_LLM_ENDPOINT is set")?;
        let timeout_secs = std::env::var("AELIO_LLM_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(30);
        world.set_llm_provider(Box::new(OpenAiCompatibleProvider::new(
            HttpProviderConfig {
                endpoint,
                model,
                api_key: std::env::var("AELIO_LLM_API_KEY").ok(),
                timeout: std::time::Duration::from_secs(timeout_secs),
                extra_headers: Default::default(),
            },
        )?));
    }
    let embedder: std::sync::Arc<dyn Embedder> = if semantic_embedder_configured {
        std::sync::Arc::new(GatewayEmbedder::from_env()?)
    } else if allow_hash_embedder {
        eprintln!(
            "WARNING: AELIO_ALLOW_HASH_EMBEDDER is enabled — semantic generalization is disabled"
        );
        std::sync::Arc::new(HashEmbedder::new(usize::from(embedding_dim))?)
    } else {
        return Err(
            "a semantic embedder is required: set AELIO_LLM_EMBED_URL (or \
             AELIO_LLM_GATEWAY_URL), or explicitly set AELIO_ALLOW_HASH_EMBEDDER=1 for offline \
             development"
                .into(),
        );
    };
    let runtime = DurableRuntime::new_with_embedder(world, store, embedder)?;
    let invocation_timeout = std::env::var("AELIO_SDK_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(std::time::Duration::from_millis)
        .unwrap_or_else(|| std::time::Duration::from_secs(30));
    let max_in_flight_per_tenant = std::env::var("AELIO_SDK_MAX_IN_FLIGHT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(64);
    let state = AppState::new_with_scoped_sdk_bridge(
        runtime,
        api_keys,
        admin_api_keys,
        BridgeConfig {
            invocation_timeout,
            max_in_flight_per_tenant,
            ..BridgeConfig::default()
        },
    );

    if state.is_open() {
        eprintln!(
            "WARNING: AELIO_ALLOW_INSECURE_OPEN is enabled — aelio-server is running without authentication"
        );
    }
    let address: SocketAddr = bind.parse()?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    eprintln!("aelio-server listening on http://{address}");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn affirmative(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn optional_positive_u16(name: &str) -> Result<Option<u16>, Box<dyn std::error::Error>> {
    let value = match std::env::var(name) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(error) => return Err(format!("cannot read {name}: {error}").into()),
    };
    let parsed = value
        .parse::<u16>()
        .map_err(|_| format!("{name} must be a positive integer no greater than 65535"))?;
    if parsed == 0 {
        return Err(format!("{name} must be greater than zero").into());
    }
    Ok(Some(parsed))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::{affirmative, optional_positive_u16};

    #[test]
    fn insecure_open_requires_an_explicit_affirmative_value() {
        for value in ["1", "true", "TRUE", "yes", "on"] {
            assert!(affirmative(value), "{value}");
        }
        for value in ["", "0", "false", "no", "enabled", "typo"] {
            assert!(!affirmative(value), "{value}");
        }
    }

    #[test]
    fn numeric_environment_values_fail_closed() {
        // A missing value is not an error and lets the caller select its documented default.
        let missing = format!("AELIO_TEST_MISSING_{}", std::process::id());
        assert_eq!(optional_positive_u16(&missing).unwrap(), None);
    }
}
