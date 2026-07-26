//! serde_json ↔ SolValue bridge. `serde_json::Number` preserves the int/float distinction
//! (`is_i64` vs `is_f64`), so `2` → `Int` and `2.0` → `Float` survive parsing (§4.3). NaN/∞ can't
//! appear in JSON text.

use aelio_sol::{SolError, SolValue};
use serde_json::Value as J;

/// Convert a parsed JSON value into a program-free `SolValue`. Errors only on non-finite floats
/// (which JSON text cannot actually produce, but we stay total).
pub fn from_json(j: &J) -> Result<SolValue, SolError> {
    Ok(match j {
        J::Null => SolValue::Null,
        J::Bool(b) => SolValue::Bool(*b),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                SolValue::Int(i)
            } else if let Some(f) = n.as_f64() {
                SolValue::float(f)?
            } else {
                // u64 > i64::MAX — out of the i64 fundamental type (§5).
                return Err(SolError::NonFinite);
            }
        }
        J::String(s) => SolValue::Str(s.clone()),
        J::Array(a) => {
            let mut items = Vec::with_capacity(a.len());
            for v in a {
                items.push(from_json(v)?);
            }
            SolValue::List(items)
        }
        J::Object(o) => {
            let mut map = std::collections::BTreeMap::new();
            for (k, v) in o {
                map.insert(k.clone(), from_json(v)?);
            }
            SolValue::Map(map)
        }
    })
}
