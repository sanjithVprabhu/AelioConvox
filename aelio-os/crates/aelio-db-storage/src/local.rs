//! Local filesystem segment store — the default / always-present cache root.

use std::io;
use std::path::{Path, PathBuf};

use crate::SegmentStore;

#[derive(Debug, Clone)]
pub struct LocalSegmentStore {
    dir: PathBuf,
}

impl LocalSegmentStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl SegmentStore for LocalSegmentStore {
    fn publish(&self, name: &str, local_path: &Path) -> io::Result<()> {
        // Database writes to `dir/name` already; if the caller used a different
        // staging path, copy into place.
        let dest = self.dir.join(name);
        if local_path != dest.as_path() {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(local_path, &dest)?;
            // Best-effort durability for the local copy.
            if let Ok(f) = std::fs::File::open(&dest) {
                let _ = f.sync_all();
            }
        }
        Ok(())
    }

    fn ensure_local(&self, name: &str, local_path: &Path) -> io::Result<()> {
        let src = self.dir.join(name);
        if local_path.exists() {
            return Ok(());
        }
        if src.exists() && src != local_path {
            if let Some(parent) = local_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&src, local_path)?;
            return Ok(());
        }
        if !src.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("local segment missing: {name}"),
            ));
        }
        Ok(())
    }

    fn remove(&self, name: &str, local_path: &Path) -> io::Result<()> {
        let _ = std::fs::remove_file(local_path);
        let canonical = self.dir.join(name);
        if canonical != local_path {
            let _ = std::fs::remove_file(&canonical);
        }
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "local"
    }
}
