//! WAL record types and on-disk framing.
//!
//! Logical logging (spec Part XVI): records describe *what changed*, not page edits. Each
//! record is framed `[len u32][body][crc32c u32]` where `body = lsn||txn_id||rtype||payload`
//! and the CRC covers the body. On replay a record whose frame is incomplete (torn tail)
//! or whose CRC fails terminates the log — by construction it must be the last record.

use aelio_db_format::crc32c;

/// What a WAL record represents. On-disk tags are stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RecordType {
    InsertRow = 1,
    UpdateRow = 2,
    DeleteRow = 3,
    Commit = 4,
    Abort = 5,
    Checkpoint = 6,
}

impl RecordType {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => RecordType::InsertRow,
            2 => RecordType::UpdateRow,
            3 => RecordType::DeleteRow,
            4 => RecordType::Commit,
            5 => RecordType::Abort,
            6 => RecordType::Checkpoint,
            _ => return None,
        })
    }
}

/// One logical WAL record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub lsn: u64,
    pub txn_id: u64,
    pub rtype: RecordType,
    pub payload: Vec<u8>,
}

impl Record {
    /// Serialize to the framed on-disk form.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(17 + self.payload.len());
        body.extend_from_slice(&self.lsn.to_le_bytes());
        body.extend_from_slice(&self.txn_id.to_le_bytes());
        body.push(self.rtype as u8);
        body.extend_from_slice(&self.payload);

        let crc = crc32c(&body);
        let len = (body.len() + 4) as u32; // body + crc
        let mut out = Vec::with_capacity(4 + len as usize);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// Try to decode one record at `pos` in `buf`. Returns `Some((record, next_pos))` on a
    /// complete, CRC-valid record, or `None` at end-of-log / torn tail / CRC failure
    /// (replay should stop). A returned `None` is not an error — it is the log's end.
    pub fn decode_at(buf: &[u8], pos: usize) -> Option<(Record, usize)> {
        if pos + 4 > buf.len() {
            return None; // no length prefix → clean EOF
        }
        let len = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap()) as usize;
        let frame_start = pos + 4;
        let frame_end = frame_start.checked_add(len)?;
        if frame_end > buf.len() || len < 4 + 17 {
            return None; // torn tail (truncated frame) or impossibly small
        }
        let body = &buf[frame_start..frame_end - 4];
        let crc = u32::from_le_bytes(buf[frame_end - 4..frame_end].try_into().unwrap());
        if crc32c(body) != crc {
            return None; // corrupt tail → stop
        }
        let lsn = u64::from_le_bytes(body[0..8].try_into().unwrap());
        let txn_id = u64::from_le_bytes(body[8..16].try_into().unwrap());
        let rtype = RecordType::from_u8(body[16])?;
        let payload = body[17..].to_vec();
        Some((
            Record {
                lsn,
                txn_id,
                rtype,
                payload,
            },
            frame_end,
        ))
    }
}
