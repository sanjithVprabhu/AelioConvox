//! `aelio-db-storage` — durable backends for immutable Aelio `.vss` segments.
//!
//! ## What lives where
//!
//! | Artifact | Location | Why |
//! |----------|----------|-----|
//! | `wal.log` | **Always local** | Append + fsync latency; not object-store friendly |
//! | `manifest.bin` | **Always local** | Crash-critical atomic commit of the segment set |
//! | `catalog.bin` | **Always local** | Tiny schema; needed before any segment fetch |
//! | `seg-*.vss` | Local **and/or** cloud | Immutable blobs with embedded HNSW/BM25/edge indexes |
//!
//! ## Backends
//!
//! - [`LocalSegmentStore`] — the on-disk layout under `AELIO_DATA_DIR`
//! - [`S3SegmentStore`] — S3-compatible (AWS, MinIO, R2, Backblaze B2, …)
//! - [`CachedSegmentStore`] — write-through: publish to both; open/search from local
//!   cache, downloading from cloud when a segment is missing locally
//!
//! ## Crash order (unchanged)
//!
//! 1. Write+fsync segment (local staging)
//! 2. [`SegmentStore::publish`] to cloud (if configured)
//! 3. Atomic manifest swing
//! 4. WAL checkpoint
//! 5. On compact: [`SegmentStore::remove`] pruned segments (local + cloud)

mod config;
mod local;
mod memory;
mod runtime;
mod s3;

pub use config::{from_env, SegmentBackend, StorageConfig};
pub use local::LocalSegmentStore;
pub use memory::MemorySegmentStore;
pub use s3::S3SegmentStore;

use std::io;
use std::path::Path;
use std::sync::Arc;

/// Durable store for immutable segment blobs referenced by basename (`seg-00001.vss`).
pub trait SegmentStore: Send + Sync {
    /// After a segment has been written+fsynced at `local_path`, publish it under `name`.
    /// Local-only backends may treat this as a no-op (file already at the right path).
    fn publish(&self, name: &str, local_path: &Path) -> io::Result<()>;

    /// Ensure `name` is present at `local_path` (download from remote if needed).
    fn ensure_local(&self, name: &str, local_path: &Path) -> io::Result<()>;

    /// Remove a segment that is no longer in the manifest (local + remote).
    /// Missing objects are not an error.
    fn remove(&self, name: &str, local_path: &Path) -> io::Result<()>;

    /// Human-readable backend label for logs / health.
    fn backend_name(&self) -> &'static str;
}

/// Write-through cache: every publish hits local disk AND the remote store;
/// opens prefer the local file and fall back to downloading.
pub struct CachedSegmentStore {
    local: LocalSegmentStore,
    remote: Arc<dyn SegmentStore>,
}

impl CachedSegmentStore {
    pub fn new(local: LocalSegmentStore, remote: Arc<dyn SegmentStore>) -> Self {
        Self { local, remote }
    }
}

impl SegmentStore for CachedSegmentStore {
    fn publish(&self, name: &str, local_path: &Path) -> io::Result<()> {
        // Local file is already the staging write from Database::flush; ensure it's the
        // canonical local copy, then push to cloud before the caller swings the manifest.
        self.local.publish(name, local_path)?;
        self.remote.publish(name, local_path)?;
        Ok(())
    }

    fn ensure_local(&self, name: &str, local_path: &Path) -> io::Result<()> {
        if local_path.exists() {
            return Ok(());
        }
        self.remote.ensure_local(name, local_path)
    }

    fn remove(&self, name: &str, local_path: &Path) -> io::Result<()> {
        // Best-effort both sides; never fail the compact if only one side is gone.
        let _ = self.local.remove(name, local_path);
        let _ = self.remote.remove(name, local_path);
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "s3-cached"
    }
}

/// Build the store selected by [`StorageConfig`].
pub fn build_store(config: &StorageConfig) -> io::Result<Arc<dyn SegmentStore>> {
    match config.backend {
        SegmentBackend::Local => Ok(Arc::new(LocalSegmentStore::new(&config.data_dir))),
        SegmentBackend::S3 => {
            let remote = S3SegmentStore::from_config(config)?;
            let local = LocalSegmentStore::new(&config.data_dir);
            Ok(Arc::new(CachedSegmentStore::new(local, Arc::new(remote))))
        }
    }
}
