//! The fixed 64-byte file preamble (region [0]). See `TechSpec/LL_FileFormat_ByteLayout.md` §1.
//!
//! Identification/sanity only; the footer is authoritative for everything it duplicates.

use crate::error::{FormatError, Result};

/// Magic number bracketing the file: on-disk bytes `VSS1` — the VSS (Versioned Storage
/// Substrate) format that LL writes, with extension `.vss`.
pub const MAGIC: u32 = u32::from_le_bytes(*b"VSS1");
/// On-disk format version this crate writes and can read.
pub const FORMAT_VERSION: u16 = 1;
/// Preamble size in bytes (fixed).
pub const PREAMBLE_LEN: usize = 64;

/// `preamble_flags` bit0: little-endian on-disk (always set in v1).
const FLAG_LITTLE_ENDIAN: u16 = 0x0001;

/// The fixed-size file header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preamble {
    pub file_uuid: [u8; 16],
    pub min_lsn: u64,
    pub max_lsn: u64,
    pub row_count: u64,
    pub schema_fingerprint: u64,
    pub creation_unix_nanos: u64,
}

impl Preamble {
    /// Serialize to exactly [`PREAMBLE_LEN`] bytes.
    pub fn encode(&self) -> [u8; PREAMBLE_LEN] {
        let mut b = [0u8; PREAMBLE_LEN];
        b[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        b[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        b[6..8].copy_from_slice(&FLAG_LITTLE_ENDIAN.to_le_bytes());
        b[8..24].copy_from_slice(&self.file_uuid);
        b[24..32].copy_from_slice(&self.min_lsn.to_le_bytes());
        b[32..40].copy_from_slice(&self.max_lsn.to_le_bytes());
        b[40..48].copy_from_slice(&self.row_count.to_le_bytes());
        b[48..56].copy_from_slice(&self.schema_fingerprint.to_le_bytes());
        b[56..64].copy_from_slice(&self.creation_unix_nanos.to_le_bytes());
        b
    }

    /// Parse from the first [`PREAMBLE_LEN`] bytes of a file, validating magic/version.
    pub fn decode(buf: &[u8]) -> Result<Self> {
        if buf.len() < PREAMBLE_LEN {
            return Err(FormatError::Truncated("preamble"));
        }
        let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        if magic != MAGIC {
            return Err(FormatError::BadMagic {
                expected: MAGIC,
                found: magic,
            });
        }
        let version = u16::from_le_bytes(buf[4..6].try_into().unwrap());
        if version != FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion(version));
        }
        let flags = u16::from_le_bytes(buf[6..8].try_into().unwrap());
        if flags & FLAG_LITTLE_ENDIAN == 0 {
            return Err(FormatError::Decode(
                "big-endian files are not supported".into(),
            ));
        }
        let mut file_uuid = [0u8; 16];
        file_uuid.copy_from_slice(&buf[8..24]);
        Ok(Preamble {
            file_uuid,
            min_lsn: u64::from_le_bytes(buf[24..32].try_into().unwrap()),
            max_lsn: u64::from_le_bytes(buf[32..40].try_into().unwrap()),
            row_count: u64::from_le_bytes(buf[40..48].try_into().unwrap()),
            schema_fingerprint: u64::from_le_bytes(buf[48..56].try_into().unwrap()),
            creation_unix_nanos: u64::from_le_bytes(buf[56..64].try_into().unwrap()),
        })
    }
}
