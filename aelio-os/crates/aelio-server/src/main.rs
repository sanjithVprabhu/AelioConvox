use aelio_agent::embedding::{Embedder, HashEmbedder};
use aelio_agent::provider::{GatewayEmbedder, TsGatewayProvider};
use aelio_agent::runtime::{DurableRuntime, World};
use aelio_agent::storage::AelioStore;
use aelio_db_query::{storage_from_env, Database};
use aelio_runtime::{Runtime, RuntimeConfig, DEFAULT_QUEUE_DEPTH};
use aelio_server::{router, ServerState};
use std::net::SocketAddr;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if run_maintenance_command()? {
        return Ok(());
    }
    // Build all synchronous state before entering Tokio. Several runtime adapters intentionally
    // use reqwest's blocking client inside dedicated blocking workers; constructing or dropping
    // those clients while an async runtime is entered can mask an ordinary configuration error
    // with reqwest's "cannot drop a runtime in an async context" panic.
    let (address, application) = build_application()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(address).await?;
        eprintln!("authoritative Aelio Rust runtime listening on http://{address}");
        axum::serve(listener, application)
            .with_graceful_shutdown(shutdown_signal())
            .await
    })?;
    Ok(())
}

fn build_application() -> Result<(SocketAddr, axum::Router), Box<dyn std::error::Error>> {
    let bind = env("AELIO_RUNTIME_BIND", "127.0.0.1:8090");
    let data_dir = env("AELIO_DATA_DIR", "./aelio-data");
    let host_url = std::env::var("AELIO_HOST_URL").ok();
    let host_token = std::env::var("AELIO_HOST_TOKEN").ok();
    let api_tokens: Vec<String> = std::env::var("AELIO_RUNTIME_TOKENS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect();
    let allow_insecure = env_flag("AELIO_ALLOW_INSECURE_OPEN");
    let queue_depth = optional_usize("AELIO_INSTANCE_QUEUE_DEPTH")?.unwrap_or(DEFAULT_QUEUE_DEPTH);
    let event_key_secret = match secret_32("AELIO_EVENT_KEY_SECRET") {
        Ok(secret) => secret,
        Err(_) if allow_insecure => {
            eprintln!(
                "WARNING: using a fixed development event-key secret because insecure mode is enabled"
            );
            [0_u8; 32]
        }
        Err(error) => return Err(error.into()),
    };

    let data_root = std::path::PathBuf::from(data_dir);
    let runtime = Runtime::open(RuntimeConfig {
        data_dir: data_root.join("runtime"),
        host_url,
        host_token,
        event_key_secret,
        queue_depth,
    })?;
    let database_dir = data_root.join("database");
    std::fs::create_dir_all(&database_dir)?;
    let (_, segment_store) = storage_from_env(database_dir.clone())?;
    let database = Database::open_with_store(database_dir, segment_store)?;
    let tenant_id = env("AELIO_TENANT_ID", "default");
    let agent_dir = data_root.join("agent");
    std::fs::create_dir_all(&agent_dir)?;
    let agent_database = if agent_dir.join("catalog.bin").exists() {
        Database::open(&agent_dir)?
    } else {
        Database::create(&agent_dir)?
    };
    let embedding_dim = optional_usize("AELIO_LLM_EMBED_DIM")?.unwrap_or(1_536);
    let agent_store = AelioStore::new(
        agent_database,
        u16::try_from(embedding_dim)
            .map_err(|_| "AELIO_LLM_EMBED_DIM must be no greater than 65535")?,
    )?;
    let mut world = World::empty_tenant(&tenant_id)?;
    if std::env::var("AELIO_LLM_GATEWAY_URL").is_ok() {
        world.set_llm_provider(Box::new(TsGatewayProvider::from_env()?));
    } else if !allow_insecure {
        return Err("AELIO_LLM_GATEWAY_URL is required for the adaptive Rust agent".into());
    }
    let embedder: std::sync::Arc<dyn Embedder> = if std::env::var("AELIO_LLM_EMBED_URL").is_ok()
        || std::env::var("AELIO_LLM_GATEWAY_URL").is_ok()
    {
        std::sync::Arc::new(GatewayEmbedder::from_env()?)
    } else if allow_insecure {
        std::sync::Arc::new(HashEmbedder::new(embedding_dim)?)
    } else {
        return Err("AELIO_LLM_EMBED_URL is required for the adaptive Rust agent".into());
    };
    let database_state = if allow_insecure {
        aelio_db_api::AppState::new(database, vec![])
    } else {
        aelio_db_api::AppState::new_tenant_scoped(
            database,
            api_tokens
                .iter()
                .cloned()
                .map(|token| (tenant_id.clone(), token))
                .collect(),
        )?
    }
    .with_embedder(std::sync::Arc::new(DatabaseEmbedder(embedder.clone())));
    let agent_runtime = DurableRuntime::new_with_embedder(world, agent_store, embedder)?;
    let agent_state = aelio_agent_api::AppState::new_with_artifact_runtime(
        agent_runtime,
        api_tokens.clone(),
        runtime.clone(),
    );
    let state = if allow_insecure {
        ServerState::new(runtime, vec![], true)?
    } else {
        ServerState::new_tenant_scoped(
            runtime,
            api_tokens
                .into_iter()
                .map(|token| (tenant_id.clone(), token))
                .collect(),
        )?
    };
    if allow_insecure {
        eprintln!(
            "WARNING: AELIO_ALLOW_INSECURE_OPEN=1; the Rust runtime accepts unauthenticated requests"
        );
    }

    let application = router(state)
        .merge(aelio_db_api::router(database_state))
        .nest("/agent", aelio_agent_api::router(agent_state));
    Ok((bind.parse()?, application))
}

/// Shares the canonical agent embedding space with Prism/Aelio DB without blocking Tokio's I/O
/// workers. One configured model and one cache therefore serve learning and database recall.
struct DatabaseEmbedder(std::sync::Arc<dyn Embedder>);

impl aelio_db_api::embed::Embedder for DatabaseEmbedder {
    fn embed<'a>(
        &'a self,
        texts: Vec<String>,
    ) -> aelio_db_api::BoxFuture<'a, Result<Vec<Vec<f32>>, String>> {
        let embedder = self.0.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                texts
                    .iter()
                    .map(|text| embedder.embed(text).map_err(|error| error.to_string()))
                    .collect()
            })
            .await
            .map_err(|error| format!("embedding worker failed: {error}"))?
        })
    }
}

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn optional_usize(name: &str) -> Result<Option<usize>, String> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<usize>()
            .map(Some)
            .map_err(|_| format!("{name} must be a positive integer")),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(format!("cannot read {name}: {error}")),
    }
}

fn secret_32(name: &str) -> Result<[u8; 32], String> {
    let value = std::env::var(name)
        .map_err(|_| format!("{name} is required and must contain at least 32 bytes"))?;
    let bytes = value.as_bytes();
    if bytes.len() < 32 {
        return Err(format!("{name} must contain at least 32 bytes"));
    }
    Ok(*blake3::hash(bytes).as_bytes())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM handler must install");
        tokio::select! {
            result = tokio::signal::ctrl_c() => { let _ = result; }
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    eprintln!("shutdown signal received; draining admitted HTTP work");
}

fn run_maintenance_command() -> Result<bool, Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let Some(command) = args.first().and_then(|value| value.to_str()) else {
        return Ok(false);
    };
    let path = |index: usize| {
        args.get(index)
            .map(std::path::PathBuf::from)
            .ok_or_else(|| format!("missing path argument {index}"))
    };
    match command {
        "backup" if args.len() == 3 => {
            let manifest = aelio_server::backup::create_backup(&path(1)?, &path(2)?)?;
            println!("backup verified: {} files", manifest.files.len());
            Ok(true)
        }
        "verify-backup" if args.len() == 2 => {
            let manifest = aelio_server::backup::verify_backup(&path(1)?)?;
            println!("backup valid: {} files", manifest.files.len());
            Ok(true)
        }
        "restore" if args.len() == 3 => {
            let manifest = aelio_server::backup::restore_backup(&path(1)?, &path(2)?)?;
            println!("restore complete: {} files", manifest.files.len());
            Ok(true)
        }
        "backup" | "verify-backup" | "restore" => Err(
            "usage: aelio-server backup <stopped-data-dir> <new-backup-dir> | verify-backup <backup-dir> | restore <backup-dir> <new-data-dir>"
                .into(),
        ),
        _ => Ok(false),
    }
}
