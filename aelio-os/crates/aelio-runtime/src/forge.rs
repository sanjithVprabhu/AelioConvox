//! Flow Forge v0 — cold-path flow authoring.
//!
//! `prompt → (LLM|mock) closed program JSON → Planner → optional store via [`Runtime::push_flow`].
//!
//! Forge v0 authoring boundary (FLAGS F-017 resolution): an LLM may only emit App E control trees
//! over the admitted forge vendor Call catalog; the Planner and artifact gate remain authoritative.
//! Full §16 gate / pathway prototypes are P1; this unit proves draft → validate → durable flow.

use crate::{
    BoundSpec, EffectSpec, FlowPush, OriginSpec, Runtime, RuntimeError, TargetClassSpec, TargetSpec,
};
use aelio_kernel::compile;
use aelio_prompt::{prompt_artifact_hash, ModelPin, PromptArtifact, SlotDecl, TemplateRegistry};
use aelio_sol::{structural_imprint, SolValue};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// Natural-language request to draft (and optionally store) a flow.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeRequest {
    pub tenant: String,
    pub prompt: String,
    /// Optional stable id; otherwise derived from the prompt slug.
    #[serde(default)]
    pub flow_id: Option<String>,
    #[serde(default = "default_flow_rev")]
    pub flow_rev: String,
    /// When true, call [`Runtime::push_flow`] after Planner acceptance.
    #[serde(default)]
    pub store: bool,
}

fn default_flow_rev() -> String {
    "1".into()
}

#[derive(Debug, Clone, Serialize)]
pub struct ForgeResult {
    pub accepted: bool,
    pub stored: bool,
    pub flow: FlowPush,
    pub summary: String,
    pub drafter: String,
    pub authoring_prompt: String,
    pub authoring_prompt_hash: String,
}

/// Something that turns a prompt + catalog into a forge draft JSON object.
pub trait FlowDrafter: Send + Sync {
    fn name(&self) -> &'static str;
    fn draft(&self, prompt: &str, system_prompt: &str) -> Result<ForgeDraft, RuntimeError>;
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeDraft {
    pub flow_id: String,
    pub summary: String,
    pub program: Json,
}

/// Deterministic drafter for local tests — no LLM.
///
/// Keyword heuristics map common intents onto closed App E trees over the forge catalog.
pub struct MockDrafter;

impl FlowDrafter for MockDrafter {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn draft(&self, prompt: &str, _system_prompt: &str) -> Result<ForgeDraft, RuntimeError> {
        let lower = prompt.to_lowercase();
        let (flow_id, summary, program) = if lower.contains("otp") || lower.contains("login") {
            (
                "forge.login_otp",
                "Ask for a phone number, park for the reply, acknowledge.",
                serde_json::json!({
                    "nid":"root","op":"Seq","steps":[
                        {"nid":"ask","op":"Call","id":"forge.say@1",
                         "args":{"text":{"lit":"What is your phone number?"}},"into":"ask1"},
                        {"nid":"wait","op":"Park","until":{"kind":"event"},"into":"reply1"},
                        {"nid":"ok","op":"Call","id":"forge.say@1",
                         "args":{"text":{"lit":"Thanks — we received your number."}},"into":"out"}
                    ]
                }),
            )
        } else if lower.contains("park") || lower.contains("ask") || lower.contains("wait") {
            (
                "forge.ask_wait",
                "Say a prompt, park for the user reply, then acknowledge.",
                serde_json::json!({
                    "nid":"root","op":"Seq","steps":[
                        {"nid":"ask","op":"Call","id":"forge.say@1",
                         "args":{"text":{"lit":"How can I help?"}},"into":"ask1"},
                        {"nid":"wait","op":"Park","until":{"kind":"event"},"into":"reply1"},
                        {"nid":"ok","op":"Call","id":"forge.say@1",
                         "args":{"text":{"lit":"Got it."}},"into":"out"}
                    ]
                }),
            )
        } else {
            (
                "forge.echo",
                "Single express step — baseline forged flow.",
                serde_json::json!({
                    "nid":"root","op":"Call","id":"forge.say@1",
                    "args":{"text":{"lit":"Hello from a forged flow."}},
                    "into":"out"
                }),
            )
        };
        Ok(ForgeDraft {
            flow_id: flow_id.into(),
            summary: summary.into(),
            program,
        })
    }
}

struct HostFlowDrafter<'a> {
    runtime: &'a Runtime,
    tenant: &'a str,
}

impl FlowDrafter for HostFlowDrafter<'_> {
    fn name(&self) -> &'static str {
        "host"
    }

    fn draft(&self, prompt: &str, system_prompt: &str) -> Result<ForgeDraft, RuntimeError> {
        let composed = format!("{system_prompt}\n\nUSER REQUEST:\n{prompt}");
        let prompt_hash = blake3::hash(composed.as_bytes()).to_hex().to_string();
        let correlation = format!("forge:{prompt_hash}");
        let content = self
            .runtime
            .complete_model_text(crate::ModelCompletionRequest {
                tenant: self.tenant,
                prompt: &composed,
                prompt_hash: &prompt_hash,
                model: "aelio.model.forge@1",
                max_tokens: 16_384,
                temperature: 0.0,
                correlation: &correlation,
            })?;
        parse_forge_draft(&content)
    }
}

impl Runtime {
    /// Live Forge authoring through the TypeScript provider boundary.
    pub fn forge_flow_with_host(
        &self,
        request: &ForgeRequest,
    ) -> Result<ForgeResult, RuntimeError> {
        forge_flow(
            Some(self),
            request,
            &HostFlowDrafter {
                runtime: self,
                tenant: &request.tenant,
            },
        )
    }
}

pub fn forge_system_prompt(catalog_json: &str) -> String {
    format!(
        r#"You are Aelio Flow Forge. Emit ONE JSON object only (no markdown) with keys:
  flow_id (string slug like "tenant.ask_phone"),
  summary (one sentence),
  program (App E instruction tree).

Program rules:
- Every node needs unique "nid" and "op".
- Allowed ops: Const, Identity, Seq, Let, Branch, Loop, Try, Fallback, Guard, Budget, Timeout, Once, Park, Tee, Map, Filter, Call.
- Seq children field is ALWAYS "steps" (array). Never "nodes", never "children".
- Call shape: {{"nid","op":"Call","id","args","into"}} — into is mandatory; args values are only {{"lit":...}}, {{"pull":"path"}}, or {{"fn":"op","args":[...]}}.
- Park shape: {{"nid","op":"Park","until":{{"kind":"event"}},"into":"reply1"}}
- You may ONLY Call ids from this vendor catalog (JSON):
{catalog_json}
- Prefer Seq + Call + Park for conversational ask/wait.
- No writes under path prefix "sense".
- Loop requires max_iter > 0; Map/Filter require max_items > 0.

Minimal valid example:
{{
  "flow_id":"forge.ask_wait",
  "summary":"Ask then wait.",
  "program":{{
    "nid":"root","op":"Seq","steps":[
      {{"nid":"ask","op":"Call","id":"forge.say@1","args":{{"text":{{"lit":"Hi"}}}},"into":"ask1"}},
      {{"nid":"wait","op":"Park","until":{{"kind":"event"}},"into":"reply1"}}
    ]
  }}
}}
"#
    )
}

pub fn forge_vendor_prompt() -> PromptArtifact {
    // App-J reserves `{{...}}` for slots. Separate adjacent JSON closing braces in examples (valid
    // JSON whitespace) so they cannot be mistaken for an unmatched template terminator.
    let body = forge_system_prompt("__AELIO_FORGE_CATALOG_SLOT__")
        .replace("}}", "} }")
        .replace("__AELIO_FORGE_CATALOG_SLOT__", "{{catalog_json}}");
    let mut prompt = PromptArtifact {
        template_format: 1,
        id: "aelio.template.flow_forge".into(),
        version: "1".into(),
        description: "Stock Aelio cold-path prompt that authors closed flow programs.".into(),
        objective: "author one bounded flow over the admitted target catalog".into(),
        body,
        slots: vec![SlotDecl {
            name: "catalog_json".into(),
            ty: "str".into(),
            sensitivity: "internal".into(),
            required: true,
        }],
        layers: vec![],
        output_imprint: "aelio.forge.draft@1".into(),
        output_fields: [
            ("flow_id".into(), "str".into()),
            ("summary".into(), "str".into()),
            ("program".into(), "map".into()),
        ]
        .into_iter()
        .collect(),
        model: ModelPin {
            id: "aelio.model.flow_forge@1".into(),
            params: [("temperature".into(), serde_json::json!(0.0))]
                .into_iter()
                .collect(),
        },
        exemplars: vec![],
        is_axiom: false,
        root_version: String::new(),
        composed_hash: String::new(),
        inputs_sufficient: true,
        sufficiency_note: "human-authored vendor artifact".into(),
    };
    prompt.composed_hash = prompt_artifact_hash(&prompt);
    prompt
}

fn compose_forge_prompt(
    prompt: &PromptArtifact,
    catalog_json: &str,
) -> Result<aelio_prompt::ComposedPrompt, RuntimeError> {
    let mut registry = TemplateRegistry::default();
    registry
        .register(prompt.to_template())
        .map_err(RuntimeError::Invalid)?;
    registry
        .compose(
            &prompt.key(),
            &SolValue::map([("catalog_json", SolValue::str(catalog_json))]),
        )
        .map_err(RuntimeError::Invalid)
}

pub fn parse_forge_draft(content: &str) -> Result<ForgeDraft, RuntimeError> {
    let trimmed = content.trim();
    let mut json: Json = serde_json::from_str(trimmed)
        .map_err(|error| RuntimeError::Invalid(format!("forge draft is not JSON: {error}")))?;
    // Authoring-boundary repair: common LLM slip — Seq.nodes → Seq.steps before closed-schema
    // compile. The repaired result still enters the closed Planner and cannot bypass validation.
    if let Some(program) = json.get_mut("program") {
        normalize_seq_steps(program);
    }
    serde_json::from_value(json).map_err(|error| {
        RuntimeError::Invalid(format!(
            "forge draft must be {{flow_id, summary, program}}: {error}"
        ))
    })
}

fn normalize_seq_steps(node: &mut Json) {
    match node {
        Json::Object(map) => {
            let is_seq = map.get("op").and_then(Json::as_str) == Some("Seq");
            if is_seq && map.contains_key("nodes") && !map.contains_key("steps") {
                if let Some(nodes) = map.remove("nodes") {
                    map.insert("steps".into(), nodes);
                }
            }
            for value in map.values_mut() {
                normalize_seq_steps(value);
            }
        }
        Json::Array(items) => {
            for item in items {
                normalize_seq_steps(item);
            }
        }
        _ => {}
    }
}

/// Vendor Call catalog every forged program may use (v0).
pub fn forge_vendor_targets() -> Vec<TargetSpec> {
    let say_in = SolValue::map([("text", SolValue::str("shape"))]);
    let say_out = SolValue::map([("text", SolValue::str("shape"))]);
    vec![TargetSpec {
        id: "forge.say@1".into(),
        class: TargetClassSpec::Io,
        effect: EffectSpec::Read,
        input_imprint: structural_imprint(&say_in),
        output_imprint: structural_imprint(&say_out),
        bounded: BoundSpec::Cost { max_units: 1 },
        policy_tags: vec!["forge.express".into()],
        origin: OriginSpec::Vendor,
    }]
}

pub fn forge_catalog_json() -> String {
    serde_json::to_string_pretty(&serde_json::json!([
        {
            "id": "forge.say@1",
            "class": "io",
            "effect": "read",
            "args": { "text": "str lit or pull — customer-facing line" },
            "into": "map with key text"
        }
    ]))
    .expect("catalog json")
}

/// Draft + Planner-validate; optionally persist via [`Runtime::push_flow`].
pub fn forge_flow(
    runtime: Option<&Runtime>,
    request: &ForgeRequest,
    drafter: &dyn FlowDrafter,
) -> Result<ForgeResult, RuntimeError> {
    if request.tenant.trim().is_empty() {
        return Err(RuntimeError::Invalid("tenant must not be empty".into()));
    }
    if request.prompt.trim().is_empty() {
        return Err(RuntimeError::Invalid("prompt must not be empty".into()));
    }
    if request.store && runtime.is_none() {
        return Err(RuntimeError::Invalid(
            "store=true requires an open Runtime".into(),
        ));
    }

    let catalog = forge_catalog_json();
    let forge_prompt = forge_vendor_prompt();
    if let Some(runtime) = runtime {
        runtime.verify_vendor_prompt(&forge_prompt)?;
    }
    let composed = compose_forge_prompt(&forge_prompt, &catalog)?;
    let mut draft = drafter.draft(request.prompt.trim(), &composed.text)?;
    if let Some(forced) = request.flow_id.as_ref().filter(|id| !id.trim().is_empty()) {
        draft.flow_id = forced.trim().to_string();
    } else if draft.flow_id.trim().is_empty() {
        draft.flow_id = slugify_flow_id(&request.prompt);
    }

    let program_text = serde_json::to_string(&draft.program)
        .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
    let node = compile(&program_text)?;
    ensure_calls_in_catalog(&draft.program)?;

    let flow = FlowPush {
        tenant: request.tenant.clone(),
        flow_id: draft.flow_id.clone(),
        flow_rev: request.flow_rev.clone(),
        program: draft.program.clone(),
        targets: forge_vendor_targets(),
        prompts: vec![],
    };
    // Full registry plan (same path as push_flow).
    let registry = crate::build_registry_for_forge(&flow)?;
    aelio_kernel::plan::plan_with_registry(&node, &registry, &flow.tenant)?;

    let mut stored = false;
    if request.store {
        let runtime = runtime.expect("checked above");
        runtime.push_forged_flow(
            flow.clone(),
            &forge_prompt.key(),
            &composed.prompt_hash,
            drafter.name(),
        )?;
        stored = true;
    }

    Ok(ForgeResult {
        accepted: true,
        stored,
        flow,
        summary: draft.summary,
        drafter: drafter.name().into(),
        authoring_prompt: forge_prompt.key(),
        authoring_prompt_hash: composed.prompt_hash,
    })
}

fn ensure_calls_in_catalog(program: &Json) -> Result<(), RuntimeError> {
    let allowed: std::collections::HashSet<String> =
        forge_vendor_targets().into_iter().map(|t| t.id).collect();
    let mut stack = vec![program];
    while let Some(node) = stack.pop() {
        if let Some(obj) = node.as_object() {
            if obj.get("op").and_then(Json::as_str) == Some("Call") {
                let id = obj
                    .get("id")
                    .and_then(Json::as_str)
                    .ok_or_else(|| RuntimeError::Invalid("Call missing id".into()))?;
                if !allowed.contains(id) {
                    return Err(RuntimeError::Invalid(format!(
                        "Call id `{id}` is not in the forge vendor catalog"
                    )));
                }
            }
            for value in obj.values() {
                stack.push(value);
            }
        } else if let Some(arr) = node.as_array() {
            for value in arr {
                stack.push(value);
            }
        }
    }
    Ok(())
}

fn slugify_flow_id(prompt: &str) -> String {
    let mut out = String::from("forge.");
    for ch in prompt.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if out.ends_with('.') || out.ends_with('_') {
            continue;
        } else {
            out.push('_');
        }
        if out.len() > 48 {
            break;
        }
    }
    if out == "forge." || out == "forge._" {
        "forge.untitled".into()
    } else {
        out.trim_end_matches(['_', '.']).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RuntimeConfig;
    use tempfile::tempdir;

    #[test]
    fn mock_forge_ask_wait_compiles_and_stores() {
        let dir = tempdir().unwrap();
        let runtime = Runtime::open(RuntimeConfig {
            data_dir: dir.path().join("runtime"),
            host_url: None,
            host_token: None,
            event_key_secret: [7u8; 32],
            queue_depth: 8,
        })
        .unwrap();
        let result = forge_flow(
            Some(&runtime),
            &ForgeRequest {
                tenant: "demo".into(),
                prompt: "ask the user something and wait for reply".into(),
                flow_id: Some("forge.test_ask".into()),
                flow_rev: "1".into(),
                store: true,
            },
            &MockDrafter,
        )
        .expect("forge");
        assert!(result.accepted);
        assert!(result.stored);
        assert_eq!(result.flow.flow_id, "forge.test_ask");
        assert_eq!(result.drafter, "mock");
        assert_eq!(result.authoring_prompt, "aelio.template.flow_forge@1");
        assert_eq!(result.authoring_prompt_hash.len(), 64);
        let stored = runtime
            .artifact_repository()
            .unwrap()
            .get("demo", "forge.test_ask", 1)
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.artifact.provenance.metadata["authoring_prompt"],
            result.authoring_prompt
        );
        assert_eq!(
            stored.artifact.provenance.metadata["authoring_prompt_hash"],
            result.authoring_prompt_hash
        );
    }

    #[test]
    fn mock_forge_rejects_empty_prompt() {
        let err = forge_flow(
            None,
            &ForgeRequest {
                tenant: "demo".into(),
                prompt: "  ".into(),
                flow_id: None,
                flow_rev: "1".into(),
                store: false,
            },
            &MockDrafter,
        )
        .unwrap_err();
        assert!(matches!(err, RuntimeError::Invalid(_)));
    }

    #[test]
    fn parse_draft_normalizes_seq_nodes_to_steps() {
        let raw = r#"{"flow_id":"a.b","summary":"s","program":{"nid":"root","op":"Seq","nodes":[{"nid":"x","op":"Identity"}]}}"#;
        let d = parse_forge_draft(raw).unwrap();
        assert!(d.program.get("steps").is_some());
        assert!(d.program.get("nodes").is_none());
    }
}
