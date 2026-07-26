//! Structural imprint (§4.1.3) — the canonical recursive hash over a value's **shape** (keys +
//! fundamental-type signatures), written `~<hash>`.
//!
//! Distinct from [`crate::hash::value_hash`], which hashes *content*. The structural imprint is a
//! lookup/bucket key (§4.1.5) — never application authority. Rules (§4.1.3):
//! - sorted keys (BTreeMap order),
//! - fundamental-type signatures,
//! - nested maps hashed recursively,
//! - homogeneous lists as `list<T>`, else `list<mixed>`,
//! - `var` resolved before hashing (sol never sees an unresolved `var`; the kernel resolves first).
//!
//! Program-bearing bags (`fn`/`flow`) have **no** structural imprint (§4.1.4) — not representable in
//! [`crate::SolValue`], so that invariant holds by construction.

use crate::value::{SolValue, TypeTag};

/// The `~<hash>` structural imprint of a materialized value (§4.1.3).
pub fn structural(value: &SolValue) -> String {
    let sig = shape_signature(value);
    format!("~{}", crate::hash::blake3_hex(sig.as_bytes()))
}

/// The canonical shape signature string that gets hashed. Deterministic and self-describing so two
/// values imprint-match iff their shapes match — regardless of scalar contents.
pub fn shape_signature(value: &SolValue) -> String {
    let mut out = String::new();
    write_shape(value, &mut out);
    out
}

fn write_shape(value: &SolValue, out: &mut String) {
    match value {
        SolValue::Map(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                // Key identity matters to shape; value canonical-encodes the key string.
                out.push_str(&crate::canonical::to_string(&SolValue::Str(k.clone())));
                out.push(':');
                write_shape(v, out);
            }
            out.push('}');
        }
        SolValue::List(items) => {
            out.push_str("list<");
            out.push_str(&list_element_signature(items));
            out.push('>');
        }
        scalar => out.push_str(scalar.type_tag().signature()),
    }
}

/// `list<T>` when every element shares the same shape; `list<mixed>` otherwise; `list<never>` for
/// the empty list (a distinct, stable shape).
fn list_element_signature(items: &[SolValue]) -> String {
    let mut iter = items.iter();
    let Some(first) = iter.next() else {
        return "never".to_string();
    };
    let first_sig = element_shape(first);
    for item in iter {
        if element_shape(item) != first_sig {
            return "mixed".to_string();
        }
    }
    first_sig
}

fn element_shape(value: &SolValue) -> String {
    match value.type_tag() {
        TypeTag::Map | TypeTag::List => shape_signature(value),
        scalar => scalar.signature().to_string(),
    }
}
