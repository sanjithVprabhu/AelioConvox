//! ToolCallBlock composition (policy → bind → ledger → once call → sig → extract).

use crate::abilities::bind::{resolve_all, BindSources};
use crate::abilities::express;
use crate::abilities::invoke::{tool_call_block, CapabilityHost, InvokeContext, InvokeReceipt};
use crate::abilities::sig::SignatureRegistry;
use crate::ops::effects::EffectEnv;
use crate::policy::PolicyCtx;
use crate::tenant::{PolicySpec, ToolSpec};
use crate::types::{AelioResult, Sensitivity, Value};
use indexmap::IndexMap;
use std::collections::HashSet;

#[derive(Debug)]
pub enum ToolCallOutcome {
    NeedUser {
        missing: Vec<String>,
        question: String,
    },
    Done {
        receipt: InvokeReceipt,
    },
}

pub struct ToolCallContext<'a> {
    pub policies: &'a [PolicySpec],
    pub policy: &'a PolicyCtx,
    pub host: &'a mut dyn CapabilityHost,
    pub signatures: &'a mut SignatureRegistry,
    pub once_seen: &'a mut HashSet<String>,
    pub effects: &'a mut EffectEnv,
    pub user_id: &'a str,
    pub channel: &'a str,
    /// Client-supplied retry-stable turn identity; never derived from tool arguments.
    pub turn_key: &'a str,
    /// Monotonic effect position within the turn.
    pub effect_seq: &'a mut u64,
    /// Map child position when this call is executing per-element.
    pub element_index: Option<u64>,
}

pub fn run_tool_call_block(
    tool: &ToolSpec,
    sources: &BindSources,
    context: &mut ToolCallContext<'_>,
) -> AelioResult<ToolCallOutcome> {
    let resolved = resolve_all(tool, sources)?;
    if !resolved.residual.is_empty() {
        let slot = &resolved.residual[0];
        let hint = tool
            .params
            .iter()
            .find(|p| p.name == *slot)
            .and_then(|p| p.prompt_hint.as_deref());
        let q = express::ask(slot, hint, 1);
        return Ok(ToolCallOutcome::NeedUser {
            missing: resolved.residual,
            question: q.text,
        });
    }

    let safe_args: IndexMap<String, Value> = resolved
        .bound
        .iter()
        .map(|(name, value)| {
            let sensitive = tool
                .params
                .iter()
                .find(|param| param.name == *name)
                .is_some_and(|param| !matches!(param.sensitivity, Sensitivity::None));
            (
                name.clone(),
                if sensitive {
                    Value::str("[REDACTED]")
                } else {
                    value.clone()
                },
            )
        })
        .collect();

    // Ledger intent (pending) BEFORE effect. Sensitive argument values never enter the ledger.
    context.effects.ledger_append(
        "intent",
        Value::Map(indexmap::indexmap! {
            "tool".into() => Value::str(&tool.id),
            "args".into() => Value::Map(safe_args),
            "status".into() => Value::str("pending"),
        }),
    )?;

    let effect_seq = *context.effect_seq;
    *context.effect_seq += 1;
    let receipt = match tool_call_block(
        tool,
        &resolved.bound,
        &mut InvokeContext {
            policies: context.policies,
            policy: context.policy,
            host: context.host,
            signatures: context.signatures,
            once_seen: context.once_seen,
            user_id: context.user_id,
            channel: context.channel,
            turn_key: context.turn_key,
            effect_seq,
            element_index: context.element_index,
        },
    ) {
        Ok(receipt) => receipt,
        Err(error) => {
            context.effects.ledger_append(
                "receipt",
                Value::Map(indexmap::indexmap! {
                    "tool".into() => Value::str(&tool.id),
                    "status".into() => Value::str("error"),
                    "reason_code".into() => Value::str(format!("{:?}", error.code).to_lowercase()),
                }),
            )?;
            return Err(error);
        }
    };

    context.effects.ledger_append(
        "receipt",
        Value::Map(indexmap::indexmap! {
            "tool".into() => Value::str(&tool.id),
            "status".into() => Value::str("success"),
            "sig_hash".into() => Value::str(&receipt.sig_hash),
        }),
    )?;

    // Continuations: enqueue declared expected next from ToolSpec
    if !tool.continuations.is_empty() {
        context.effects.ledger_append(
            "continuation",
            Value::List(tool.continuations.iter().map(Value::str).collect()),
        )?;
    }

    Ok(ToolCallOutcome::Done { receipt })
}

pub fn bind_sources_from_maps(
    slots: IndexMap<String, Value>,
    env: IndexMap<String, Value>,
    state: IndexMap<String, Value>,
    tool_outputs: IndexMap<String, Value>,
) -> BindSources {
    BindSources {
        slots,
        env,
        state,
        tool_outputs,
        user_text: None,
    }
}
