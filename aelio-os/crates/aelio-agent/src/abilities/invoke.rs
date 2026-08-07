//! Invoke.* — Call / Signature / Extract / Interpret.
//! Only Call is externally effectful. Ledger brackets Call alone.

use crate::abilities::sig::{self, SignatureRegistry};
use crate::orchestration::wavefront::effect_idempotency_key;
use crate::policy::{require_allow, PolicyActionRisk, PolicyCtx};
use crate::tenant::{ErrorSpec, PolicySpec, ToolSpec};
use crate::types::{AelioError, AelioResult, ReasonCode, Recovery, ResponseRole, Value};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Host-provided pinned capability executor. Production turns may only call through this
/// boundary with a catalog `ToolSpec` (exact id/version/effects). Raw tool-id dispatch belongs
/// on the optional [`ToolHost`] extension used by local/parity doubles — not on the production
/// turn spine (§14.1).
pub trait CapabilityHost: Send {
    /// Context-rich entry point used by the generic executor. Durable hosts override this to
    /// bracket the effect with a lease/result record.
    fn call_with_context(
        &mut self,
        tool: &ToolSpec,
        args: &IndexMap<String, Value>,
        idempotency_key: &str,
        user_id: &str,
        channel: &str,
    ) -> AelioResult<Value>;

    /// Optional test/diagnostic counter. Production hosts may leave this unavailable.
    fn invocation_count(&self) -> Option<usize> {
        None
    }

    fn invocation_count_for(&self, _tool_id: &str) -> Option<usize> {
        None
    }

    /// Metadata-only narrative from an authoritative execution engine. Implementations must not
    /// include raw arguments or results. The value is consumed once by the caller.
    fn take_decision_trace(&mut self) -> Option<String> {
        None
    }
}

/// Local/parity tool executor that also permits raw tool-id dispatch for fixtures.
pub trait ToolHost: CapabilityHost {
    fn call(&mut self, tool_id: &str, args: &IndexMap<String, Value>) -> AelioResult<Value>;
}

/// In-memory mock host for tests.
#[derive(Default)]
pub struct MockToolHost {
    pub handlers: IndexMap<String, ToolHandler>,
    pub calls: Vec<(String, IndexMap<String, Value>)>,
}

pub type ToolHandler = Box<dyn Fn(&IndexMap<String, Value>) -> AelioResult<Value> + Send + Sync>;

impl MockToolHost {
    pub fn on(
        mut self,
        tool_id: impl Into<String>,
        f: impl Fn(&IndexMap<String, Value>) -> AelioResult<Value> + Send + Sync + 'static,
    ) -> Self {
        self.handlers.insert(tool_id.into(), Box::new(f));
        self
    }
}

impl CapabilityHost for MockToolHost {
    fn call_with_context(
        &mut self,
        tool: &ToolSpec,
        args: &IndexMap<String, Value>,
        _idempotency_key: &str,
        _user_id: &str,
        _channel: &str,
    ) -> AelioResult<Value> {
        ToolHost::call(self, &tool.id, args)
    }

    fn invocation_count(&self) -> Option<usize> {
        Some(self.calls.len())
    }

    fn invocation_count_for(&self, tool_id: &str) -> Option<usize> {
        Some(
            self.calls
                .iter()
                .filter(|(called_tool, _)| called_tool == tool_id)
                .count(),
        )
    }
}

impl ToolHost for MockToolHost {
    fn call(&mut self, tool_id: &str, args: &IndexMap<String, Value>) -> AelioResult<Value> {
        self.calls.push((tool_id.to_string(), args.clone()));
        match self.handlers.get(tool_id) {
            Some(h) => h(args),
            None => Err(AelioError::new(
                ReasonCode::NotFound,
                format!("tool {tool_id} not registered in host"),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeReceipt {
    pub tool_id: String,
    pub args_hash: String,
    /// Sanitized response safe for persistence and prompt construction. The raw response exists
    /// only transiently while computing its signature and extraction.
    pub safe_response: Value,
    pub role: ResponseRole,
    pub extracted: IndexMap<String, Value>,
    pub used_llm_interpret: bool,
    pub sig_hash: String,
    pub decision_trace: Option<String>,
}

/// Full tool call lifecycle (minus CollectAllSlots parking).
pub struct InvokeContext<'a> {
    pub policies: &'a [PolicySpec],
    pub policy: &'a PolicyCtx,
    pub host: &'a mut dyn CapabilityHost,
    pub signatures: &'a mut SignatureRegistry,
    pub once_seen: &'a mut std::collections::HashSet<String>,
    pub user_id: &'a str,
    pub channel: &'a str,
    pub turn_key: &'a str,
    pub effect_seq: u64,
    pub element_index: Option<u64>,
}

pub fn tool_call_block(
    tool: &ToolSpec,
    args: &IndexMap<String, Value>,
    context: &mut InvokeContext<'_>,
) -> AelioResult<InvokeReceipt> {
    // Policy invariant — structural, not composed.
    let mut ctx = context.policy.clone();
    ctx.tool = Some(tool.id.clone());
    ctx.capability = tool.capability_tags.first().cloned();
    ctx.action_risk = if tool.effectful {
        PolicyActionRisk::EffectfulTool
    } else {
        PolicyActionRisk::ReadOnly
    };
    require_allow(context.policies, &ctx)?;

    let key = effect_idempotency_key(context.turn_key, context.effect_seq, context.element_index);

    // Invoke.Call — the only effectful line
    let raw_result =
        context
            .host
            .call_with_context(tool, args, &key, context.user_id, context.channel);
    let decision_trace = context.host.take_decision_trace();
    let raw = raw_result.map_err(|error| error.with_decision_trace(decision_trace.clone()))?;
    context.once_seen.insert(key.clone());

    let result = (|| {
        // Signature on RAW before cleaning
        let shape = sig::compute(&raw);
        let sig_hash = sig::hash(&shape);

        // Error envelopes have a deliberately different shape from success responses. Classify them
        // after recording the raw signature but before extracting success fields; otherwise a valid
        // typed tool error is misreported as SigMismatch merely because `ok` is absent.
        let role = sig::classify(&raw, tool.output_semantics.role_hint.as_deref());
        if role == ResponseRole::Error {
            let classified = classify_error(&raw, &tool.errors);
            return Err(AelioError::new(classified.0, classified.1)
                .with_detail(sanitize_response(&raw, tool)?));
        }

        let (mut extracted, used_llm) =
            if let Some(plan) = sig::match_plan(context.signatures, &sig_hash) {
                (sig::extract(&raw, &plan)?, false)
            } else {
                // Cold path: Interpret → validate against declared semantics → propose. Promotion is a
                // separate evidence-gated cold-loop operation.
                let interpreted = structural_interpret(&raw, tool)?;
                let declared_paths = tool
                    .output_semantics
                    .fields
                    .iter()
                    .map(|(name, field)| (name.clone(), field.path.clone()))
                    .collect();
                let plan = sig::propose_with_paths(&raw, &interpreted, &declared_paths);
                context.signatures.plans.insert(plan.sig_hash.clone(), plan);
                (interpreted, true)
            };

        // Declared semantics, not learned structure: any output field the tenant marked `pii` or
        // `secret` must never leave this function in the clear. `extracted` flows straight into the
        // execution frame's evidence, which is bound into `Express.Synthesize`'s prompt (rendered
        // verbatim into the text sent to the model — only the *logged* copy is redacted) and persisted
        // to memory and the ledger. Redact by declared field name here so the OTP never reaches a
        // prompt or a log. (§15.3, decisions T2/C3.)
        redact_sensitive_extracted(tool, &mut extracted);

        Ok(InvokeReceipt {
            tool_id: tool.id.clone(),
            args_hash: key,
            safe_response: sanitize_response(&raw, tool)?,
            role,
            extracted,
            used_llm_interpret: used_llm,
            sig_hash,
            decision_trace: decision_trace.clone(),
        })
    })();
    result.map_err(|error| error.with_decision_trace(decision_trace))
}

fn structural_interpret(raw: &Value, tool: &ToolSpec) -> AelioResult<IndexMap<String, Value>> {
    let mut out = IndexMap::new();
    if !tool.output_semantics.fields.is_empty() {
        for (name, field) in &tool.output_semantics.fields {
            let value = crate::path::get_path(raw, &field.path)?.ok_or_else(|| {
                AelioError::new(
                    ReasonCode::SigMismatch,
                    format!("declared output field {name} is missing at {}", field.path),
                )
            })?;
            if field.type_name != "auto"
                && !field
                    .type_name
                    .eq_ignore_ascii_case(&value.type_tag().to_string())
            {
                return Err(AelioError::new(
                    ReasonCode::SigMismatch,
                    format!("declared output field {name} has the wrong type"),
                ));
            }
            out.insert(name.clone(), value);
        }
    } else {
        // No declaration means no evidence. Treating an undeclared response shape as implicitly
        // safe would let a tool expand the prompt/memory disclosure surface without catalog
        // review, and a learned signature could then freeze accidental semantics.
    }
    Ok(out)
}

/// Redact declared-sensitive fields out of the extracted record, keyed by declared field name.
/// `extracted`'s keys are the declared output-field names (both the promoted-plan and cold
/// interpret paths key by name), so a name-keyed pass covers every extraction route. `pii` and
/// `secret` alike are stripped: the extracted record is consumed as evidence, and the rendered
/// prompt text sent to the model is never redacted — only its logged copy is.
pub(crate) fn redact_sensitive_extracted(tool: &ToolSpec, extracted: &mut IndexMap<String, Value>) {
    for (name, field) in &tool.output_semantics.fields {
        if !matches!(field.sensitivity, crate::types::Sensitivity::None) {
            if let Some(slot) = extracted.get_mut(name) {
                *slot = Value::str("[REDACTED]");
            }
        }
    }
}

pub(crate) fn sanitize_response(raw: &Value, tool: &ToolSpec) -> AelioResult<Value> {
    // Output semantics are an allowlist, not merely redaction annotations. Tool implementations
    // often add debug/internal fields over time; undeclared data must never reach evidence,
    // learning, logs, or the terminal model.
    let mut safe = Value::Map(IndexMap::new());
    for field in tool.output_semantics.fields.values() {
        let Some(value) = crate::path::get_path(raw, &field.path)? else {
            continue;
        };
        let value = if matches!(field.sensitivity, crate::types::Sensitivity::None) {
            value
        } else {
            Value::str("[REDACTED]")
        };
        safe = crate::path::set_path(&safe, &field.path, value)?;
    }
    Ok(safe)
}

pub fn classify_error(raw: &Value, taxonomy: &[ErrorSpec]) -> (ReasonCode, String) {
    let code = raw
        .as_map()
        .and_then(|m| m.get("error"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    for e in taxonomy {
        if e.match_code == code || code.contains(&e.match_code) {
            let rc = match e.reason.as_str() {
                "rate_limited" => ReasonCode::RateLimited,
                "needs_repair" => ReasonCode::NeedsRepair,
                "terminal" => ReasonCode::Terminal,
                "needs_escalation" => ReasonCode::NeedsEscalation,
                _ => ReasonCode::ToolError,
            };
            return (rc, e.recovery.clone());
        }
    }
    match code {
        "rate_limited" => (ReasonCode::RateLimited, "retryable".into()),
        "invalid_otp" => (ReasonCode::NeedsRepair, "re-ask".into()),
        "account_locked" => (ReasonCode::Terminal, "escalate".into()),
        _ => (ReasonCode::ToolError, "terminal".into()),
    }
}

pub fn recovery_for(code: ReasonCode) -> Recovery {
    code.recovery()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::Predicate;
    use crate::tenant::{
        OutputField, OutputSpec, PolicyAction, PolicyEffect, PolicySpec, PolicySubject,
    };
    use crate::types::Sensitivity;
    use std::collections::HashSet;

    fn secret_echo_tool() -> ToolSpec {
        let mut fields = IndexMap::new();
        fields.insert(
            "otp".into(),
            OutputField {
                path: "$.otp".into(),
                type_name: "str".into(),
                sensitivity: Sensitivity::Secret,
                meaning: "one-time passcode".into(),
            },
        );
        fields.insert(
            "ok".into(),
            OutputField {
                path: "$.ok".into(),
                type_name: "bool".into(),
                sensitivity: Sensitivity::None,
                meaning: "delivery flag".into(),
            },
        );
        ToolSpec {
            id: "send_otp".into(),
            name: "send_otp".into(),
            version: "1".into(),
            capability_tags: vec!["auth.otp.send".into()],
            contract: Some(crate::tenant::ToolContract {
                effect_class: crate::tenant::EffectClass::Write,
                completeness: crate::tenant::Completeness::Complete,
                returns_entity: "otp_delivery_receipt".into(),
                pushdown: vec![],
                max_result_rows: Some(1),
                row_scoped: false,
            }),
            effect: None,
            effectful: true,
            idempotent: false,
            dry_run_available: true,
            params: vec![],
            output_semantics: OutputSpec {
                fields,
                role_hint: None,
            },
            continuations: vec![],
            errors: vec![],
        }
    }

    #[test]
    fn declared_secret_output_never_reaches_extracted_evidence() {
        let tool = secret_echo_tool();
        let mut host = MockToolHost::default().on("send_otp", |_| {
            Ok(Value::Map(indexmap::indexmap! {
                "otp".into() => Value::str("434543"),
                "ok".into() => Value::Bool(true),
            }))
        });
        let mut signatures = SignatureRegistry::default();
        let mut once_seen = HashSet::new();
        let policy = PolicyCtx::default();
        let policies = [PolicySpec {
            id: "allow-test-effect".into(),
            effect: PolicyEffect::Allow,
            subject: PolicySubject::default(),
            action: PolicyAction::default(),
            condition: Predicate::True,
            reason_code: "test".into(),
            priority: 1,
        }];
        let receipt = tool_call_block(
            &tool,
            &IndexMap::new(),
            &mut InvokeContext {
                policies: &policies,
                policy: &policy,
                host: &mut host,
                signatures: &mut signatures,
                once_seen: &mut once_seen,
                user_id: "u1",
                channel: "test",
                turn_key: "turn-secret-output",
                effect_seq: 0,
                element_index: None,
            },
        )
        .expect("tool call succeeds");

        // The extracted record (which the executor merges into evidence → the model prompt) must
        // not carry the raw secret. Structure is preserved (key present) but the value is redacted.
        assert_eq!(
            receipt.extracted.get("otp").and_then(|v| v.as_str()),
            Some("[REDACTED]"),
            "declared secret leaked into extracted evidence"
        );
        assert_eq!(
            receipt.extracted.get("ok").and_then(|v| v.as_bool()),
            Some(true),
            "non-sensitive field should pass through unchanged"
        );

        // The sanitized response (persisted / prompt-facing) must also be clean, and the raw secret
        // must appear nowhere in the receipt's serialized form.
        let serialized = serde_json::to_string(&receipt).unwrap();
        assert!(
            !serialized.contains("434543"),
            "raw secret present in receipt payload: {serialized}"
        );
    }

    #[test]
    fn undeclared_tool_output_is_removed_from_the_safe_response() {
        let tool = secret_echo_tool();
        let safe = sanitize_response(
            &Value::Map(indexmap::indexmap! {
                "ok".into() => Value::Bool(true),
                "otp".into() => Value::str("123456"),
                "internal_debug_dump".into() => Value::str("must-not-escape"),
            }),
            &tool,
        )
        .unwrap();
        assert_eq!(
            crate::path::get_path(&safe, "$.ok").unwrap(),
            Some(Value::Bool(true))
        );
        assert_eq!(
            crate::path::get_path(&safe, "$.otp").unwrap(),
            Some(Value::str("[REDACTED]"))
        );
        assert_eq!(
            crate::path::get_path(&safe, "$.internal_debug_dump").unwrap(),
            None
        );
    }

    #[test]
    fn tool_without_output_contract_produces_no_extracted_evidence() {
        let mut tool = secret_echo_tool();
        tool.output_semantics.fields.clear();
        let extracted = structural_interpret(
            &Value::Map(indexmap::indexmap! {
                "status".into() => Value::str("shipped"),
                "internal_debug_dump".into() => Value::str("must-not-escape"),
            }),
            &tool,
        )
        .unwrap();
        assert!(extracted.is_empty());
    }

    #[test]
    fn effect_sequence_is_part_of_the_durable_idempotency_identity() {
        let policies = [PolicySpec {
            id: "allow-test-effect".into(),
            effect: PolicyEffect::Allow,
            subject: PolicySubject::default(),
            action: PolicyAction::default(),
            condition: Predicate::True,
            reason_code: "test".into(),
            priority: 1,
        }];
        let policy = PolicyCtx::default();
        let mut keys = Vec::new();
        for version in ["1", "2"] {
            let mut tool = secret_echo_tool();
            tool.version = version.into();
            let mut host = MockToolHost::default().on("send_otp", |_| {
                Ok(Value::Map(indexmap::indexmap! {
                    "otp".into() => Value::str("434543"),
                    "ok".into() => Value::Bool(true),
                }))
            });
            let mut signatures = SignatureRegistry::default();
            let mut once_seen = HashSet::new();
            let receipt = tool_call_block(
                &tool,
                &IndexMap::new(),
                &mut InvokeContext {
                    policies: &policies,
                    policy: &policy,
                    host: &mut host,
                    signatures: &mut signatures,
                    once_seen: &mut once_seen,
                    user_id: "u1",
                    channel: "test",
                    turn_key: "turn-key",
                    effect_seq: if version == "1" { 0 } else { 1 },
                    element_index: None,
                },
            )
            .unwrap();
            keys.push(receipt.args_hash);
        }
        assert_ne!(keys[0], keys[1]);
    }

    #[test]
    fn authoritative_trace_survives_a_failed_tool_invocation() {
        struct FailingTracedHost {
            trace: Option<String>,
        }
        impl CapabilityHost for FailingTracedHost {
            fn call_with_context(
                &mut self,
                _tool: &ToolSpec,
                _args: &IndexMap<String, Value>,
                _idempotency_key: &str,
                _user_id: &str,
                _channel: &str,
            ) -> AelioResult<Value> {
                self.trace = Some("{\"steps\":[{\"kind\":\"error\"}]}".into());
                Err(AelioError::new(ReasonCode::NeedsRepair, "invalid otp"))
            }

            fn take_decision_trace(&mut self) -> Option<String> {
                self.trace.take()
            }
        }

        impl ToolHost for FailingTracedHost {
            fn call(
                &mut self,
                _tool_id: &str,
                _args: &IndexMap<String, Value>,
            ) -> AelioResult<Value> {
                Err(AelioError::new(ReasonCode::NeedsRepair, "invalid otp"))
            }
        }

        let policies = [PolicySpec {
            id: "allow-test-effect".into(),
            effect: PolicyEffect::Allow,
            subject: PolicySubject::default(),
            action: PolicyAction::default(),
            condition: Predicate::True,
            reason_code: "test".into(),
            priority: 1,
        }];
        let mut host = FailingTracedHost { trace: None };
        let mut signatures = SignatureRegistry::default();
        let mut once_seen = HashSet::new();
        let error = tool_call_block(
            &secret_echo_tool(),
            &IndexMap::new(),
            &mut InvokeContext {
                policies: &policies,
                policy: &PolicyCtx::default(),
                host: &mut host,
                signatures: &mut signatures,
                once_seen: &mut once_seen,
                user_id: "u1",
                channel: "test",
                turn_key: "turn-traced-failure",
                effect_seq: 0,
                element_index: None,
            },
        )
        .unwrap_err();
        assert_eq!(error.code, ReasonCode::NeedsRepair);
        assert_eq!(
            error.decision_trace.as_deref(),
            Some("{\"steps\":[{\"kind\":\"error\"}]}")
        );
    }
}
