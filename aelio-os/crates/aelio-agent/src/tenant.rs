//! Tenant declaration model — the entire product surface from the tenant side.
//! Tools, personalities, states, policies, flows.

use crate::contract::Predicate;
use crate::types::{Sensitivity, Value};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

// ── Tools ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub id: String,
    pub name: String,
    pub version: String,
    /// How the system FINDS this tool — not judgment, lookup.
    pub capability_tags: Vec<String>,
    /// Exact kernel effect class. `None` is accepted only for legacy/local declarations and
    /// conservatively derives External when `effectful` is true, Read otherwise.
    #[serde(default)]
    pub effect: Option<ToolEffect>,
    pub effectful: bool,
    pub idempotent: bool,
    pub dry_run_available: bool,
    pub params: Vec<ParamSpec>,
    pub output_semantics: OutputSpec,
    /// Declared expected next capabilities (e.g. auth.otp.verify after send).
    pub continuations: Vec<String>,
    pub errors: Vec<ErrorSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEffect {
    Pure,
    Read,
    Write,
    External,
}

impl ToolSpec {
    pub fn effect_class(&self) -> ToolEffect {
        self.effect.unwrap_or(if self.effectful {
            ToolEffect::External
        } else {
            ToolEffect::Read
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamSpec {
    pub name: String,
    pub type_name: String,
    pub required: bool,
    pub constraint: Option<ParamConstraint>,
    pub source: ParamSource,
    pub repair: Option<RepairFn>,
    pub prompt_hint: Option<String>,
    pub sensitivity: Sensitivity,
    pub default: Option<Value>,
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamConstraint {
    Range { lo: f64, hi: f64 },
    Pattern { regex: String },
    Enum { allowed: Vec<String> },
    Length { min: usize, max: usize },
    Format { kind: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamSource {
    User,
    Slot { name: String },
    State { path: String },
    Env { key: String },
    Derived { expr: String },
    ToolOutput { ref_path: String },
    Const { value: Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairFn {
    NormalizePhone { region: String },
    NormalizeEmail,
    NormalizeWhitespace,
    CoerceNumber,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputSpec {
    /// Declared meaning of fields — never learned.
    pub fields: IndexMap<String, OutputField>,
    pub role_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputField {
    pub path: String,
    pub type_name: String,
    pub sensitivity: Sensitivity,
    pub meaning: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorSpec {
    pub match_code: String,
    pub reason: String,
    pub recovery: String,
}

// ── Personality ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalitySpec {
    pub id: String,
    pub voice: VoiceSpec,
    pub lexicon: LexiconSpec,
    pub constraints: Vec<String>,
    pub templates: IndexMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceSpec {
    pub register: String,
    pub verbosity: String,
    pub formality: String,
    pub emoji_policy: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LexiconSpec {
    pub preferred: Vec<String>,
    pub forbidden: Vec<String>,
}

// ── States ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSpec {
    pub id: String,
    pub name: String,
    pub permission_envelope: Vec<String>,
    pub direction: Option<StateDirection>,
    pub entry_conditions: Vec<Predicate>,
    pub exit_edges: Vec<ExitEdge>,
    pub timeout: Option<StateTimeout>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDirection {
    pub target: String,
    pub nudge_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitEdge {
    pub to: String,
    pub guard: Predicate,
    pub evidence_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateTimeout {
    pub after_secs: u64,
    pub to: String,
}

// ── Policies ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicySpec {
    pub id: String,
    pub effect: PolicyEffect,
    pub subject: PolicySubject,
    pub action: PolicyAction,
    pub condition: Predicate,
    pub reason_code: String,
    pub priority: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffect {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolicySubject {
    pub role: Option<String>,
    pub state: Option<String>,
    pub tenant: Option<String>,
    pub segment: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolicyAction {
    pub capability: Option<String>,
    pub tool_id: Option<String>,
    pub transition: Option<String>,
    pub flow_id: Option<String>,
}

// ── Flows ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowSpec {
    pub id: String,
    pub version: String,
    pub name: String,
    pub activation: FlowActivation,
    /// Auth / payment / destructive → structurally barred from reordering.
    pub learnable: bool,
    pub preemption: Preemption,
    pub steps: Vec<FlowStep>,
    pub escape: FlowEscape,
    pub terminal_states: Vec<String>,
    pub ttl_secs: Option<u64>,
    pub max_attempts: u32,
    /// Optional closed HOW-layer compiled at catalog admission. Semantic flow declarations remain
    /// useful without it, but are deliberately non-executable and create materialization demand.
    /// Calls inside `program` must use `$cap:<binding>` symbols; raw target ids are rejected.
    #[serde(default)]
    pub lowering: Option<FlowLoweringV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowLoweringV1 {
    pub format: u32,
    pub program: serde_json::Value,
    pub bindings: Vec<FlowCapabilityBindingV1>,
    pub cases: Vec<FlowGateCaseV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowCapabilityBindingV1 {
    /// Local symbol referenced as `$cap:<name>` by Call nodes and fixtures.
    pub name: String,
    /// Semantic step whose admissible set authorizes this capability.
    pub step_id: String,
    pub capability: String,
    pub deadline_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowGateCaseV1 {
    pub input: serde_json::Value,
    #[serde(default)]
    pub wakes: Vec<serde_json::Value>,
    #[serde(default)]
    pub expect_park: bool,
    pub expected: serde_json::Value,
    #[serde(default)]
    pub fixtures: Vec<FlowFixtureV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowFixtureV1 {
    /// A binding name, not a raw target id.
    pub binding: String,
    pub output: serde_json::Value,
    #[serde(default)]
    pub usage_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowActivation {
    pub hard_preconditions: Vec<Predicate>,
    pub trigger_surface: Vec<String>,
    pub margin_threshold: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preemption {
    Hold,
    SuspendYield,
    Yield,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowStep {
    pub id: String,
    pub intent: String,
    /// WHAT must be true — never HOW.
    pub postcondition: Predicate,
    pub admissible: Vec<String>,
    pub on_violation: ViolationAction,
    pub suspendable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViolationAction {
    Repair,
    Escape,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FlowEscape {
    Fallback { flow_id: String },
    FreeRange,
    Escalate,
}

// ── Attributes (term resolution with polarity) ───────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttributeSpec {
    pub name: String,
    pub anchors: Vec<String>,
    pub learned: Vec<String>,
    /// high = more pressure → "most X" sorts desc when polarity is HighMeansMore
    pub polarity: Polarity,
    pub sortable: bool,
    /// Tenant-declared capability which executes this attribute's query-plan fragment.
    #[serde(default)]
    pub query_capability: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Polarity {
    HighMeansMore,
    HighMeansLess,
}

// ── Tenant bundle ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantDecl {
    pub tenant_id: String,
    pub mode: crate::types::TenantMode,
    pub tools: Vec<ToolSpec>,
    pub personalities: Vec<PersonalitySpec>,
    pub states: Vec<StateSpec>,
    pub policies: Vec<PolicySpec>,
    pub flows: Vec<FlowSpec>,
    /// Optional exact runtime artifact pins for authored flows. A declared binding makes the
    /// runtime continuation authoritative; absence retains the migration shadow rail.
    #[serde(default)]
    pub flow_artifacts: IndexMap<String, crate::adaptive::ArtifactPinV1>,
    pub attributes: Vec<AttributeSpec>,
}

impl TenantDecl {
    pub fn empty(tenant_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            mode: crate::types::TenantMode::Bake,
            tools: vec![],
            personalities: vec![],
            states: vec![],
            policies: vec![],
            flows: vec![],
            flow_artifacts: IndexMap::new(),
            attributes: vec![],
        }
    }
}
