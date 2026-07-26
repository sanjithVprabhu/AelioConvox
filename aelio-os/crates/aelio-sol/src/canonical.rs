//! Canonical serialization (§4.3) — the exact bytes that get BLAKE3-hashed for equality, ledger
//! `payload_hash`, and replay bit-identity (G2).
//!
//! Rules (all normative, §4.3):
//! - UTF-8; map keys sorted bytewise by code point (guaranteed by `BTreeMap`); zero insignificant
//!   whitespace.
//! - Integers: i64, plain decimal, no leading zeros (Rust's `{}` already does this).
//! - Floats: IEEE-754 f64, **shortest round-trip form, always carrying a decimal point** (`2` int ≢
//!   `2.0` float); `-0.0 → 0.0` (enforced at construction, [`crate::SolValue::float`]).
//! - Strings: raw code points, **minimal escape set, no Unicode normalization** (hash exact bytes).
//! - `NaN`/`±∞` cannot occur (rejected at construction).
//!
//! The grammar here is a deterministic JSON-shaped encoding — it is a *hashing* form, not a
//! user-facing serializer; there is exactly one byte string per value.

use crate::value::SolValue;

/// Serialize a value to its canonical byte form (§4.3). Total: every constructible `SolValue` has
/// exactly one canonical encoding (floats are pre-normalized finite, so no error case remains here).
pub fn to_bytes(value: &SolValue) -> Vec<u8> {
    let mut out = Vec::new();
    write_value(value, &mut out);
    out
}

/// Convenience: canonical form as a `String` (the bytes are always valid UTF-8).
pub fn to_string(value: &SolValue) -> String {
    // Safe: `write_value` only emits UTF-8.
    String::from_utf8(to_bytes(value)).expect("canonical form is UTF-8")
}

fn write_value(value: &SolValue, out: &mut Vec<u8>) {
    match value {
        SolValue::Null => out.extend_from_slice(b"null"),
        SolValue::Bool(true) => out.extend_from_slice(b"true"),
        SolValue::Bool(false) => out.extend_from_slice(b"false"),
        SolValue::Int(i) => {
            // Plain decimal, no leading zeros, no `+`. Rust's Display for i64 is exactly this.
            out.extend_from_slice(itoa_i64(*i).as_bytes());
        }
        SolValue::Float(f) => write_float(*f, out),
        SolValue::Str(s) => write_string(s, out),
        SolValue::List(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_value(item, out);
            }
            out.push(b']');
        }
        SolValue::Map(map) => {
            // BTreeMap iterates in sorted key order — the §4.3 "sorted bytewise" rule, structurally.
            out.push(b'{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_string(k, out);
                out.push(b':');
                write_value(v, out);
            }
            out.push(b'}');
        }
    }
}

fn itoa_i64(i: i64) -> String {
    // i64 Display: plain decimal, no leading zeros, `-` only for negatives. Matches §4.3.
    i.to_string()
}

/// Shortest round-trip float with a guaranteed decimal point (or exponent), so a float is never
/// byte-identical to an integer (§4.3 int/float distinction). `-0.0` is already `0.0` at
/// construction.
fn write_float(f: f64, out: &mut Vec<u8>) {
    debug_assert!(f.is_finite(), "non-finite float reached canonicalization (§4.3 invariant)");
    let mut buf = ryu::Buffer::new();
    let s = buf.format_finite(f); // shortest round-trip; ryu emits "2.0", "0.0", "1e20", etc.
    // ryu always emits a `.` or an exponent for floats (never a bare integer like "2"), so the
    // int/float distinction holds. Guard defensively anyway: if neither a '.' nor 'e' is present,
    // append ".0" to keep the "always a decimal point" invariant absolute.
    if s.bytes().any(|b| b == b'.' || b == b'e' || b == b'E') {
        out.extend_from_slice(s.as_bytes());
    } else {
        out.extend_from_slice(s.as_bytes());
        out.extend_from_slice(b".0");
    }
}

/// Minimal-escape string encoding (§4.3): quote; escape only the JSON-mandatory set
/// (`"`, `\`, and control chars U+0000..U+001F); everything else is raw UTF-8 code points with
/// **no Unicode normalization**.
fn write_string(s: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for ch in s.chars() {
        match ch {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\u{08}' => out.extend_from_slice(b"\\b"),
            '\u{0C}' => out.extend_from_slice(b"\\f"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            c if (c as u32) < 0x20 => {
                // Other control chars → \u00XX (lowercase hex, the minimal canonical choice).
                let code = c as u32;
                out.extend_from_slice(b"\\u00");
                out.push(hex_nibble((code >> 4) as u8 & 0xF));
                out.push(hex_nibble(code as u8 & 0xF));
            }
            c => {
                // Raw code point, no normalization.
                let mut b = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
            }
        }
    }
    out.push(b'"');
}

fn hex_nibble(n: u8) -> u8 {
    match n {
        0..=9 => b'0' + n,
        _ => b'a' + (n - 10),
    }
}
