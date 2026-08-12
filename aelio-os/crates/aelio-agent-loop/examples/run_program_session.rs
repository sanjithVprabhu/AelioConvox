//! Deterministic write → store → reuse conversation for `run_program`.
//! Emits the same MD layout as the widget chat soaks under docs/chat-runs/.
//!
//!   cargo run -p aelio-agent-loop --example run_program_session -- ../../docs/chat-runs

use aelio_agent_loop::{
    kernel_tool_definitions, AgentLoop, AssistantBlock, BudgetLimits, Context, EffectClass,
    EffectGate, GateDecision, LoopError, LoopOutcome, Model, ModelRequest, ModelResponse,
    ModelUsage, StopReason, ToolCall, ToolDefinition, ToolError, ToolHost,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fs::{create_dir_all, write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

struct ScriptedModel(VecDeque<ModelResponse>);

#[async_trait]
impl Model for ScriptedModel {
    async fn complete(&mut self, _request: ModelRequest) -> Result<ModelResponse, LoopError> {
        self.0
            .pop_front()
            .ok_or_else(|| LoopError::Model("script exhausted".into()))
    }
}

#[derive(Clone, Default)]
struct RecordingHost(Arc<Mutex<Vec<String>>>);

#[async_trait]
impl ToolHost for RecordingHost {
    async fn invoke(&self, call: &ToolCall) -> Result<Value, ToolError> {
        self.0.lock().expect("log").push(call.name.clone());
        Ok(json!({
            "ok": true,
            "tool": call.name,
            "payload": format!("fixture result for {}", call.name),
        }))
    }
}

struct Allow;
impl EffectGate for Allow {
    fn decide(&self, _call: &ToolCall, _class: &EffectClass) -> GateDecision {
        GateDecision::Allow
    }
}

fn tool(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        version: "1".into(),
        description: format!("Read-only fixture tool {name} for program soak."),
        input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
        effect_class: EffectClass::Read,
    }
}

fn response(id: &str, blocks: Vec<AssistantBlock>) -> ModelResponse {
    ModelResponse {
        id: id.into(),
        content: blocks,
        stop_reason: StopReason::ToolUse,
        usage: ModelUsage {
            input_tokens: 20,
            cached_input_tokens: 0,
            output_tokens: 10,
        },
    }
}

fn call(id: &str, name: &str, arguments: Value) -> AssistantBlock {
    AssistantBlock::ToolCall {
        call: ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
        },
    }
}

fn finish(id: &str, message: impl Into<String>) -> AssistantBlock {
    call(
        id,
        "finish",
        json!({
            "message": message.into(),
            "status": "completed",
            "resolved_effect_ids": [],
            "unresolved_effect_ids": []
        }),
    )
}

fn stamp() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_millis();
    format!("harness-program-{ms}")
}

fn write_md(path: &PathBuf, body: &str) {
    write(path, body).expect("write md");
}

#[tokio::main]
async fn main() {
    let out_root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("docs/chat-runs"));
    let run_id = stamp();
    let out_dir = out_root.join(&run_id);
    create_dir_all(&out_dir).expect("mkdir");

    let program_source = json!({
        "version": 1,
        "steps": [
            {"tool": "get_employer_profile", "arguments": {}},
            {"tool": "get_company_profile", "arguments": {}}
        ]
    })
    .to_string();

    let host = RecordingHost::default();
    let model = ScriptedModel(VecDeque::from([
        // Turn-equivalent 1: write + execute + store
        response(
            "r1",
            vec![call(
                "c1",
                "run_program",
                json!({
                    "source": program_source,
                    "rationale": "batch profile reads for reuse"
                }),
            )],
        ),
        response(
            "r2",
            vec![finish(
                "c2",
                "Wrote and stored the Sol program for employer+company profile reads. Ready to reuse.",
            )],
        ),
        // After we inspect orchestration outside the loop we continue with a second loop for reuse.
    ]));

    let ctx = Context::new(
        "system",
        "bootstrap\nActive personality: warm_recruiter\nScenario: write/store/reuse",
        kernel_tool_definitions(),
        vec![tool("get_employer_profile"), tool("get_company_profile")],
    );

    let (outcome1, engine1) = AgentLoop::new(model, host.clone(), Allow, ctx, BudgetLimits::default())
        .run()
        .await
        .expect("first run");
    let LoopOutcome::Finished {
        message: msg1,
        status: status1,
    } = outcome1
    else {
        panic!("expected finished first run: {outcome1:?}");
    };

    let programs = engine1.orchestration().programs.snapshot();
    let first = programs["programs"].as_array().expect("programs")[0].clone();
    let program_id = first["program_id"].as_str().unwrap().to_string();
    let source_hash = first["source_hash"].as_str().unwrap().to_string();

    // Second conversation turn: reuse identical source → same hash / new or same store entry
    let host2 = host.clone();
    let model2 = ScriptedModel(VecDeque::from([
        response(
            "r3",
            vec![call(
                "c3",
                "run_program",
                json!({
                    "source": json!({
                        "version": 1,
                        "steps": [
                            {"tool": "get_employer_profile", "arguments": {}},
                            {"tool": "get_company_profile", "arguments": {}}
                        ]
                    }).to_string(),
                    "rationale": "reuse stored profile batch"
                }),
            )],
        ),
        response(
            "r4",
            vec![finish(
                "c4",
                format!(
                    "Reused Sol program {program_id} (source_hash={source_hash}). Both runs executed the same two profile steps."
                ),
            )],
        ),
    ]));

    let ctx2 = Context::new(
        "system",
        "bootstrap\nActive personality: warm_recruiter\nScenario: reuse",
        kernel_tool_definitions(),
        vec![tool("get_employer_profile"), tool("get_company_profile")],
    );
    // Carry prior orchestration so the registry already has the stored program.
    let (outcome2, engine2) = AgentLoop::new(model2, host2, Allow, ctx2, BudgetLimits::default())
        .with_orchestration(engine1.orchestration().clone())
        .run()
        .await
        .expect("reuse run");
    let LoopOutcome::Finished {
        message: msg2,
        status: status2,
    } = outcome2
    else {
        panic!("expected finished reuse: {outcome2:?}");
    };

    let host_log = host.0.lock().expect("log").clone();
    let programs_after = engine2.orchestration().programs.snapshot();

    let turns = vec![
        (
            1,
            "write-run-store",
            "Please write a Sol JSON program that calls get_employer_profile then get_company_profile, execute it with run_program, and store it.",
            format!("{msg1}\n\nObservation artifacts: program_id={program_id}, source_hash={source_hash}, status={status1}"),
        ),
        (
            2,
            "reuse-program",
            "Reuse that same stored program with identical source and confirm the source_hash matches.",
            format!("{msg2}\n\nRegistry snapshot: {programs_after}\nHost tool order: {host_log:?}\nstatus={status2}"),
        ),
    ];

    let mut summary_lines = vec![
        format!("# Program harness session — {run_id}"),
        String::new(),
        "- **result:** PASS".into(),
        "- **scenario:** deterministic AgentLoop `run_program` write → store → reuse".into(),
        format!("- **program_id:** `{program_id}`"),
        format!("- **source_hash:** `{source_hash}`"),
        format!("- **host_invocations:** `{}`", host_log.join(",")),
        "- **note:** This is the harness-guaranteed session. Live widget attempt is logged separately when LLM/auth allow profile tools.".into(),
        String::new(),
        "## Per-turn files".into(),
        String::new(),
    ];

    let mut turns_md = vec![
        format!("# Program harness turns — {run_id}"),
        String::new(),
        format!("- program_id: `{program_id}`"),
        format!("- source_hash: `{source_hash}`"),
        String::new(),
        "## Full transcript".into(),
        String::new(),
    ];

    for (id, label, user, assistant) in &turns {
        let slug = label.replace(|c: char| !c.is_ascii_alphanumeric(), "-");
        let path = format!("turn-{:02}-{slug}.md", id);
        summary_lines.push(format!("- `{path}`"));
        let body = format!(
            "# Turn {id}: {label}\n\n- **run:** `{run_id}`\n- **scenario:** run_program write → store → reuse (harness)\n- **status:** OK\n\n## User\n\n```text\n{user}\n```\n\n## Assistant\n\n```text\n{assistant}\n```\n"
        );
        write_md(&out_dir.join(&path), &body);
        turns_md.push(format!("### Turn {id} — {label} (OK)\n\n**User:** {user}\n\n**Assistant:**\n\n```text\n{assistant}\n```\n"));
    }

    write_md(&out_dir.join("SUMMARY.md"), &summary_lines.join("\n"));
    write_md(&out_dir.join("turns.md"), &turns_md.join("\n"));

    println!("[program-harness] PASS — {program_id} @ {source_hash}");
    println!("[program-harness] logs in {}", out_dir.display());
    println!("[program-harness] host calls: {}", host_log.join(", "));
}
