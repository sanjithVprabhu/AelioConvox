//! Phase 2 — frozen Call ISA (T3 surface for all Sol harnesses).
//!
//! Authority: `docs/architecture/HARNESS_OS_EXECUTION_PLAN.md` §Phase 2.
//!
//! **Freeze rule.** After this module lands, Phase 3 library work may only compose against
//! ids listed in [`FROZEN_CALL_SPECS`] (plus pure stdlib `math.*` / `logic.*` / `collection.*`
//! from [`crate::stdlib_targets`]). New Call ids require an explicit ISA amendment here.

use crate::compile;
use crate::driver::{Instance, TurnOutcome};
use crate::error::{ErrV1, ReasonCode};
use crate::registry::{Boundedness, Declaration, EffectClass, Origin, Registry, TargetClass};
use aelio_sol::SolValue;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// Max nested harness.invoke / invoke_seq depth (same ceiling as legacy invoke).
pub const HARNESS_CALL_MAX_DEPTH: u32 = 8;

/// One frozen Call id with effect + arg schema documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallSpec {
    pub id: &'static str,
    pub effect: EffectClass,
    pub class: TargetClass,
    /// Human-readable arg schema (not a runtime typecheck yet).
    pub args_schema: &'static str,
    pub summary: &'static str,
}

/// Process / spawn family (plan §2.1).
pub const PROC_FAMILY: &[CallSpec] = &[
    CallSpec {
        id: "harness.invoke@1",
        effect: EffectClass::Read,
        class: TargetClass::Flow,
        args_schema: "{ id: str, ...child_bag }",
        summary: "Run child harness to completion; return completed child bag",
    },
    CallSpec {
        id: "harness.spawn@1",
        effect: EffectClass::Read,
        class: TargetClass::Flow,
        args_schema: "{ id: str, ...child_bag }",
        summary: "Request child stack frame (control signal); parent resumes on return",
    },
    CallSpec {
        id: "harness.invoke_seq@1",
        effect: EffectClass::Read,
        class: TargetClass::Flow,
        args_schema: "{ ids: [str], bags?: [map] }",
        summary: "Invoke N children in declared order; collect outputs into a list",
    },
    CallSpec {
        id: "harness.return@1",
        effect: EffectClass::Pure,
        class: TargetClass::Flow,
        args_schema: "{ out?: map }",
        summary: "Pop frame and return out.* to parent",
    },
    CallSpec {
        id: "harness.exit_up@1",
        effect: EffectClass::Pure,
        class: TargetClass::Flow,
        args_schema: "{}",
        summary: "Pop one stack layer only; ceiling is Conductor",
    },
    CallSpec {
        id: "harness.fresh@1",
        effect: EffectClass::Write,
        // Write+Flow is impossible under Declaration::validate (deadline vs registered-flow).
        // Fresh is a control *signal* consumed by the Conductor host, not a registered-flow body.
        class: TargetClass::Io,
        args_schema: "{}",
        summary: "Clear stack and restart at Conductor",
    },
    CallSpec {
        id: "harness.describe@1",
        effect: EffectClass::Read,
        class: TargetClass::Io,
        args_schema: "{ id: str }",
        summary: "Return contract signature + summary for selection",
    },
    CallSpec {
        id: "harness.list@1",
        effect: EffectClass::Read,
        class: TargetClass::Io,
        args_schema: "{ library?: str, effect?: str }",
        summary: "List installed harness ids filtered by library / effect",
    },
];

/// Tool family (plan §2.2). `tool.act_stub@1` is legacy; prefer `tool.invoke@1`.
pub const TOOL_FAMILY: &[CallSpec] = &[
    CallSpec {
        id: "tool.invoke@1",
        effect: EffectClass::External,
        class: TargetClass::Tool,
        args_schema: "{ tool_id: str, args: map, corr?: str }",
        summary: "Host-proxy tool dispatch (Once-wrapped intent→result; F-026)",
    },
    CallSpec {
        id: "tool.act_stub@1",
        effect: EffectClass::External,
        class: TargetClass::Tool,
        args_schema: "{}",
        summary: "Legacy demo stub; superseded by tool.invoke@1",
    },
];

/// LLM family (plan §2.3). Every call names a stored prompt artifact id.
pub const LLM_FAMILY: &[CallSpec] = &[
    CallSpec {
        id: "llm.classify@1",
        effect: EffectClass::External,
        class: TargetClass::Model,
        args_schema: "{ prompt_id: str, text: str }",
        summary: "Classify text via pinned prompt artifact",
    },
    CallSpec {
        id: "llm.extract@1",
        effect: EffectClass::External,
        class: TargetClass::Model,
        args_schema: "{ prompt_id: str, text: str, schema?: map }",
        summary: "Extract structured fields via pinned prompt",
    },
    CallSpec {
        id: "llm.generate@1",
        effect: EffectClass::External,
        class: TargetClass::Model,
        args_schema: "{ prompt_id: str, text: str }",
        summary: "Generate free text via pinned prompt",
    },
    CallSpec {
        id: "llm.rewrite@1",
        effect: EffectClass::External,
        class: TargetClass::Model,
        args_schema: "{ prompt_id: str, text: str }",
        summary: "Rewrite text via pinned prompt",
    },
    CallSpec {
        id: "llm.summarize@1",
        effect: EffectClass::External,
        class: TargetClass::Model,
        args_schema: "{ prompt_id: str, text: str }",
        summary: "Summarize text via pinned prompt",
    },
    CallSpec {
        id: "llm.judge@1",
        effect: EffectClass::External,
        class: TargetClass::Model,
        args_schema: "{ prompt_id: str, text: str }",
        summary: "Judgement / scoring via pinned prompt",
    },
    CallSpec {
        id: "llm.sanitize@1",
        effect: EffectClass::External,
        class: TargetClass::Model,
        args_schema: "{ prompt_id: str, text: str }",
        summary: "Sanitize / redaction assist via pinned prompt",
    },
    CallSpec {
        id: "llm.embed@1",
        effect: EffectClass::External,
        class: TargetClass::Model,
        args_schema: "{ prompt_id: str, text: str }",
        summary: "Embedding vector via pinned prompt (or embed model pin)",
    },
    CallSpec {
        id: "llm.rerank@1",
        effect: EffectClass::External,
        class: TargetClass::Model,
        args_schema: "{ prompt_id: str, query: str, docs: list }",
        summary: "Rerank documents via pinned prompt",
    },
];

/// Memory / page / slot family (plan §2.4).
pub const MEMORY_FAMILY: &[CallSpec] = &[
    CallSpec {
        id: "memory.search@1",
        effect: EffectClass::Read,
        class: TargetClass::Io,
        args_schema: "{ query?: str }",
        summary: "Search memory; return snippet",
    },
    CallSpec {
        id: "memory.write@1",
        effect: EffectClass::Write,
        class: TargetClass::Io,
        args_schema: "{ key: str, value: any }",
        summary: "Write a memory entry",
    },
    CallSpec {
        id: "memory.forget@1",
        effect: EffectClass::Write,
        class: TargetClass::Io,
        args_schema: "{ key: str }",
        summary: "Forget a memory entry",
    },
    CallSpec {
        id: "context.attach@1",
        effect: EffectClass::Write,
        class: TargetClass::Io,
        args_schema: "{ snippet: str }",
        summary: "Attach context note into bag (legacy name retained)",
    },
    CallSpec {
        id: "page.read@1",
        effect: EffectClass::Read,
        class: TargetClass::Io,
        args_schema: "{ page: str, key?: str }",
        summary: "Read harness-scoped page working memory",
    },
    CallSpec {
        id: "page.write@1",
        effect: EffectClass::Write,
        class: TargetClass::Io,
        args_schema: "{ page: str, key: str, value: any }",
        summary: "Write harness-scoped page slot",
    },
    CallSpec {
        id: "page.append@1",
        effect: EffectClass::Write,
        class: TargetClass::Io,
        args_schema: "{ page: str, key: str, value: any }",
        summary: "Append to a list-valued page slot",
    },
    CallSpec {
        id: "page.compact@1",
        effect: EffectClass::Write,
        class: TargetClass::Io,
        args_schema: "{ page: str }",
        summary: "Compact / drop tmp fields on a page",
    },
    CallSpec {
        id: "slot.set@1",
        effect: EffectClass::Write,
        class: TargetClass::Io,
        args_schema: "{ name: str, value: any }",
        summary: "Set a named slot",
    },
    CallSpec {
        id: "slot.get@1",
        effect: EffectClass::Read,
        class: TargetClass::Io,
        args_schema: "{ name: str, store?: map }",
        summary: "Get a named slot from provided store map",
    },
];

/// Conversation / express / understand demo surface retained for seed harnesses.
pub const CONV_FAMILY: &[CallSpec] = &[
    CallSpec {
        id: "express.say@1",
        effect: EffectClass::Read,
        class: TargetClass::Io,
        args_schema: "{ text: str }",
        summary: "Produce a reply text envelope",
    },
    CallSpec {
        id: "understand.classify@1",
        effect: EffectClass::Read,
        class: TargetClass::Io,
        args_schema: "{ text: str }",
        summary: "Coarse intent clarity label (clear|unclear)",
    },
    CallSpec {
        id: "compute.hold@1",
        effect: EffectClass::Pure,
        class: TargetClass::Compute,
        args_schema: "{ v: any }",
        summary: "Hold a value into the bag (pure passthrough)",
    },
];

/// Union of all Phase-2 frozen families (excluding pure stdlib math/logic/collection).
pub const FROZEN_CALL_SPECS: &[&[CallSpec]] = &[
    PROC_FAMILY,
    TOOL_FAMILY,
    LLM_FAMILY,
    MEMORY_FAMILY,
    CONV_FAMILY,
];

/// Flat list of every frozen Call id.
pub fn frozen_call_ids() -> Vec<&'static str> {
    let mut ids = Vec::new();
    for fam in FROZEN_CALL_SPECS {
        for s in *fam {
            ids.push(s.id);
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

/// Lookup a frozen spec by id.
pub fn frozen_spec(id: &str) -> Option<&'static CallSpec> {
    for fam in FROZEN_CALL_SPECS {
        for s in *fam {
            if s.id == id {
                return Some(s);
            }
        }
    }
    None
}

/// Catalog entry for harness.describe / harness.list.
#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub id: String,
    pub summary: String,
    pub library: String,
    pub effect: String,
    pub version: u32,
    pub identity: String,
}

fn str_arg<'a>(map: &'a std::collections::BTreeMap<String, SolValue>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(|v| match v {
        SolValue::Str(s) => Some(s.as_str()),
        _ => None,
    })
}

fn require_map<'a>(args: &'a SolValue, op: &str) -> Result<&'a std::collections::BTreeMap<String, SolValue>, ErrV1> {
    args.as_map().ok_or_else(|| ErrV1::new(ReasonCode::Type, op, "args must be a map"))
}

fn require_str(map: &std::collections::BTreeMap<String, SolValue>, key: &str, op: &str) -> Result<String, ErrV1> {
    str_arg(map, key)
        .map(|s| s.to_string())
        .ok_or_else(|| ErrV1::new(ReasonCode::Missing, op, format!("args.{key} string required")))
}

fn decl(
    id: &str,
    class: TargetClass,
    effect: EffectClass,
    input: &str,
    output: &str,
) -> Declaration {
    let (boundedness, policy_tags) = if effect.is_effectful() {
        (
            Boundedness::DeadlineCompliant { max_ms: 30_000 },
            vec!["call_isa".into()],
        )
    } else if class == TargetClass::Flow {
        (Boundedness::RegisteredFlow, vec![])
    } else {
        (Boundedness::CostEnvelope { max_units: 1 }, vec![])
    };
    Declaration {
        id: id.into(),
        class,
        input_imprint: input.into(),
        output_imprint: output.into(),
        boundedness,
        effect_class: effect,
        policy_tags,
        tenant: "vendor".into(),
        origin: Origin::Vendor,
    }
}

/// In-memory prompt artifact pin (plan §2.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptArtifactPin {
    pub id: String,
    pub version: u32,
    /// BLAKE3 hex over canonical Sol of {id, version, body}.
    pub identity: String,
    pub body: String,
}

impl PromptArtifactPin {
    pub fn new(id: impl Into<String>, version: u32, body: impl Into<String>) -> Self {
        let id = id.into();
        let body = body.into();
        let envelope = SolValue::map([
            ("id", SolValue::str(&id)),
            ("version", SolValue::Int(version as i64)),
            ("body", SolValue::str(&body)),
        ]);
        let identity = aelio_sol::value_hash(&envelope);
        Self {
            id,
            version,
            identity,
            body,
        }
    }
}

/// Seed prompt pins so llm.* stubs can require a real id.
pub fn seed_prompt_pins() -> HashMap<String, PromptArtifactPin> {
    let pins = [
        PromptArtifactPin::new("prompt.classify.default", 1, "Classify the user utterance."),
        PromptArtifactPin::new("prompt.extract.default", 1, "Extract structured fields."),
        PromptArtifactPin::new("prompt.generate.default", 1, "Generate a helpful reply."),
        PromptArtifactPin::new("prompt.rewrite.default", 1, "Rewrite for clarity."),
        PromptArtifactPin::new("prompt.summarize.default", 1, "Summarize the text."),
        PromptArtifactPin::new("prompt.judge.default", 1, "Judge quality as JSON."),
        PromptArtifactPin::new("prompt.sanitize.default", 1, "Sanitize sensitive content."),
        PromptArtifactPin::new("prompt.embed.default", 1, "Embed text (model pin)."),
        PromptArtifactPin::new("prompt.rerank.default", 1, "Rerank documents."),
    ];
    pins.into_iter().map(|p| (p.id.clone(), p)).collect()
}

/// Register the full frozen Call ISA (stubs + working process helpers).
///
/// - `programs`: child harness id → program_json for invoke / invoke_seq
/// - `catalog`: optional describe/list surface (empty → describe fails Missing)
/// - `prompts`: optional prompt pins for llm.* (default seed pins if None)
pub fn register_frozen_call_isa(
    registry: &mut Registry,
    programs: HashMap<String, String>,
    catalog: Vec<CatalogEntry>,
    prompts: Option<HashMap<String, PromptArtifactPin>>,
) -> Result<(), String> {
    let programs = Arc::new(programs);
    let depth = Arc::new(AtomicU32::new(0));
    let catalog = Arc::new(catalog);
    let prompts = Arc::new(prompts.unwrap_or_else(seed_prompt_pins));

    // ── conv / leaf demos ──────────────────────────────────────────────────
    registry.register_declared(
        decl(
            "compute.hold@1",
            TargetClass::Compute,
            EffectClass::Pure,
            "aelio.any@1",
            "aelio.any@1",
        ),
        |args| {
            Ok(args
                .as_map()
                .and_then(|m| m.get("v"))
                .cloned()
                .unwrap_or(SolValue::Null))
        },
    )?;

    registry.register_declared(
        decl(
            "express.say@1",
            TargetClass::Io,
            EffectClass::Read,
            "aelio.text@1",
            "aelio.reply@1",
        ),
        |args| {
            let text = args
                .as_map()
                .and_then(|m| m.get("text"))
                .cloned()
                .unwrap_or_else(|| SolValue::str(""));
            Ok(SolValue::map([("text", text)]))
        },
    )?;

    registry.register_declared(
        decl(
            "understand.classify@1",
            TargetClass::Io,
            EffectClass::Read,
            "aelio.text@1",
            "aelio.label@1",
        ),
        |args| {
            let text = args
                .as_map()
                .and_then(|m| m.get("text"))
                .and_then(|v| match v {
                    SolValue::Str(s) => Some(s.as_str()),
                    _ => None,
                })
                .unwrap_or("");
            let label = if text.split_whitespace().count() <= 6 && !text.contains('?') {
                "clear"
            } else {
                "unclear"
            };
            Ok(SolValue::map([("label", SolValue::str(label))]))
        },
    )?;

    // ── memory / page / slot ───────────────────────────────────────────────
    registry.register_declared(
        decl(
            "memory.search@1",
            TargetClass::Io,
            EffectClass::Read,
            "aelio.query@1",
            "aelio.snippet@1",
        ),
        |args| {
            let q = args
                .as_map()
                .and_then(|m| m.get("query"))
                .cloned()
                .unwrap_or_else(|| SolValue::str(""));
            Ok(SolValue::map([
                ("snippet", SolValue::str("prior note")),
                ("query", q),
            ]))
        },
    )?;

    registry.register_declared(
        decl(
            "memory.write@1",
            TargetClass::Io,
            EffectClass::Write,
            "aelio.kv@1",
            "aelio.ack@1",
        ),
        |args| {
            let map = require_map(args, "memory.write")?;
            let key = require_str(map, "key", "memory.write")?;
            let value = map.get("value").cloned().unwrap_or(SolValue::Null);
            Ok(SolValue::map([
                ("written", SolValue::str(key)),
                ("value", value),
            ]))
        },
    )?;

    registry.register_declared(
        decl(
            "memory.forget@1",
            TargetClass::Io,
            EffectClass::Write,
            "aelio.key@1",
            "aelio.ack@1",
        ),
        |args| {
            let map = require_map(args, "memory.forget")?;
            let key = require_str(map, "key", "memory.forget")?;
            Ok(SolValue::map([("forgotten", SolValue::str(key))]))
        },
    )?;

    registry.register_declared(
        decl(
            "context.attach@1",
            TargetClass::Io,
            EffectClass::Write,
            "aelio.snippet@1",
            "aelio.note@1",
        ),
        |args| {
            let snippet = args
                .as_map()
                .and_then(|m| m.get("snippet"))
                .cloned()
                .unwrap_or_else(|| SolValue::str(""));
            Ok(SolValue::map([("note", snippet)]))
        },
    )?;

    registry.register_declared(
        decl(
            "page.read@1",
            TargetClass::Io,
            EffectClass::Read,
            "aelio.page@1",
            "aelio.any@1",
        ),
        |args| {
            let map = require_map(args, "page.read")?;
            let page = require_str(map, "page", "page.read")?;
            let key = str_arg(map, "key").unwrap_or("");
            let store = map.get("store").cloned().unwrap_or(SolValue::map::<_, &str>([]));
            let value = if key.is_empty() {
                store
            } else {
                store
                    .as_map()
                    .and_then(|m| m.get(key))
                    .cloned()
                    .unwrap_or(SolValue::Null)
            };
            Ok(SolValue::map([
                ("page", SolValue::str(page)),
                ("value", value),
            ]))
        },
    )?;

    registry.register_declared(
        decl(
            "page.write@1",
            TargetClass::Io,
            EffectClass::Write,
            "aelio.page_kv@1",
            "aelio.ack@1",
        ),
        |args| {
            let map = require_map(args, "page.write")?;
            let page = require_str(map, "page", "page.write")?;
            let key = require_str(map, "key", "page.write")?;
            let value = map.get("value").cloned().unwrap_or(SolValue::Null);
            Ok(SolValue::map([
                ("page", SolValue::str(page)),
                ("key", SolValue::str(key)),
                ("written", value),
            ]))
        },
    )?;

    registry.register_declared(
        decl(
            "page.append@1",
            TargetClass::Io,
            EffectClass::Write,
            "aelio.page_kv@1",
            "aelio.ack@1",
        ),
        |args| {
            let map = require_map(args, "page.append")?;
            let page = require_str(map, "page", "page.append")?;
            let key = require_str(map, "key", "page.append")?;
            let value = map.get("value").cloned().unwrap_or(SolValue::Null);
            Ok(SolValue::map([
                ("page", SolValue::str(page)),
                ("key", SolValue::str(key)),
                ("appended", value),
            ]))
        },
    )?;

    registry.register_declared(
        decl(
            "page.compact@1",
            TargetClass::Io,
            EffectClass::Write,
            "aelio.page@1",
            "aelio.ack@1",
        ),
        |args| {
            let map = require_map(args, "page.compact")?;
            let page = require_str(map, "page", "page.compact")?;
            Ok(SolValue::map([
                ("page", SolValue::str(page)),
                ("compacted", SolValue::Bool(true)),
            ]))
        },
    )?;

    registry.register_declared(
        decl(
            "slot.set@1",
            TargetClass::Io,
            EffectClass::Write,
            "aelio.slot@1",
            "aelio.ack@1",
        ),
        |args| {
            let map = require_map(args, "slot.set")?;
            let name = require_str(map, "name", "slot.set")?;
            let value = map.get("value").cloned().unwrap_or(SolValue::Null);
            Ok(SolValue::map([
                ("name", SolValue::str(name)),
                ("set", value),
            ]))
        },
    )?;

    registry.register_declared(
        decl(
            "slot.get@1",
            TargetClass::Io,
            EffectClass::Read,
            "aelio.slot@1",
            "aelio.any@1",
        ),
        |args| {
            let map = require_map(args, "slot.get")?;
            let name = require_str(map, "name", "slot.get")?;
            let value = map
                .get("store")
                .and_then(|s| s.as_map())
                .and_then(|m| m.get(&name))
                .cloned()
                .unwrap_or(SolValue::Null);
            Ok(SolValue::map([
                ("name", SolValue::str(name)),
                ("value", value),
            ]))
        },
    )?;

    // ── tools (F-026) ──────────────────────────────────────────────────────
    // Default host proxy is a deterministic stub. Production hosts re-register
    // tool.invoke@1 with a reverse-channel adapter that ledgers intent→result
    // and requires Once around effectful invocations.
    registry.register_declared(
        decl(
            "tool.invoke@1",
            TargetClass::Tool,
            EffectClass::External,
            "aelio.tool.request@1",
            "aelio.tool.result@1",
        ),
        |args| {
            let map = require_map(args, "tool.invoke")?;
            let tool_id = require_str(map, "tool_id", "tool.invoke")?;
            let tool_args = map
                .get("args")
                .cloned()
                .unwrap_or_else(|| SolValue::map::<_, &str>([]));
            let corr = str_arg(map, "corr").unwrap_or("").to_string();
            // Deterministic stub — host proxy replaces this in production.
            Ok(SolValue::map([
                ("ok", SolValue::Bool(true)),
                ("tool_id", SolValue::str(tool_id)),
                ("corr", SolValue::str(corr)),
                ("echo", tool_args),
                ("via", SolValue::str("tool.invoke@1")),
            ]))
        },
    )?;

    registry.register_declared(
        decl(
            "tool.act_stub@1",
            TargetClass::Tool,
            EffectClass::External,
            "aelio.unit@1",
            "aelio.tool.result@1",
        ),
        |_| Ok(SolValue::map([("ok", SolValue::Bool(true))])),
    )?;

    // ── llm family ─────────────────────────────────────────────────────────
    for spec in LLM_FAMILY {
        let prompts_c = prompts.clone();
        let call_id = spec.id;
        registry.register_declared(
            decl(
                call_id,
                TargetClass::Model,
                EffectClass::External,
                "aelio.llm.request@1",
                "aelio.llm.result@1",
            ),
            move |args| {
                let map = require_map(args, call_id)?;
                let prompt_id = require_str(map, "prompt_id", call_id)?;
                if !prompts_c.contains_key(&prompt_id) {
                    return Err(ErrV1::new(
                        ReasonCode::Missing,
                        call_id,
                        format!("unknown prompt artifact id={prompt_id}"),
                    ));
                }
                let pin = &prompts_c[&prompt_id];
                llm_stub_result(call_id, map, pin)
            },
        )?;
    }

    // ── process control signals ────────────────────────────────────────────
    registry.register_declared(
        decl(
            "harness.return@1",
            TargetClass::Flow,
            EffectClass::Pure,
            "aelio.out@1",
            "aelio.control@1",
        ),
        |args| {
            let out = args
                .as_map()
                .and_then(|m| m.get("out"))
                .cloned()
                .unwrap_or_else(|| SolValue::map::<_, &str>([]));
            Ok(SolValue::map([
                ("action", SolValue::str("return")),
                ("out", out),
            ]))
        },
    )?;

    registry.register_declared(
        decl(
            "harness.exit_up@1",
            TargetClass::Flow,
            EffectClass::Pure,
            "aelio.unit@1",
            "aelio.control@1",
        ),
        |_| {
            Ok(SolValue::map([
                ("action", SolValue::str("exit_up")),
            ]))
        },
    )?;

    registry.register_declared(
        decl(
            "harness.fresh@1",
            TargetClass::Io,
            EffectClass::Write,
            "aelio.unit@1",
            "aelio.control@1",
        ),
        |_| {
            Ok(SolValue::map([
                ("action", SolValue::str("fresh")),
            ]))
        },
    )?;

    registry.register_declared(
        decl(
            "harness.spawn@1",
            TargetClass::Flow,
            EffectClass::Read,
            "aelio.spawn@1",
            "aelio.control@1",
        ),
        |args| {
            let map = require_map(args, "harness.spawn")?;
            let id = require_str(map, "id", "harness.spawn")?;
            // Control signal for the Conductor / process tree host to push a frame.
            // Synchronous child execution remains harness.invoke@1.
            Ok(SolValue::map([
                ("action", SolValue::str("spawn")),
                ("id", SolValue::str(id)),
                (
                    "bag",
                    SolValue::map(
                        map.iter()
                            .filter(|(k, _)| k.as_str() != "id")
                            .map(|(k, v)| (k.clone(), v.clone())),
                    ),
                ),
            ]))
        },
    )?;

    // describe / list
    {
        let cat = catalog.clone();
        registry.register_declared(
            decl(
                "harness.describe@1",
                TargetClass::Io,
                EffectClass::Read,
                "aelio.harness_id@1",
                "aelio.harness_desc@1",
            ),
            move |args| {
                let map = require_map(args, "harness.describe")?;
                let id = require_str(map, "id", "harness.describe")?;
                let entry = cat.iter().find(|e| e.id == id).ok_or_else(|| {
                    ErrV1::new(
                        ReasonCode::Missing,
                        "harness.describe",
                        format!("unknown harness id={id}"),
                    )
                })?;
                Ok(SolValue::map([
                    ("id", SolValue::str(&entry.id)),
                    ("summary", SolValue::str(&entry.summary)),
                    ("library", SolValue::str(&entry.library)),
                    ("effect", SolValue::str(&entry.effect)),
                    ("version", SolValue::Int(entry.version as i64)),
                    ("identity", SolValue::str(&entry.identity)),
                ]))
            },
        )?;
    }

    {
        let cat = catalog.clone();
        registry.register_declared(
            decl(
                "harness.list@1",
                TargetClass::Io,
                EffectClass::Read,
                "aelio.list_filter@1",
                "aelio.id_list@1",
            ),
            move |args| {
                let map = args.as_map();
                let lib_filter = map.and_then(|m| str_arg(m, "library"));
                let effect_filter = map.and_then(|m| str_arg(m, "effect"));
                let ids: Vec<SolValue> = cat
                    .iter()
                    .filter(|e| lib_filter.map(|l| e.library == l).unwrap_or(true))
                    .filter(|e| effect_filter.map(|ef| e.effect == ef).unwrap_or(true))
                    .map(|e| SolValue::str(&e.id))
                    .collect();
                Ok(SolValue::map([("ids", SolValue::List(ids))]))
            },
        )?;
    }

    // harness.invoke@1 + invoke_seq@1 (working nested Sol)
    register_invoke_family(registry, programs, depth)?;

    Ok(())
}

fn llm_stub_result(
    call_id: &str,
    map: &std::collections::BTreeMap<String, SolValue>,
    pin: &PromptArtifactPin,
) -> Result<SolValue, ErrV1> {
    let text = map
        .get("text")
        .or_else(|| map.get("query"))
        .cloned()
        .unwrap_or_else(|| SolValue::str(""));
    let base = [
        ("prompt_id", SolValue::str(&pin.id)),
        ("prompt_identity", SolValue::str(&pin.identity)),
        ("call", SolValue::str(call_id)),
    ];
    match call_id {
        "llm.classify@1" => Ok(SolValue::map(
            base.into_iter().chain([
                ("label", SolValue::str("other")),
                ("confidence", SolValue::float(0.5).unwrap()),
            ]),
        )),
        "llm.extract@1" => Ok(SolValue::map(
            base.into_iter().chain([
                ("fields", SolValue::map::<_, &str>([])),
                ("text", text),
            ]),
        )),
        "llm.generate@1" | "llm.rewrite@1" | "llm.summarize@1" | "llm.sanitize@1" => {
            Ok(SolValue::map(base.into_iter().chain([(
                "text",
                SolValue::str(format!("[{call_id}] stub")),
            )])))
        }
        "llm.judge@1" => Ok(SolValue::map(base.into_iter().chain([
            ("score", SolValue::float(0.0).unwrap()),
            ("pass", SolValue::Bool(false)),
        ]))),
        "llm.embed@1" => Ok(SolValue::map(base.into_iter().chain([(
            "vector",
            SolValue::List(vec![
                SolValue::float(0.0).unwrap(),
                SolValue::float(0.0).unwrap(),
            ]),
        )]))),
        "llm.rerank@1" => {
            let docs = map
                .get("docs")
                .cloned()
                .unwrap_or_else(|| SolValue::List(vec![]));
            Ok(SolValue::map(
                base.into_iter()
                    .chain([("ranked", docs), ("query", text)]),
            ))
        }
        _ => Err(ErrV1::new(
            ReasonCode::Internal,
            call_id,
            "unhandled llm stub",
        )),
    }
}

fn register_invoke_family(
    registry: &mut Registry,
    programs: Arc<HashMap<String, String>>,
    depth: Arc<AtomicU32>,
) -> Result<(), String> {
    // invoke
    {
        let programs_c = programs.clone();
        let depth_c = depth.clone();
        registry.register_declared(
            decl(
                "harness.invoke@1",
                TargetClass::Flow,
                EffectClass::Read,
                "aelio.invoke@1",
                "aelio.bag@1",
            ),
            move |args| invoke_one(&programs_c, &depth_c, args),
        )?;
    }

    // invoke_seq — deterministic ordered fan-out/join
    {
        let programs_c = programs.clone();
        let depth_c = depth.clone();
        registry.register_declared(
            decl(
                "harness.invoke_seq@1",
                TargetClass::Flow,
                EffectClass::Read,
                "aelio.invoke_seq@1",
                "aelio.bag_list@1",
            ),
            move |args| {
                let map = require_map(args, "harness.invoke_seq")?;
                let ids = map.get("ids").and_then(|v| v.as_list()).ok_or_else(|| {
                    ErrV1::new(
                        ReasonCode::Missing,
                        "harness.invoke_seq",
                        "args.ids list required",
                    )
                })?;
                let bags = map.get("bags").and_then(|v| v.as_list());
                let mut outs = Vec::with_capacity(ids.len());
                for (i, id_v) in ids.iter().enumerate() {
                    let id = match id_v {
                        SolValue::Str(s) => s.as_str(),
                        _ => {
                            return Err(ErrV1::new(
                                ReasonCode::Type,
                                "harness.invoke_seq",
                                "ids must be strings",
                            ))
                        }
                    };
                    let mut child_args = std::collections::BTreeMap::new();
                    child_args.insert("id".into(), SolValue::str(id));
                    if let Some(list) = bags {
                        if let Some(bag) = list.get(i) {
                            if let Some(bm) = bag.as_map() {
                                for (k, v) in bm {
                                    if k != "id" {
                                        child_args.insert(k.clone(), v.clone());
                                    }
                                }
                            }
                        }
                    }
                    let bag = invoke_one(
                        &programs_c,
                        &depth_c,
                        &SolValue::Map(child_args),
                    )?;
                    outs.push(bag);
                }
                Ok(SolValue::map([("results", SolValue::List(outs))]))
            },
        )?;
    }

    Ok(())
}

fn invoke_one(
    programs: &Arc<HashMap<String, String>>,
    depth: &Arc<AtomicU32>,
    args: &SolValue,
) -> Result<SolValue, ErrV1> {
    let map = require_map(args, "harness.invoke")?;
    let id = require_str(map, "id", "harness.invoke")?;
    let cur = depth.fetch_add(1, Ordering::SeqCst);
    if cur >= HARNESS_CALL_MAX_DEPTH {
        depth.fetch_sub(1, Ordering::SeqCst);
        return Err(ErrV1::new(
            ReasonCode::BudgetCalls,
            "harness.invoke",
            format!("harness invoke depth exceeded max={HARNESS_CALL_MAX_DEPTH}"),
        ));
    }
    let run = (|| {
        let program_json = programs.get(&id).ok_or_else(|| {
            ErrV1::new(
                ReasonCode::Missing,
                "harness.invoke",
                format!("unknown child harness id={id}"),
            )
        })?;
        let child_bag = if map.len() <= 1 {
            SolValue::map::<_, &str>([])
        } else {
            SolValue::map(
                map.iter()
                    .filter(|(k, _)| k.as_str() != "id")
                    .map(|(k, v)| (k.clone(), v.clone())),
            )
        };
        let program = compile(program_json).map_err(|e| {
            ErrV1::new(
                ReasonCode::Shape,
                "harness.invoke",
                format!("child compile failed: {e:?}"),
            )
        })?;
        // Nested registry: re-register frozen ISA with same programs for child Calls.
        let mut child_reg = Registry::default();
        register_frozen_call_isa(
            &mut child_reg,
            programs.as_ref().clone(),
            Vec::new(),
            None,
        )
        .map_err(|e| ErrV1::new(ReasonCode::Internal, "harness.invoke", e))?;
        // Pure stdlib for children that call math.*
        let _ = crate::stdlib_targets::register_p0_pure_stdlib(&mut child_reg);
        let mut inst = Instance::new(program, &mut child_reg);
        match inst.start(child_bag).map_err(|e| {
            ErrV1::new(
                ReasonCode::Internal,
                "harness.invoke",
                format!("child start failed: {e:?}"),
            )
        })? {
            TurnOutcome::Completed { bag, .. } => Ok(bag),
            TurnOutcome::Parked(_) => Err(ErrV1::new(
                ReasonCode::Internal,
                "harness.invoke",
                "child harness parked; parent/child park handoff not in this path",
            )),
        }
    })();
    depth.fetch_sub(1, Ordering::SeqCst);
    run
}

/// Catalog entries derived from the in-memory seed Sol library.
pub fn catalog_from_seed_library() -> Vec<CatalogEntry> {
    crate::sol_harness_library()
        .into_iter()
        .map(|c| CatalogEntry {
            id: c.id,
            summary: c.summary,
            library: c.library,
            effect: crate::harness_contract::effect_to_str(c.effect).into(),
            version: c.version,
            identity: c.identity,
        })
        .collect()
}

/// Full admission registry: pure stdlib + frozen Call ISA + seed catalog + library programs.
pub fn frozen_admission_registry() -> Registry {
    let programs = crate::sol_harness_lib::library_program_map();
    let catalog = catalog_from_seed_library();
    let mut r = Registry::default();
    crate::stdlib_targets::register_p0_pure_stdlib(&mut r)
        .expect("P0 pure stdlib must register");
    register_frozen_call_isa(&mut r, programs, catalog, None)
        .expect("frozen Call ISA must register");
    // Flow stack targets used by proper_sol_* seeds (not part of generic ISA).
    for flow_id in [
        crate::sol_harness_lib::FLOW_STACK_LEAF_C,
        crate::sol_harness_lib::FLOW_STACK_MID_B,
        crate::sol_harness_lib::FLOW_STACK_TOP_A,
    ] {
        if r.effect_of(flow_id).is_none() {
            let _ = r.register_declared(
                decl(
                    flow_id,
                    TargetClass::Flow,
                    EffectClass::Read,
                    "aelio.stack_in@1",
                    "aelio.stack_out@1",
                ),
                |_| Ok(SolValue::map::<_, &str>([])),
            );
        }
    }
    r
}

/// True if `id` is in the frozen ISA or pure P0 stdlib surface.
pub fn is_allowed_call_id(id: &str) -> bool {
    if frozen_spec(id).is_some() {
        return true;
    }
    // Flow stack pins used by proper Sol demos (seed-only, not general ISA).
    if matches!(
        id,
        "flow.stack_leaf_c@1" | "flow.stack_mid_b@1" | "flow.stack_top_a@1"
    ) {
        return true;
    }
    // Pure stdlib namespaces
    id.starts_with("math.")
        || id.starts_with("logic.")
        || id.starts_with("collection.")
        || id.starts_with("str.")
        || id.starts_with("list.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness_contract::{admit_contract, collect_call_ids};
    use crate::sol_harness_lib::{
        library_program_map, parent_waits_on_child_contract, sol_harness_library,
        HARNESS_INVOKE_MAX_DEPTH,
    };

    #[test]
    fn frozen_ids_are_unique_and_version_pinned() {
        let ids = frozen_call_ids();
        assert!(ids.len() >= 30, "expected full ISA, got {}", ids.len());
        let mut seen = std::collections::HashSet::new();
        for id in &ids {
            assert!(id.contains('@'), "must be version-pinned: {id}");
            assert!(seen.insert(*id), "duplicate frozen id {id}");
        }
    }

    #[test]
    fn every_frozen_id_registers_with_effect() {
        let r = frozen_admission_registry();
        for id in frozen_call_ids() {
            assert!(
                r.effect_of(id).is_some(),
                "frozen id {id} not registered"
            );
            let spec = frozen_spec(id).unwrap();
            assert_eq!(r.effect_of(id), Some(spec.effect));
        }
    }

    #[test]
    fn seed_library_call_ids_subset_of_allowed() {
        for c in sol_harness_library() {
            let calls = collect_call_ids(&c.program_json).unwrap();
            for id in calls {
                assert!(
                    is_allowed_call_id(&id),
                    "seed harness {} uses non-ISA Call `{id}`",
                    c.id
                );
            }
        }
    }

    #[test]
    fn every_seed_admits_under_frozen_registry() {
        let reg = frozen_admission_registry();
        for c in sol_harness_library() {
            admit_contract(
                &c.id,
                c.version,
                &c.program_json,
                c.effect,
                &c.budget,
                Some(&reg),
                "demo",
            )
            .unwrap_or_else(|e| panic!("admit {}: {:?}", c.id, e));
        }
    }

    #[test]
    fn invoke_seq_runs_children_in_order() {
        let programs = library_program_map();
        assert!(programs.contains_key("quick_reply"));
        let mut r = frozen_admission_registry();
        let bags = SolValue::List(vec![
            SolValue::map([("utterance", SolValue::str("hi"))]),
            SolValue::map([("utterance", SolValue::str("bye"))]),
        ]);
        let args = SolValue::map([
            (
                "ids",
                SolValue::List(vec![
                    SolValue::str("quick_reply"),
                    SolValue::str("apologize_closed"),
                ]),
            ),
            ("bags", bags),
        ]);
        let out = r
            .call("harness.invoke_seq@1", &args)
            .expect("registered")
            .expect("ok");
        let results = out
            .as_map()
            .and_then(|m| m.get("results"))
            .and_then(|v| v.as_list())
            .expect("results list");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn describe_and_list_from_catalog() {
        let mut r = frozen_admission_registry();
        let desc = r
            .call(
                "harness.describe@1",
                &SolValue::map([("id", SolValue::str("quick_reply"))]),
            )
            .unwrap()
            .unwrap();
        let summary = desc
            .as_map()
            .and_then(|m| m.get("summary"))
            .and_then(|v| match v {
                SolValue::Str(s) => Some(s.as_str()),
                _ => None,
            });
        assert!(summary.is_some_and(|s| !s.is_empty()));

        let listed = r
            .call(
                "harness.list@1",
                &SolValue::map([("library", SolValue::str("conv"))]),
            )
            .unwrap()
            .unwrap();
        let ids = listed
            .as_map()
            .and_then(|m| m.get("ids"))
            .and_then(|v| v.as_list())
            .unwrap();
        assert!(ids.iter().any(|v| matches!(v, SolValue::Str(s) if s == "quick_reply")));
    }

    #[test]
    fn tool_invoke_stub_echoes() {
        let mut r = frozen_admission_registry();
        let out = r
            .call(
                "tool.invoke@1",
                &SolValue::map([
                    ("tool_id", SolValue::str("send_otp")),
                    ("args", SolValue::map([("phone", SolValue::str("900"))])),
                    ("corr", SolValue::str("c1")),
                ]),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            out.as_map().and_then(|m| m.get("ok")),
            Some(&SolValue::Bool(true))
        );
        assert_eq!(
            out.as_map().and_then(|m| m.get("tool_id")),
            Some(&SolValue::str("send_otp"))
        );
    }

    #[test]
    fn llm_requires_known_prompt_id() {
        let mut r = frozen_admission_registry();
        let err = r
            .call(
                "llm.classify@1",
                &SolValue::map([
                    ("prompt_id", SolValue::str("prompt.missing")),
                    ("text", SolValue::str("hi")),
                ]),
            )
            .unwrap()
            .unwrap_err();
        assert!(err.detail.contains("unknown prompt"));

        let ok = r
            .call(
                "llm.classify@1",
                &SolValue::map([
                    ("prompt_id", SolValue::str("prompt.classify.default")),
                    ("text", SolValue::str("hi")),
                ]),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            ok.as_map().and_then(|m| m.get("label")),
            Some(&SolValue::str("other"))
        );
    }

    #[test]
    fn control_signals_return_action() {
        let mut r = frozen_admission_registry();
        for (id, action) in [
            ("harness.exit_up@1", "exit_up"),
            ("harness.fresh@1", "fresh"),
        ] {
            let out = r.call(id, &SolValue::map::<_, &str>([])).unwrap().unwrap();
            assert_eq!(
                out.as_map().and_then(|m| m.get("action")),
                Some(&SolValue::str(action))
            );
        }
        let ret = r
            .call(
                "harness.return@1",
                &SolValue::map([("out", SolValue::map([("x", SolValue::Int(1))]))]),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            ret.as_map().and_then(|m| m.get("action")),
            Some(&SolValue::str("return"))
        );
    }

    #[test]
    fn parent_invoke_still_works_via_frozen_registry() {
        let parent = parent_waits_on_child_contract();
        let mut registry = frozen_admission_registry();
        let mut inst = Instance::new(
            crate::compile(&parent.program_json).unwrap(),
            &mut registry,
        );
        let bag = match inst.start(SolValue::map::<_, &str>([])).unwrap() {
            TurnOutcome::Completed { bag, .. } => bag,
            TurnOutcome::Parked(_) => panic!("should complete"),
        };
        let child_sum = bag.as_map().and_then(|m| m.get("child")).and_then(|c| {
            c.as_map().and_then(|m| m.get("sum")).cloned()
        });
        assert_eq!(child_sum, Some(SolValue::Int(15)));
        assert_eq!(HARNESS_CALL_MAX_DEPTH, HARNESS_INVOKE_MAX_DEPTH);
    }

    #[test]
    fn prompt_pin_identity_stable() {
        let a = PromptArtifactPin::new("prompt.x", 1, "body");
        let b = PromptArtifactPin::new("prompt.x", 1, "body");
        assert_eq!(a.identity, b.identity);
        assert_eq!(a.identity.len(), 64);
        let c = PromptArtifactPin::new("prompt.x", 2, "body");
        assert_ne!(a.identity, c.identity);
    }

}
