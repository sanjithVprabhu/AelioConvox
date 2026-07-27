//! Sol-layer errors. These are the `aelio-sol` internal failures; the kernel maps the
//! boundedness ones to the §11 `Budget.Size` ReasonCode. Kept total: nothing panics on
//! untrusted input.

use core::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SolError {
    /// `NaN`/`±∞` are forbidden in Sol data (§4.3) — rejected at construction/ingress.
    NonFinite,
    /// A program-bearing bag (contains `fn`/`flow`) has no canonical form / structural imprint
    /// and may not cross a boundary (§4.1.4).
    ProgramBearing,
    /// A `var` reference remained unresolved when a materialized value was required (§4.1.3).
    /// `aelio-sol` never resolves `var`; the kernel resolves before asking sol to hash (FLAGS F-005).
    UnresolvedVar { path: String },
    /// Path text violated the §6.1 grammar.
    PathSyntax { at: usize, why: &'static str },
    /// §4.4 limit exceeded → kernel maps to `Budget.Size`.
    Limit(LimitError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitError {
    /// Nesting deeper than the §4.4 cap (default 32).
    Depth { max: usize },
    /// Map with more than the §4.4 key cap (default 1024).
    Keys { max: usize },
    /// Canonical body larger than the §4.4 byte cap (default 1 MiB).
    Bytes { max: usize },
    /// List longer than the §4.4 length cap (default 10_000).
    ListLen { max: usize },
}

impl fmt::Display for SolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SolError::NonFinite => write!(f, "NaN/±∞ forbidden in Sol data (§4.3)"),
            SolError::ProgramBearing => {
                write!(f, "program-bearing bag has no canonical form (§4.1.4)")
            }
            SolError::UnresolvedVar { path } => write!(f, "unresolved var `{path}` (§4.1.3)"),
            SolError::PathSyntax { at, why } => {
                write!(f, "path syntax error at {at}: {why} (§6.1)")
            }
            SolError::Limit(e) => write!(f, "limit exceeded: {e:?} (§4.4 → Budget.Size)"),
        }
    }
}

impl std::error::Error for SolError {}

pub type SolResult<T> = Result<T, SolError>;
