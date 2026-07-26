//! The flowing bag (§4.2): one `SolValue::Map` root; ops edit exactly their static write set, all
//! other keys pass untouched. `set` replaces the value at a literal path (creating intermediate
//! maps for `Key` segments). Writes are atomic at the executor layer (§4.2.6).

use aelio_sol::{Path, Segment, SolValue};
use std::collections::BTreeMap;

/// A working-memory bag.
#[derive(Debug, Clone)]
pub struct Bag {
    root: SolValue,
}

impl Default for Bag {
    fn default() -> Self {
        Bag { root: SolValue::Map(BTreeMap::new()) }
    }
}

impl Bag {
    pub fn from_value(root: SolValue) -> Bag {
        Bag { root }
    }

    pub fn value(&self) -> &SolValue {
        &self.root
    }

    pub fn get(&self, path: &Path) -> Option<&SolValue> {
        path.get(&self.root)
    }

    /// Whole-bag replace — only `Const` may do this (§4.2.3); the executor enforces that.
    pub fn replace_root(&mut self, value: SolValue) {
        self.root = value;
    }

    /// Edit: write `value` at `path`, creating intermediate maps for `Key` segments. `Index`
    /// segments require an existing list of sufficient length (writes never grow lists implicitly).
    pub fn set(&mut self, path: &Path, value: SolValue) -> Result<(), &'static str> {
        set_rec(&mut self.root, path.segments(), value)
    }

    /// `blake3(canonical(bag))` — the App G `turn_end.bag_hash`, the bit-identity check.
    pub fn hash(&self) -> String {
        aelio_sol::value_hash(&self.root)
    }
}

fn set_rec(node: &mut SolValue, segs: &[Segment], value: SolValue) -> Result<(), &'static str> {
    match segs {
        [] => {
            *node = value;
            Ok(())
        }
        [seg, rest @ ..] => match seg {
            Segment::Key(k) => {
                if !matches!(node, SolValue::Map(_)) {
                    *node = SolValue::Map(BTreeMap::new());
                }
                let SolValue::Map(map) = node else { unreachable!() };
                let child = map.entry(k.clone()).or_insert(SolValue::Null);
                set_rec(child, rest, value)
            }
            Segment::Index(i) => {
                let SolValue::List(list) = node else {
                    return Err("index write requires an existing list");
                };
                let child = list.get_mut(*i).ok_or("index out of range")?;
                set_rec(child, rest, value)
            }
        },
    }
}
