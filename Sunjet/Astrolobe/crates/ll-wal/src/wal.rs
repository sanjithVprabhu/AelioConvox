//! The write-ahead log: a single LSN-ordered append log with fsync-on-commit durability
//! and crash-safe replay.
//!
//! v0 scope: one WAL file, append + explicit fsync, full replay, and post-flush checkpoint
//! truncation ([`Wal::checkpoint_truncate`]). Group commit and file rotation are later
//! additions.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::record::{Record, RecordType};

/// File header magic: on-disk bytes `VWAL`.
pub const WAL_MAGIC: u32 = u32::from_le_bytes(*b"VWAL");
pub const WAL_VERSION: u16 = 1;
const HEADER_LEN: usize = 16; // magic(4) + version(2) + reserved(2) + base_lsn(8)

/// An append handle to a WAL file.
pub struct Wal {
    file: File,
    next_lsn: u64,
}

impl Wal {
    /// Create a fresh WAL at `path`, writing the header. LSNs begin at `base_lsn`.
    pub fn create<P: AsRef<Path>>(path: P, base_lsn: u64) -> io::Result<Self> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .read(true)
            .open(path)?;
        let mut header = Vec::with_capacity(HEADER_LEN);
        header.extend_from_slice(&WAL_MAGIC.to_le_bytes());
        header.extend_from_slice(&WAL_VERSION.to_le_bytes());
        header.extend_from_slice(&[0u8; 2]); // reserved
        header.extend_from_slice(&base_lsn.to_le_bytes());
        debug_assert_eq!(header.len(), HEADER_LEN);
        file.write_all(&header)?;
        file.sync_all()?;
        Ok(Wal {
            file,
            next_lsn: base_lsn,
        })
    }

    /// Next LSN that will be assigned.
    pub fn next_lsn(&self) -> u64 {
        self.next_lsn
    }

    /// Append a record, assigning it the next LSN. Does **not** fsync — call [`Wal::sync`]
    /// (or [`Wal::commit`]) at the durability point.
    pub fn append(&mut self, txn_id: u64, rtype: RecordType, payload: &[u8]) -> io::Result<u64> {
        let lsn = self.next_lsn;
        let record = Record {
            lsn,
            txn_id,
            rtype,
            payload: payload.to_vec(),
        };
        self.file.write_all(&record.encode())?;
        self.next_lsn += 1;
        Ok(lsn)
    }

    /// Flush to durable storage (the durability moment for prior appends).
    pub fn sync(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }

    /// Append a `Commit` record for `txn_id` and fsync. Returns the commit LSN.
    pub fn commit(&mut self, txn_id: u64) -> io::Result<u64> {
        let lsn = self.append(txn_id, RecordType::Commit, &[])?;
        self.sync()?;
        Ok(lsn)
    }

    /// Truncate the log to empty and restart LSNs at `base_lsn`, fsynced. Called after a
    /// flush makes every prior record durable in a segment, so the WAL only needs to retain
    /// writes that happen afterward. `base_lsn` must exceed every LSN already assigned, to
    /// keep LSNs globally monotonic (the caller passes the post-flush [`Wal::next_lsn`]).
    pub fn checkpoint_truncate(&mut self, base_lsn: u64) -> io::Result<()> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        let mut header = Vec::with_capacity(HEADER_LEN);
        header.extend_from_slice(&WAL_MAGIC.to_le_bytes());
        header.extend_from_slice(&WAL_VERSION.to_le_bytes());
        header.extend_from_slice(&[0u8; 2]); // reserved
        header.extend_from_slice(&base_lsn.to_le_bytes());
        self.file.write_all(&header)?;
        self.file.sync_all()?;
        self.next_lsn = base_lsn;
        Ok(())
    }
}

/// Result of replaying a WAL file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replay {
    pub base_lsn: u64,
    pub records: Vec<Record>,
    /// Byte length of the valid prefix (end of the last good record). Bytes beyond this
    /// are a torn/corrupt tail and should be truncated before appending.
    pub valid_len: u64,
}

/// Replay a WAL file: validate the header, then return every complete CRC-valid record up
/// to (but not including) the first torn/corrupt one (the crash point).
pub fn replay<P: AsRef<Path>>(path: P) -> io::Result<Replay> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    if bytes.len() < HEADER_LEN {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "WAL shorter than header"));
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    if magic != WAL_MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad WAL magic"));
    }
    let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
    if version != WAL_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported WAL version {version}"),
        ));
    }
    let base_lsn = u64::from_le_bytes(bytes[8..16].try_into().unwrap());

    let mut records = Vec::new();
    let mut pos = HEADER_LEN;
    while let Some((rec, next)) = Record::decode_at(&bytes, pos) {
        records.push(rec);
        pos = next;
    }
    Ok(Replay {
        base_lsn,
        records,
        valid_len: pos as u64,
    })
}

impl Wal {
    /// Reopen an existing WAL for appending after recovery: replay it, truncate any
    /// torn/corrupt tail to the valid prefix, and continue at `last_lsn + 1`.
    pub fn open_append<P: AsRef<Path>>(path: P) -> io::Result<(Self, Replay)> {
        let r = replay(&path)?;
        // Truncate any trailing garbage so new records append cleanly after valid data.
        {
            let f = OpenOptions::new().write(true).open(&path)?;
            f.set_len(r.valid_len)?;
            f.sync_all()?;
        }
        let file = OpenOptions::new().append(true).read(true).open(&path)?;
        let next_lsn = r
            .records
            .last()
            .map(|rec| rec.lsn + 1)
            .unwrap_or(r.base_lsn);
        Ok((Wal { file, next_lsn }, r))
    }
}
