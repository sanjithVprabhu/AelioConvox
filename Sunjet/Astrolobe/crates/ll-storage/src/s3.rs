//! S3-compatible object store for `.vss` segments (AWS S3, MinIO, R2, B2, …).

use std::io;
use std::path::Path;
use std::sync::Arc;

use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjPath;
use object_store::{ObjectStore, PutPayload};

use crate::config::StorageConfig;
use crate::runtime::block_on;
use crate::SegmentStore;

pub struct S3SegmentStore {
    store: Arc<dyn ObjectStore>,
    prefix: String,
}

impl S3SegmentStore {
    pub fn from_config(config: &StorageConfig) -> io::Result<Self> {
        let bucket = config
            .s3_bucket
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing S3 bucket"))?;

        let mut builder = AmazonS3Builder::new()
            .with_bucket_name(bucket)
            .with_region(
                config
                    .s3_region
                    .clone()
                    .unwrap_or_else(|| "us-east-1".into()),
            );

        if let Some(endpoint) = &config.s3_endpoint {
            builder = builder.with_endpoint(endpoint.clone());
        }
        if let Some(key) = &config.s3_access_key_id {
            builder = builder.with_access_key_id(key.clone());
        }
        if let Some(secret) = &config.s3_secret_access_key {
            builder = builder.with_secret_access_key(secret.clone());
        }
        if config.s3_allow_http {
            builder = builder.with_allow_http(true);
        }
        if config.s3_path_style {
            builder = builder.with_virtual_hosted_style_request(false);
        }

        let store = builder
            .build()
            .map_err(|e| io::Error::other(format!("S3 client build failed: {e}")))?;

        Ok(Self {
            store: Arc::new(store),
            prefix: config.prefix.clone(),
        })
    }

    fn key(&self, name: &str) -> ObjPath {
        ObjPath::from(format!("{}{name}", self.prefix))
    }
}

impl SegmentStore for S3SegmentStore {
    fn publish(&self, name: &str, local_path: &Path) -> io::Result<()> {
        let bytes = std::fs::read(local_path)?;
        let path = self.key(name);
        let store = Arc::clone(&self.store);
        block_on(async move {
            store
                .put(&path, PutPayload::from(bytes))
                .await
                .map_err(|e| io::Error::other(format!("S3 put {path}: {e}")))
        })?;
        Ok(())
    }

    fn ensure_local(&self, name: &str, local_path: &Path) -> io::Result<()> {
        if local_path.exists() {
            return Ok(());
        }
        let path = self.key(name);
        let store = Arc::clone(&self.store);
        let bytes = block_on(async move {
            let result = store
                .get(&path)
                .await
                .map_err(|e| io::Error::other(format!("S3 get {path}: {e}")))?;
            result
                .bytes()
                .await
                .map_err(|e| io::Error::other(format!("S3 read body {path}: {e}")))
        })?;

        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = local_path.with_extension("download.tmp");
        std::fs::write(&tmp, &bytes)?;
        if let Ok(f) = std::fs::File::open(&tmp) {
            let _ = f.sync_all();
        }
        std::fs::rename(&tmp, local_path)?;
        Ok(())
    }

    fn remove(&self, name: &str, _local_path: &Path) -> io::Result<()> {
        let path = self.key(name);
        let store = Arc::clone(&self.store);
        block_on(async move {
            match store.delete(&path).await {
                Ok(()) => Ok(()),
                // S3 delete is often idempotent; treat not-found as success.
                Err(object_store::Error::NotFound { .. }) => Ok(()),
                Err(e) => Err(io::Error::other(format!("S3 delete {path}: {e}"))),
            }
        })
    }

    fn backend_name(&self) -> &'static str {
        "s3"
    }
}
