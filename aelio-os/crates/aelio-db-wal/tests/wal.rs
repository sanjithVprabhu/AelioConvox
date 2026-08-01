//! WAL durability + crash-safe replay.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use aelio_db_wal::{replay, Record, RecordType, Wal};

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct TempPath(PathBuf);
impl TempPath {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        TempPath(std::env::temp_dir().join(format!("aelio_db_wal_{}_{n}.log", std::process::id())))
    }
}
impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn append_and_replay_roundtrip() {
    let p = TempPath::new();
    {
        let mut wal = Wal::create(&p.0, 100).unwrap();
        let l0 = wal.append(1, RecordType::InsertRow, b"row-a").unwrap();
        let l1 = wal.append(1, RecordType::InsertRow, b"row-b").unwrap();
        let l2 = wal.commit(1).unwrap();
        assert_eq!((l0, l1, l2), (100, 101, 102));
        assert_eq!(wal.next_lsn(), 103);
    }

    let r = replay(&p.0).unwrap();
    assert_eq!(r.base_lsn, 100);
    assert_eq!(r.records.len(), 3);
    assert_eq!(
        r.records[0],
        Record {
            lsn: 100,
            txn_id: 1,
            rtype: RecordType::InsertRow,
            payload: b"row-a".to_vec()
        }
    );
    assert_eq!(r.records[2].rtype, RecordType::Commit);
    // LSNs are monotonic.
    assert!(r.records.windows(2).all(|w| w[0].lsn < w[1].lsn));
}

#[test]
fn replay_stops_at_torn_tail() {
    let p = TempPath::new();
    {
        let mut wal = Wal::create(&p.0, 0).unwrap();
        wal.append(7, RecordType::InsertRow, b"good-1").unwrap();
        wal.append(7, RecordType::InsertRow, b"good-2").unwrap();
        wal.sync().unwrap();
    }
    // Simulate a partially-written trailing record (crash mid-append).
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p.0).unwrap();
        f.write_all(&[0xFF, 0x00, 0x00, 0x00, 0x01, 0x02]).unwrap(); // bogus len + short body
    }

    let r = replay(&p.0).unwrap();
    assert_eq!(r.records.len(), 2, "valid prefix must survive a torn tail");
    assert_eq!(r.records[1].payload, b"good-2");
}

#[test]
fn replay_stops_at_first_crc_mismatch() {
    let p = TempPath::new();
    {
        let mut wal = Wal::create(&p.0, 0).unwrap();
        wal.append(1, RecordType::InsertRow, b"first").unwrap();
        wal.append(1, RecordType::InsertRow, b"second").unwrap();
        wal.append(1, RecordType::InsertRow, b"third").unwrap();
        wal.sync().unwrap();
    }
    // Corrupt a byte inside the SECOND record's body. Header is 16 bytes; record 0 frame =
    // 4(len) + (8+8+1+5) + 4(crc) = 30 bytes. Flip a byte well inside record 1.
    let mut bytes = std::fs::read(&p.0).unwrap();
    let rec1_body = 16 + 30 + 4 + 8; // header + rec0 + len prefix + into rec1 body
    bytes[rec1_body] ^= 0xFF;
    std::fs::write(&p.0, &bytes).unwrap();

    let r = replay(&p.0).unwrap();
    assert_eq!(
        r.records.len(),
        1,
        "replay must stop at the first corrupt record"
    );
    assert_eq!(r.records[0].payload, b"first");
}

#[test]
fn open_append_truncates_torn_tail_and_continues() {
    use std::io::Write;
    let p = TempPath::new();
    {
        let mut w = Wal::create(&p.0, 0).unwrap();
        w.append(1, RecordType::InsertRow, b"a").unwrap();
        w.append(1, RecordType::InsertRow, b"b").unwrap();
        w.sync().unwrap();
    }
    // Append a torn trailing record (crash mid-write).
    {
        let mut f = std::fs::OpenOptions::new().append(true).open(&p.0).unwrap();
        f.write_all(&[0xFF, 0x00, 0x00, 0x00, 0x01]).unwrap();
    }
    // Reopen: the torn tail is truncated, appends continue cleanly.
    {
        let (mut w, r) = Wal::open_append(&p.0).unwrap();
        assert_eq!(r.records.len(), 2);
        assert_eq!(w.next_lsn(), 2);
        w.append(1, RecordType::InsertRow, b"c").unwrap();
        w.sync().unwrap();
    }
    let r = replay(&p.0).unwrap();
    assert_eq!(r.records.len(), 3, "garbage gone, new record appended");
    assert_eq!(r.records[2].payload, b"c");
}

#[test]
fn empty_wal_replays_to_nothing() {
    let p = TempPath::new();
    Wal::create(&p.0, 5).unwrap();
    let r = replay(&p.0).unwrap();
    assert_eq!(r.base_lsn, 5);
    assert!(r.records.is_empty());
}
