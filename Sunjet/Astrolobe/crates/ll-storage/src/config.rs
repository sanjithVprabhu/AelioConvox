//! Configuration for segment storage — entirely from environment variables.

use std::env;
use std::io;
use std::path::PathBuf;

/// Where durable `.vss` segments live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentBackend {
    /// Segments only under `LL_DATA_DIR` (default).
    Local,
    /// S3-compatible object store + local write-through cache.
    S3,
}

impl SegmentBackend {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "local" | "disk" | "fs" => Some(Self::Local),
            "s3" | "minio" | "r2" | "b2" | "cloud" => Some(Self::S3),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StorageConfig {
    pub backend: SegmentBackend,
    pub data_dir: PathBuf,
    /// Object-key prefix inside the bucket (e.g. `prod/aelio/`).
    pub prefix: String,
    pub s3_bucket: Option<String>,
    pub s3_region: Option<String>,
    pub s3_endpoint: Option<String>,
    pub s3_access_key_id: Option<String>,
    pub s3_secret_access_key: Option<String>,
    /// Allow `http://` endpoints (MinIO local).
    pub s3_allow_http: bool,
    /// Path-style addressing (required for many MinIO setups).
    pub s3_path_style: bool,
}

impl StorageConfig {
    /// Load from process env. Accepts both `LL_*` (ll-server) and `AELIO_SUNJET_*`
    /// (Aelio deployment) names — Aelio forwards its env into the ll-server process.
    pub fn from_env(data_dir: PathBuf) -> io::Result<Self> {
        let backend_raw = first_env(&[
            "LL_SEGMENT_BACKEND",
            "AELIO_SUNJET_SEGMENT_BACKEND",
        ])
        .unwrap_or_else(|| "local".into());

        let backend = SegmentBackend::parse(&backend_raw).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "unknown segment backend '{backend_raw}' — use local|s3"
                ),
            )
        })?;

        let prefix = first_env(&["LL_SEGMENT_PREFIX", "AELIO_SUNJET_SEGMENT_PREFIX"])
            .unwrap_or_default();
        let prefix = normalize_prefix(&prefix);

        let s3_bucket = first_env(&["LL_S3_BUCKET", "AELIO_SUNJET_S3_BUCKET"]);
        let s3_region = first_env(&["LL_S3_REGION", "AELIO_SUNJET_S3_REGION"]);
        let s3_endpoint = first_env(&["LL_S3_ENDPOINT", "AELIO_SUNJET_S3_ENDPOINT"]);
        let s3_access_key_id = first_env(&[
            "LL_S3_ACCESS_KEY_ID",
            "AELIO_SUNJET_S3_ACCESS_KEY_ID",
            "AWS_ACCESS_KEY_ID",
        ]);
        let s3_secret_access_key = first_env(&[
            "LL_S3_SECRET_ACCESS_KEY",
            "AELIO_SUNJET_S3_SECRET_ACCESS_KEY",
            "AWS_SECRET_ACCESS_KEY",
        ]);

        let s3_allow_http = flag_env(&[
            "LL_S3_ALLOW_HTTP",
            "AELIO_SUNJET_S3_ALLOW_HTTP",
        ]);
        let s3_path_style = flag_env(&[
            "LL_S3_PATH_STYLE",
            "AELIO_SUNJET_S3_PATH_STYLE",
        ]);

        if backend == SegmentBackend::S3 && s3_bucket.as_ref().map(|s| s.is_empty()).unwrap_or(true)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "S3 segment backend requires LL_S3_BUCKET or AELIO_SUNJET_S3_BUCKET",
            ));
        }

        Ok(Self {
            backend,
            data_dir,
            prefix,
            s3_bucket,
            s3_region,
            s3_endpoint,
            s3_access_key_id,
            s3_secret_access_key,
            s3_allow_http,
            s3_path_style,
        })
    }
}

/// Convenience: `StorageConfig::from_env` + `build_store`.
pub fn from_env(data_dir: PathBuf) -> io::Result<(StorageConfig, std::sync::Arc<dyn crate::SegmentStore>)> {
    let config = StorageConfig::from_env(data_dir)?;
    let store = crate::build_store(&config)?;
    Ok((config, store))
}

fn first_env(keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Ok(v) = env::var(key) {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn flag_env(keys: &[&str]) -> bool {
    first_env(keys)
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

fn normalize_prefix(p: &str) -> String {
    let mut s = p.trim().trim_matches('/').to_string();
    if !s.is_empty() && !s.ends_with('/') {
        s.push('/');
    }
    s
}
