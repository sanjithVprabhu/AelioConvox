//! `SolValue` — the materialized, program-free data model (§5 fundamental types).
//!
//! Kinds `fn`/`flow` (program-bearing, §4.1.4) are intentionally NOT representable here: this type
//! is exactly the set of values that may be hashed, persisted, and cross `Call` boundaries. `var`
//! (a path ref, §4) is also absent — the kernel resolves `var` before handing a value to sol.
//!
//! Invariant enforced at construction: **no `NaN`/`±∞`** (§4.3). Maps keep keys sorted (BTreeMap) so
//! canonical serialization's "sorted keys" rule is structural, not a serialize-time step.

use crate::error::{SolError, SolResult};
use std::collections::BTreeMap;

/// A materialized Sol data value. Fundamental types per §5: `null | bool | int(i64) | float(f64) |
/// str | list | map`. Nested `sol` is modeled as `Map` here (a nested contract body is a map of
/// keys); the kernel tags declared-imprint nesting separately.
#[derive(Debug, Clone, PartialEq)]
pub enum SolValue {
    Null,
    Bool(bool),
    Int(i64),
    /// Always finite — construct via [`SolValue::float`], which rejects `NaN`/`±∞`.
    Float(f64),
    Str(String),
    List(Vec<SolValue>),
    /// Keys sorted bytewise by code point via `BTreeMap` (§4.3).
    Map(BTreeMap<String, SolValue>),
}

impl SolValue {
    /// Construct a float, rejecting `NaN`/`±∞` (§4.3) and normalizing `-0.0 → 0.0`.
    pub fn float(f: f64) -> SolResult<SolValue> {
        if !f.is_finite() {
            return Err(SolError::NonFinite);
        }
        // §4.3: `-0.0 → 0.0`. `+ 0.0` collapses the sign of a zero without touching other values.
        let normalized = if f == 0.0 { 0.0 } else { f };
        Ok(SolValue::Float(normalized))
    }

    pub fn str(s: impl Into<String>) -> SolValue {
        SolValue::Str(s.into())
    }

    /// Build a map from pairs; keys land sorted. Later duplicate keys overwrite earlier.
    pub fn map<I, K>(pairs: I) -> SolValue
    where
        I: IntoIterator<Item = (K, SolValue)>,
        K: Into<String>,
    {
        SolValue::Map(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    pub fn list<I: IntoIterator<Item = SolValue>>(items: I) -> SolValue {
        SolValue::List(items.into_iter().collect())
    }

    /// The fundamental type tag, used by the structural imprint (§4.1.3) and type-stable-key checks.
    pub fn type_tag(&self) -> TypeTag {
        match self {
            SolValue::Null => TypeTag::Null,
            SolValue::Bool(_) => TypeTag::Bool,
            SolValue::Int(_) => TypeTag::Int,
            SolValue::Float(_) => TypeTag::Float,
            SolValue::Str(_) => TypeTag::Str,
            SolValue::List(_) => TypeTag::List,
            SolValue::Map(_) => TypeTag::Map,
        }
    }

    pub fn as_map(&self) -> Option<&BTreeMap<String, SolValue>> {
        match self {
            SolValue::Map(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[SolValue]> {
        match self {
            SolValue::List(l) => Some(l),
            _ => None,
        }
    }
}

/// Fundamental type tags (§5). `int` and `float` are distinct — Sol distinguishes `2` from `2.0`
/// (§4.3), so they never share a tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeTag {
    Null,
    Bool,
    Int,
    Float,
    Str,
    List,
    Map,
}

impl TypeTag {
    /// Canonical type-signature token used inside the structural imprint (§4.1.3).
    pub fn signature(self) -> &'static str {
        match self {
            TypeTag::Null => "null",
            TypeTag::Bool => "bool",
            TypeTag::Int => "int",
            TypeTag::Float => "float",
            TypeTag::Str => "str",
            TypeTag::List => "list",
            TypeTag::Map => "map",
        }
    }
}
