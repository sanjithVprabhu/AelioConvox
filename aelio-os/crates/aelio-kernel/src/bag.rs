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
        Bag {
            root: SolValue::Map(BTreeMap::new()),
        }
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

    /// Remove the value at `path`. This is used to unwind a `Let` binding that shadowed an absent
    /// key. Unlike `set`, removal never creates structure and rejects root deletion.
    pub fn remove(&mut self, path: &Path) -> Result<(), &'static str> {
        remove_rec(&mut self.root, path.segments())
    }

    /// `blake3(canonical(bag))` — the App G `turn_end.bag_hash`, the bit-identity check.
    pub fn hash(&self) -> String {
        aelio_sol::value_hash(&self.root)
    }
}

fn remove_rec(node: &mut SolValue, segs: &[Segment]) -> Result<(), &'static str> {
    match segs {
        [] => Err("cannot remove the bag root"),
        [Segment::Key(key)] => {
            let SolValue::Map(map) = node else {
                return Err("key removal requires an existing map");
            };
            map.remove(key).ok_or("path not present")?;
            Ok(())
        }
        [Segment::Index(index)] => {
            let SolValue::List(list) = node else {
                return Err("index removal requires an existing list");
            };
            if *index >= list.len() {
                return Err("index out of range");
            }
            list.remove(*index);
            Ok(())
        }
        [Segment::Key(key), rest @ ..] => {
            let SolValue::Map(map) = node else {
                return Err("key removal requires an existing map");
            };
            let child = map.get_mut(key).ok_or("path not present")?;
            remove_rec(child, rest)
        }
        [Segment::Index(index), rest @ ..] => {
            let SolValue::List(list) = node else {
                return Err("index removal requires an existing list");
            };
            let child = list.get_mut(*index).ok_or("index out of range")?;
            remove_rec(child, rest)
        }
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
                let SolValue::Map(map) = node else {
                    unreachable!()
                };
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
