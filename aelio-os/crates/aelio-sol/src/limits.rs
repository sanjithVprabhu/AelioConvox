//! §4.4 limits. v0 defaults; deployer-tunable downward, hard-capped upward. Violations map to the
//! §11 `Budget.Size` ReasonCode (the kernel does that mapping; sol returns [`crate::LimitError`]).
//!
//! "No unbounded iteration anywhere" (feeds G1): `all`/`column`-scope ops carry their own
//! `max_items` — enforced in the kernel, not here; sol enforces the structural size caps.

use crate::error::{LimitError, SolError, SolResult};
use crate::value::SolValue;

/// v0 default limits (§4.4).
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_depth: usize,
    pub max_keys_per_map: usize,
    pub max_canonical_bytes: usize,
    pub max_list_len: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_depth: 32,
            max_keys_per_map: 1_024,
            max_canonical_bytes: 1 << 20, // 1 MiB
            max_list_len: 10_000,
        }
    }
}

impl Limits {
    /// Validate structural size caps over a materialized value. Depth is 1-based (a scalar is
    /// depth 1). Also enforces the canonical-byte cap (§4.4).
    pub fn check(&self, value: &SolValue) -> SolResult<()> {
        self.check_depth(value, 1)?;
        let bytes = crate::canonical::to_bytes(value).len();
        if bytes > self.max_canonical_bytes {
            return Err(SolError::Limit(LimitError::Bytes {
                max: self.max_canonical_bytes,
            }));
        }
        Ok(())
    }

    fn check_depth(&self, value: &SolValue, depth: usize) -> SolResult<()> {
        if depth > self.max_depth {
            return Err(SolError::Limit(LimitError::Depth {
                max: self.max_depth,
            }));
        }
        match value {
            SolValue::List(items) => {
                if items.len() > self.max_list_len {
                    return Err(SolError::Limit(LimitError::ListLen {
                        max: self.max_list_len,
                    }));
                }
                for item in items {
                    self.check_depth(item, depth + 1)?;
                }
            }
            SolValue::Map(map) => {
                if map.len() > self.max_keys_per_map {
                    return Err(SolError::Limit(LimitError::Keys {
                        max: self.max_keys_per_map,
                    }));
                }
                for v in map.values() {
                    self.check_depth(v, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
