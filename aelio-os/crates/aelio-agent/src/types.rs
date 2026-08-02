//! Core type universe and closed reason-code taxonomy.
//!
//! Every op returns `AelioResult<T>` — totality: nothing throws, no ambiguous nulls.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Dynamic JSON-shaped value tree used throughout the runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Value>),
    Map(IndexMap<String, Value>),
}

impl Value {
    pub fn str(s: impl Into<String>) -> Self {
        Value::Str(s.into())
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f)
                if f.fract() == 0.0 && *f >= i64::MIN as f64 && *f <= i64::MAX as f64 =>
            {
                Some(*f as i64)
            }
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&IndexMap<String, Value>> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    pub fn type_tag(&self) -> TypeTag {
        match self {
            Value::Null => TypeTag::Null,
            Value::Bool(_) => TypeTag::Bool,
            Value::Int(_) => TypeTag::Int,
            Value::Float(_) => TypeTag::Float,
            Value::Str(_) => TypeTag::Str,
            Value::List(_) => TypeTag::List,
            Value::Map(_) => TypeTag::Map,
        }
    }

    /// Structural fingerprint used by response signatures (before cleaning).
    pub fn shape_sig(&self) -> ShapeSig {
        ShapeSig::from_value(self)
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Str(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Str(v.to_string())
    }
}
impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::List(v)
    }
}
impl From<IndexMap<String, Value>> for Value {
    fn from(v: IndexMap<String, Value>) -> Self {
        Value::Map(v)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeTag {
    Null,
    Bool,
    Int,
    Float,
    Str,
    List,
    Map,
    Bytes,
    Instant,
    Duration,
    Vector,
}

impl fmt::Display for TypeTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Closed top-level reason codes (dictionary §9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    Validation,
    Missing,
    Ambiguous,
    Denied,
    NotFound,
    Conflict,
    RateLimited,
    Timeout,
    Unavailable,
    ToolError,
    SigMismatch,
    BudgetExceeded,
    LoopBudgetExceeded,
    Unsatisfiable,
    Suspended,
    Cancelled,
    Internal,
    ParseError,
    DivByZero,
    Overflow,
    DomainError,
    EmptyInput,
    CastError,
    TypeViolation,
    RangeViolation,
    LengthViolation,
    EnumViolation,
    PatternViolation,
    FormatViolation,
    NoMatch,
    NeedsRepair,
    NeedsEscalation,
    Terminal,
    Retryable,
    GateNotMet,
    PolicyDenied,
    Unresolved,
}

impl ReasonCode {
    pub fn recovery(&self) -> Recovery {
        match self {
            ReasonCode::RateLimited
            | ReasonCode::Timeout
            | ReasonCode::Unavailable
            | ReasonCode::Retryable => Recovery::Retryable,
            ReasonCode::Validation
            | ReasonCode::Missing
            | ReasonCode::Ambiguous
            | ReasonCode::NeedsRepair
            | ReasonCode::FormatViolation
            | ReasonCode::PatternViolation
            | ReasonCode::RangeViolation
            | ReasonCode::LengthViolation
            | ReasonCode::EnumViolation
            | ReasonCode::TypeViolation => Recovery::NeedsRepair,
            ReasonCode::NeedsEscalation | ReasonCode::Denied | ReasonCode::PolicyDenied => {
                Recovery::NeedsEscalation
            }
            ReasonCode::Terminal | ReasonCode::Cancelled => Recovery::Terminal,
            _ => Recovery::Terminal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Recovery {
    Retryable,
    NeedsRepair,
    Terminal,
    NeedsEscalation,
}

/// Typed failure with optional detail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AelioError {
    pub code: ReasonCode,
    pub message: String,
    pub detail: Option<Value>,
    /// Metadata-only authoritative artifact trace. This is never populated with arguments or
    /// outputs and lets failed effects remain auditable in the same per-turn decision narrative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_trace: Option<Box<str>>,
}

impl AelioError {
    pub fn new(code: ReasonCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
            decision_trace: None,
        }
    }

    pub fn with_detail(mut self, detail: Value) -> Self {
        self.detail = Some(detail);
        self
    }

    pub fn with_decision_trace(mut self, trace: Option<String>) -> Self {
        self.decision_trace = trace.map(String::into_boxed_str);
        self
    }
}

impl fmt::Display for AelioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for AelioError {}

pub type AelioResult<T> = Result<T, AelioError>;

/// Structural fingerprint of a tool response (computed on raw, before cleaning).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapeSig {
    pub key_set: Vec<String>,
    pub depth: u32,
    pub type_per_path: IndexMap<String, String>,
    pub cardinality: IndexMap<String, usize>,
    pub value_features: IndexMap<String, ValueFeatures>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueFeatures {
    pub length: Option<usize>,
    pub char_class: Option<String>,
    pub pattern_class: Option<String>,
}

impl ShapeSig {
    pub fn from_value(v: &Value) -> Self {
        let mut key_set = Vec::new();
        let mut type_per_path = IndexMap::new();
        let mut cardinality = IndexMap::new();
        let mut value_features = IndexMap::new();
        let depth = walk(
            v,
            "",
            0,
            &mut key_set,
            &mut type_per_path,
            &mut cardinality,
            &mut value_features,
        );
        key_set.sort();
        Self {
            key_set,
            depth,
            type_per_path,
            cardinality,
            value_features,
        }
    }

    pub fn hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        let digest = Sha256::digest(&bytes);
        hex::encode(&digest[..16])
    }
}

fn walk(
    v: &Value,
    path: &str,
    depth: u32,
    key_set: &mut Vec<String>,
    type_per_path: &mut IndexMap<String, String>,
    cardinality: &mut IndexMap<String, usize>,
    value_features: &mut IndexMap<String, ValueFeatures>,
) -> u32 {
    let p = if path.is_empty() {
        "$".to_string()
    } else {
        path.to_string()
    };
    type_per_path.insert(p.clone(), format!("{}", v.type_tag()));
    match v {
        Value::Map(m) => {
            let mut max_d = depth;
            let mut entries: Vec<_> = m.iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            for (k, child) in entries {
                if !key_set.contains(k) {
                    key_set.push(k.clone());
                }
                let child_path = if path.is_empty() {
                    format!("$.{k}")
                } else {
                    format!("{path}.{k}")
                };
                max_d = max_d.max(walk(
                    child,
                    &child_path,
                    depth + 1,
                    key_set,
                    type_per_path,
                    cardinality,
                    value_features,
                ));
            }
            max_d
        }
        Value::List(items) => {
            cardinality.insert(p.clone(), items.len());
            let mut max_d = depth;
            for (i, child) in items.iter().enumerate() {
                let child_path = format!("{p}[{i}]");
                max_d = max_d.max(walk(
                    child,
                    &child_path,
                    depth + 1,
                    key_set,
                    type_per_path,
                    cardinality,
                    value_features,
                ));
            }
            max_d
        }
        Value::Str(s) => {
            value_features.insert(
                p,
                ValueFeatures {
                    length: Some(s.chars().count()),
                    char_class: Some(char_class(s)),
                    pattern_class: Some(pattern_class(s)),
                },
            );
            depth
        }
        _ => depth,
    }
}

fn char_class(s: &str) -> String {
    let digits = s.chars().all(|c| c.is_ascii_digit());
    let alpha = s.chars().all(|c| c.is_ascii_alphabetic());
    let alnum = s.chars().all(|c| c.is_ascii_alphanumeric());
    if digits {
        "digits".into()
    } else if alpha {
        "alpha".into()
    } else if alnum {
        "alnum".into()
    } else {
        "mixed".into()
    }
}

fn pattern_class(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_digit()) && s.len() == 6 {
        "otp6".into()
    } else if s.contains('@') {
        "emailish".into()
    } else if s.starts_with('+') && s.chars().skip(1).all(|c| c.is_ascii_digit()) {
        "e164ish".into()
    } else {
        "other".into()
    }
}

/// Cost substrate for an ability implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Substrate {
    Pure,
    Semantic,
    Llm,
    Effect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostClass {
    Free,
    Cheap,
    Moderate,
    Expensive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    None,
    Pii,
    Secret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Depth {
    Shallow,
    Boundary,
    Deep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyType {
    Generic,
    InputOriented,
    OutputOriented,
    Banter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LookupTier {
    Tier0,
    Tier1,
    Tier2,
    Tier3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantMode {
    Bake,
    Live,
    Rebake,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseRole {
    Data,
    EffectConfirmation,
    Continuation,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_sig_is_stable_for_same_structure() {
        let a = Value::Map(indexmap::indexmap! {
            "otp".into() => Value::str("434543"),
            "ok".into() => Value::Bool(true),
        });
        let b = Value::Map(indexmap::indexmap! {
            "ok".into() => Value::Bool(true),
            "otp".into() => Value::str("999999"),
        });
        // key_set + types match; value features differ on length/pattern
        assert_eq!(a.shape_sig().key_set, b.shape_sig().key_set);
        assert_eq!(
            a.shape_sig().type_per_path.get("$.otp"),
            b.shape_sig().type_per_path.get("$.otp")
        );
        assert_eq!(a.shape_sig().hash(), b.shape_sig().hash());
    }

    #[test]
    fn shape_hash_changes_when_structure_changes() {
        let a = Value::Map(indexmap::indexmap! { "otp".into() => Value::str("123456") });
        let b = Value::Map(indexmap::indexmap! { "code".into() => Value::str("123456") });
        assert_ne!(a.shape_sig().hash(), b.shape_sig().hash());
    }
}
