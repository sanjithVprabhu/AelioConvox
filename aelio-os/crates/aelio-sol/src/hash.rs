//! BLAKE3 over canonical bytes (§4.3). This one hash function underlies `payload_hash`,
//! `continuation_hash`, structural imprints, and the per-instance ledger chain (App G).

use crate::canonical;
use crate::value::SolValue;

/// BLAKE3 of arbitrary bytes, lowercase hex.
pub fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// `blake3(canonical(value))` — the content hash used for equality and ledger `payload_hash`.
pub fn value_hash(value: &SolValue) -> String {
    blake3_hex(&canonical::to_bytes(value))
}
