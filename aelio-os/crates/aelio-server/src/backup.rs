//! Offline, whole-root backup and restore for Aelio's single Rust durability plane.
//!
//! Operators must stop the server before invoking these functions. A snapshot contains the
//! runtime store, database segments/catalog/WAL, agent memory, artifact history, ledgers and
//! continuation pins because all of them live below the configured `AELIO_DATA_DIR`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MANIFEST: &str = "manifest.json";
const PAYLOAD: &str = "data";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupManifest {
    pub format: String,
    pub created_unix_ms: u128,
    pub files: BTreeMap<String, BackupFile>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupFile {
    pub bytes: u64,
    pub blake3: String,
}

pub fn create_backup(source: &Path, destination: &Path) -> io::Result<BackupManifest> {
    if !source.is_dir() {
        return Err(invalid("backup source must be an existing directory"));
    }
    if destination.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "backup destination must not already exist",
        ));
    }
    reject_nested(source, destination)?;
    fs::create_dir_all(destination.join(PAYLOAD))?;
    let mut files = BTreeMap::new();
    for (relative, absolute) in regular_files(source)? {
        let target = destination.join(PAYLOAD).join(&relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&absolute, &target)?;
        sync_file(&target)?;
        let (bytes, blake3) = hash_file(&target)?;
        files.insert(path_key(&relative)?, BackupFile { bytes, blake3 });
    }
    let manifest = BackupManifest {
        format: "aelio-backup/1".into(),
        created_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| invalid("system clock is before Unix epoch"))?
            .as_millis(),
        files,
    };
    let encoded = serde_json::to_vec_pretty(&manifest).map_err(invalid)?;
    fs::write(destination.join(MANIFEST), encoded)?;
    sync_file(&destination.join(MANIFEST))?;
    sync_dir(destination)?;
    Ok(manifest)
}

pub fn verify_backup(backup: &Path) -> io::Result<BackupManifest> {
    let encoded = fs::read(backup.join(MANIFEST))?;
    let manifest: BackupManifest = serde_json::from_slice(&encoded).map_err(invalid)?;
    if manifest.format != "aelio-backup/1" {
        return Err(invalid("unsupported backup format"));
    }
    let payload = backup.join(PAYLOAD);
    let actual: BTreeMap<_, _> = regular_files(&payload)?
        .into_iter()
        .map(|(relative, absolute)| Ok((path_key(&relative)?, absolute)))
        .collect::<io::Result<_>>()?;
    if actual.len() != manifest.files.len() || actual.keys().ne(manifest.files.keys()) {
        return Err(invalid("backup payload file set does not match manifest"));
    }
    for (name, expected) in &manifest.files {
        let (bytes, digest) = hash_file(&actual[name])?;
        if bytes != expected.bytes || digest != expected.blake3 {
            return Err(invalid(format!("backup checksum mismatch for {name}")));
        }
    }
    Ok(manifest)
}

pub fn restore_backup(backup: &Path, target: &Path) -> io::Result<BackupManifest> {
    if target.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "restore target must not already exist",
        ));
    }
    let manifest = verify_backup(backup)?;
    let parent = target
        .parent()
        .ok_or_else(|| invalid("restore target must have a parent directory"))?;
    fs::create_dir_all(parent)?;
    let name = target
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| invalid("restore target name is invalid"))?;
    let staging = parent.join(format!(".{name}.restore-{}", std::process::id()));
    if staging.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "restore staging directory already exists",
        ));
    }
    fs::create_dir(&staging)?;
    for (relative, absolute) in regular_files(&backup.join(PAYLOAD))? {
        let destination = staging.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(absolute, &destination)?;
        sync_file(&destination)?;
    }
    sync_dir(&staging)?;
    fs::rename(&staging, target)?;
    sync_dir(parent)?;
    Ok(manifest)
}

fn regular_files(root: &Path) -> io::Result<Vec<(PathBuf, PathBuf)>> {
    let mut pending = vec![PathBuf::new()];
    let mut files = Vec::new();
    while let Some(relative) = pending.pop() {
        let directory = root.join(&relative);
        let mut entries = fs::read_dir(&directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries.into_iter().rev() {
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                return Err(invalid("backup roots may not contain symbolic links"));
            }
            let child = relative.join(entry.file_name());
            if file_type.is_dir() {
                pending.push(child);
            } else if file_type.is_file() {
                files.push((child, entry.path()));
            } else {
                return Err(invalid(
                    "backup roots may contain only files and directories",
                ));
            }
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

fn hash_file(path: &Path) -> io::Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or_else(|| invalid("backup file size overflow"))?;
        hasher.update(&buffer[..read]);
    }
    Ok((bytes, hasher.finalize().to_hex().to_string()))
}

fn path_key(path: &Path) -> io::Result<String> {
    let value = path
        .to_str()
        .ok_or_else(|| invalid("backup paths must be valid UTF-8"))?;
    if value.is_empty() || path.is_absolute() || value.split('/').any(|part| part == "..") {
        return Err(invalid("backup path is not a safe relative path"));
    }
    Ok(value.replace('\\', "/"))
}

fn reject_nested(source: &Path, destination: &Path) -> io::Result<()> {
    let source = fs::canonicalize(source)?;
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("backup destination must have a parent"))?;
    fs::create_dir_all(parent)?;
    let destination = fs::canonicalize(parent)?.join(
        destination
            .file_name()
            .ok_or_else(|| invalid("backup destination name is invalid"))?,
    );
    if destination.starts_with(source) {
        return Err(invalid("backup destination must be outside the data root"));
    }
    Ok(())
}

fn sync_file(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}
