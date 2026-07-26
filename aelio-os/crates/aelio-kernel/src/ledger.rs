//! Turn ledger (§12.2, App G): append-only per flow instance; `seq` monotonic + gapless;
//! per-instance BLAKE3 hash chain (tamper-evident, §28). Replay consumes entries in `seq` order
//! (§12.3): INJECT = write payload in; VERIFY = recompute + compare, mismatch ⇒ hard refuse.

use aelio_sol::{value_hash, SolValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Replay consumes the payload instead of acting (LLM output, tool result, now/uuid, …).
    Inject,
    /// Replay recomputes deterministically and compares; mismatch ⇒ hard refuse.
    Verify,
    /// Neither consumed nor verified for determinism (informational / init).
    Info,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub seq: u64,
    pub turn_id: String,
    pub nid: Option<String>,
    pub kind: String,
    pub category: Category,
    pub payload: SolValue,
    pub payload_hash: String,
    /// Chain over the previous entry (envelope+payload); empty for seq 0.
    pub prev: String,
}

#[derive(Debug, Default, Clone)]
pub struct Ledger {
    entries: Vec<Entry>,
}

impl Ledger {
    pub fn append(
        &mut self,
        turn_id: &str,
        nid: Option<&str>,
        kind: &str,
        category: Category,
        payload: SolValue,
    ) -> &Entry {
        let seq = self.entries.len() as u64;
        let payload_hash = value_hash(&payload);
        let prev = self.entries.last().map(chain_hash).unwrap_or_default();
        self.entries.push(Entry {
            seq,
            turn_id: turn_id.to_string(),
            nid: nid.map(str::to_string),
            kind: kind.to_string(),
            category,
            payload,
            payload_hash,
            prev,
        });
        self.entries.last().unwrap()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Verify the chain is gapless (seq 0..n) and each `prev` matches — a test, not a comment
    /// (handoff Working rules). Returns the first offending seq on failure.
    pub fn verify_chain(&self) -> Result<(), u64> {
        let mut prev = String::new();
        for (i, e) in self.entries.iter().enumerate() {
            if e.seq != i as u64 {
                return Err(e.seq);
            }
            if e.prev != prev {
                return Err(e.seq);
            }
            prev = chain_hash(e);
        }
        Ok(())
    }
}

fn chain_hash(e: &Entry) -> String {
    // Hash the envelope identity + payload_hash + prev to chain (App G `prev` intent).
    let env = SolValue::map([
        ("seq", SolValue::Int(e.seq as i64)),
        ("turn_id", SolValue::str(e.turn_id.clone())),
        ("nid", e.nid.clone().map(SolValue::Str).unwrap_or(SolValue::Null)),
        ("kind", SolValue::str(e.kind.clone())),
        ("payload_hash", SolValue::str(e.payload_hash.clone())),
        ("prev", SolValue::str(e.prev.clone())),
    ]);
    value_hash(&env)
}
