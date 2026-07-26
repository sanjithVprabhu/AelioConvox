//! §11 ReasonCode taxonomy (closed, top-level; namespaced free-text detail) and `err.v1`.

use aelio_sol::SolValue;

/// Closed top-level ReasonCodes (§11.1). Sub-codes live in `detail`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReasonCode {
    Shape,
    Type,
    Missing,
    BudgetCalls,
    BudgetTokens,
    BudgetMs,
    BudgetSize,
    BudgetIter,
    Timeout,
    Policy,
    GuardViolation,
    ToolTransient,
    ToolPermanent,
    ToolAuth,
    ToolRateLimit,
    ModelParse,
    ModelRefuse,
    ConvertNoEdge,
    ConvertRuleFail,
    Internal,
}

impl ReasonCode {
    /// Dotted string form used in `err.v1.code` and `Try.catch` prefix matching (§11.2).
    pub fn code(&self) -> &'static str {
        use ReasonCode::*;
        match self {
            Shape => "Shape",
            Type => "Type",
            Missing => "Missing",
            BudgetCalls => "Budget.Calls",
            BudgetTokens => "Budget.Tokens",
            BudgetMs => "Budget.Ms",
            BudgetSize => "Budget.Size",
            BudgetIter => "Budget.Iter",
            Timeout => "Timeout",
            Policy => "Policy",
            GuardViolation => "Guard.Violation",
            ToolTransient => "Tool.Transient",
            ToolPermanent => "Tool.Permanent",
            ToolAuth => "Tool.Auth",
            ToolRateLimit => "Tool.RateLimit",
            ModelParse => "Model.Parse",
            ModelRefuse => "Model.Refuse",
            ConvertNoEdge => "Convert.NoEdge",
            ConvertRuleFail => "Convert.RuleFail",
            Internal => "Internal",
        }
    }

    /// Normative retryability (§11.3). Advisory — the kernel never auto-retries (§11.5).
    pub fn retryable(&self) -> bool {
        use ReasonCode::*;
        matches!(self, ToolTransient | ToolRateLimit | Timeout | Internal | ModelParse)
    }
}

/// `err.v1` (§11.4) — errors are Sols. `detail` is never load-bearing for matching.
#[derive(Debug, Clone, PartialEq)]
pub struct ErrV1 {
    pub code: ReasonCode,
    pub detail: String,
    /// `op_serial` in the spec — the raising node's nid.
    pub op_serial: String,
    pub retryable: bool,
    pub cause: Option<Box<ErrV1>>,
}

impl ErrV1 {
    pub fn new(code: ReasonCode, op_serial: impl Into<String>, detail: impl Into<String>) -> ErrV1 {
        let retryable = code.retryable();
        ErrV1 {
            code,
            detail: detail.into(),
            op_serial: op_serial.into(),
            retryable,
            cause: None,
        }
    }

    pub fn with_cause(mut self, cause: ErrV1) -> ErrV1 {
        self.cause = Some(Box::new(cause));
        self
    }

    /// Materialize as a Sol value at `err_into` (§8.3 Try) — handlers inspect via pointers.
    pub fn to_sol(&self) -> SolValue {
        let mut pairs = vec![
            ("code", SolValue::str(self.code.code())),
            ("detail", SolValue::str(self.detail.clone())),
            ("op_serial", SolValue::str(self.op_serial.clone())),
            ("retryable", SolValue::Bool(self.retryable)),
        ];
        if let Some(cause) = &self.cause {
            pairs.push(("cause", cause.to_sol()));
        }
        SolValue::map(pairs)
    }
}

pub type ExecResult<T> = Result<T, ErrV1>;
