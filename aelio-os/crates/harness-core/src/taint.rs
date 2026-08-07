//! HKv4 taint tracking (§5.9): value taint, map-key absorption, prohibited positions.

use std::collections::BTreeMap;

use aelio_sol::SolValue;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaintError {
    Prohibited { position: ProhibitedPosition },
}

/// Every runtime value carries a taint bit (§5.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaintedValue<T> {
    pub value: T,
    pub tainted: bool,
}

/// Reports taint carried by a value embedded in a container.
///
/// Container implementations are deliberately conservative: an inner tainted
/// container taints the outer container even when its immediate wrapper is clean.
pub trait TaintState {
    fn is_tainted(&self) -> bool;
}

impl TaintState for SolValue {
    fn is_tainted(&self) -> bool {
        false
    }
}

impl<T> TaintedValue<T> {
    pub fn clean(value: T) -> Self {
        Self {
            value,
            tainted: false,
        }
    }

    pub fn tainted(value: T) -> Self {
        Self {
            value,
            tainted: true,
        }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> TaintedValue<U> {
        TaintedValue {
            value: f(self.value),
            tainted: self.tainted,
        }
    }
}

impl<T: TaintState> TaintState for TaintedValue<T> {
    fn is_tainted(&self) -> bool {
        self.tainted || self.value.is_tainted()
    }
}

/// Any tainted input taints the output (§5.9).
pub fn join_taint(inputs: impl IntoIterator<Item = bool>) -> bool {
    inputs.into_iter().any(|t| t)
}

/// Positions where tainted values are forbidden (§5.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProhibitedPosition {
    ToolName,
    HarnessName,
    ModelPrompt,
    AuthoringPrompt,
    PermissionDecision,
    IdempotencyKey,
    JoinPath,
    EmitPayload,
    FailReason,
}

impl ProhibitedPosition {
    pub const ALL: [Self; 9] = [
        Self::ToolName,
        Self::HarnessName,
        Self::ModelPrompt,
        Self::AuthoringPrompt,
        Self::PermissionDecision,
        Self::IdempotencyKey,
        Self::JoinPath,
        Self::EmitPayload,
        Self::FailReason,
    ];
}

/// Reject tainted values in security-sensitive positions (§5.9).
pub fn check_prohibited(
    position: ProhibitedPosition,
    value: &TaintedValue<impl Clone>,
) -> Result<(), TaintError> {
    if value.tainted {
        return Err(TaintError::Prohibited { position });
    }
    Ok(())
}

/// Program-counter taint for implicit flows through control flow (N12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PcTaint {
    pub tainted: bool,
}

impl PcTaint {
    pub fn inherit_from_predicate(predicate_tainted: bool) -> Self {
        Self {
            tainted: predicate_tainted,
        }
    }

    pub fn join(self, other: Self) -> Self {
        Self {
            tainted: self.tainted || other.tainted,
        }
    }

    /// Nested clean predicates do not clear inherited taint (E4).
    pub fn enter_branch(self, predicate_tainted: bool) -> Self {
        self.join(Self::inherit_from_predicate(predicate_tainted))
    }

    /// An assignment made under a tainted program counter is tainted even if
    /// the selected value itself is clean.
    pub fn taint_assignment<T>(self, value: TaintedValue<T>) -> TaintedValue<T> {
        TaintedValue {
            value: value.value,
            tainted: self.tainted || value.tainted,
        }
    }
}

/// List container with conservative taint propagation (§5.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaintedList<T> {
    pub container_tainted: bool,
    pub items: Vec<TaintedValue<T>>,
}

impl<T: TaintState> TaintedList<T> {
    pub fn from_items(items: Vec<TaintedValue<T>>) -> Self {
        let container_tainted = items.iter().any(TaintState::is_tainted);
        Self {
            container_tainted,
            items,
        }
    }

    pub fn push(&mut self, item: TaintedValue<T>) {
        self.container_tainted |= item.is_tainted();
        self.items.push(item);
    }

    /// Indexing cannot launder either element or container taint.
    pub fn get(&self, index: usize) -> Option<TaintedValue<T>>
    where
        T: Clone,
    {
        self.items.get(index).cloned().map(|item| TaintedValue {
            tainted: self.container_tainted || item.is_tainted(),
            value: item.value,
        })
    }

    /// Container shape can reveal tainted data; only `declassify_count` may
    /// intentionally release cardinality.
    pub fn len(&self) -> TaintedValue<i64> {
        TaintedValue {
            value: self.items.len() as i64,
            tainted: self.container_tainted,
        }
    }
}

impl<T> TaintState for TaintedList<T> {
    fn is_tainted(&self) -> bool {
        self.container_tainted
    }
}

/// Map container that absorbs taint from keys at construction (N13).
#[derive(Debug, Clone, PartialEq)]
pub struct TaintedMap {
    pub keys_tainted: bool,
    pub entries: BTreeMap<String, TaintedValue<SolValue>>,
}

impl TaintedMap {
    pub fn insert(
        &mut self,
        key: TaintedValue<String>,
        value: TaintedValue<SolValue>,
    ) -> Option<TaintedValue<SolValue>> {
        self.keys_tainted = join_taint([self.keys_tainted, key.tainted, value.is_tainted()]);
        self.entries.insert(key.value, value)
    }

    /// A clean element of a tainted map remains tainted on extraction.
    pub fn get(&self, key: &str) -> Option<TaintedValue<SolValue>> {
        self.entries.get(key).cloned().map(|value| TaintedValue {
            tainted: self.keys_tainted || value.is_tainted(),
            value: value.value,
        })
    }

    /// Extracting a key returns it tainted when the container absorbed key taint.
    pub fn key_as_tainted_string(&self, key: &str) -> Option<TaintedValue<String>> {
        if !self.entries.contains_key(key) {
            return None;
        }
        Some(TaintedValue {
            value: key.to_owned(),
            tainted: self.keys_tainted,
        })
    }

    pub fn len(&self) -> TaintedValue<i64> {
        TaintedValue {
            value: self.entries.len() as i64,
            tainted: self.keys_tainted,
        }
    }
}

impl TaintState for TaintedMap {
    fn is_tainted(&self) -> bool {
        self.keys_tainted
    }
}

/// Apply an op — any tainted input taints the output (§5.9).
pub fn propagate_taint(inputs: &[bool]) -> bool {
    join_taint(inputs.iter().copied())
}

/// Comparisons are ordinary derived values and therefore retain operand taint.
pub fn tainted_eq<T: PartialEq>(
    left: &TaintedValue<T>,
    right: &TaintedValue<T>,
) -> TaintedValue<bool> {
    TaintedValue {
        value: left.value == right.value,
        tainted: join_taint([left.tainted, right.tainted]),
    }
}

/// The only sanctioned declassification op (§5.9).
pub fn declassify_count(items: &[TaintedValue<SolValue>]) -> TaintedValue<i64> {
    TaintedValue::clean(items.len() as i64)
}
