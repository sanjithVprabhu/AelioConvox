//! In-memory segment store — used in tests (and as a stand-in for "cloud" without MinIO).

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::Mutex;

use crate::SegmentStore;

#[derive(Debug, Default)]
pub struct MemorySegmentStore {
    objects: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemorySegmentStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.objects.lock().map(|m| m.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains(&self, name: &str) -> bool {
        self.objects
            .lock()
            .map(|m| m.contains_key(name))
            .unwrap_or(false)
    }
}

impl SegmentStore for MemorySegmentStore {
    fn publish(&self, name: &str, local_path: &Path) -> io::Result<()> {
        let bytes = std::fs::read(local_path)?;
        self.objects
            .lock()
            .map_err(|_| io::Error::other("memory store lock poisoned"))?
            .insert(name.to_string(), bytes);
        Ok(())
    }

    fn ensure_local(&self, name: &str, local_path: &Path) -> io::Result<()> {
        if local_path.exists() {
            return Ok(());
        }
        let bytes = self
            .objects
            .lock()
            .map_err(|_| io::Error::other("memory store lock poisoned"))?
            .get(name)
            .cloned()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("memory segment missing: {name}"))
            })?;
        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(local_path, bytes)?;
        Ok(())
    }

    fn remove(&self, name: &str, local_path: &Path) -> io::Result<()> {
        let _ = std::fs::remove_file(local_path);
        if let Ok(mut m) = self.objects.lock() {
            m.remove(name);
        }
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "memory"
    }
}
