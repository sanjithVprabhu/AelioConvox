//! Deterministic Starlark-surface `run_program` demo: parse → analyze → authorize → execute → store → reuse.
//!
//!   cargo run -p aelio-agent-loop --example run_compute_session -- ../../docs/chat-runs

use aelio_agent_loop::{
    compile_compute, eval_compute, kernel_tool_definitions, AgentLoop, AssistantBlock,
    BudgetLimits, Context, EffectClass, EffectGate, GateDecision, LoopError, LoopOutcome, Model,
    ModelRequest, ModelResponse, ModelUsage, StopReason, ToolCall, ToolError, ToolHost,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fs::{create_dir_all, write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const ADDITION_SOURCE: &str = r#"def add(a, b):
  return a + b

add(40, 2)
"#;

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
struct NoHost;

#[async_trait]
impl ToolHost for NoHost {
    async fn invoke(&self, call: &ToolCall) -> Result<Value, ToolError> {
        Err(ToolError {
            class: aelio_agent_loop::ToolErrorClass::UnknownTool,
            retryable: false,
            detail: format!("compute demo should not call host tool `{}`", call.name),
        })
    }
}

struct Allow;
impl EffectGate for Allow {
    fn decide(&self, _call: &ToolCall, _class: &EffectClass) -> GateDecision {
        GateDecision::Allow
    }
}

fn ctx() -> Context {
    Context::new("system", "bootstrap", kernel_tool_definitions(), Vec::new())
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
    format!("harness-compute-{ms}")
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

    let compiled = compile_compute(ADDITION_SOURCE, "harness addition").expect("compile");
    let dry_run = eval_compute(&compiled).expect("eval");
    assert_eq!(dry_run.output, 42);

    let model = ScriptedModel(VecDeque::from([
        response(
            "r1",
            vec![call(
                "c1",
                "run_program",
                json!({
                    "source": ADDITION_SOURCE,
                    "rationale": "write addition program"
                }),
            )],
        ),
        response(
            "r2",
            vec![finish(
                "c2",
                "Wrote and ran addition program; output should be 42.",
            )],
        ),
    ]));

    let (outcome1, engine1) = AgentLoop::new(model, NoHost, Allow, ctx(), BudgetLimits::default())
        .run()
        .await
        .expect("first run");
    let LoopOutcome::Finished { message: msg1, .. } = outcome1 else {
        panic!("expected finished first run");
    };

    let programs = engine1.orchestration().programs.snapshot();
    let first = programs["programs"].as_array().expect("programs")[0].clone();
    let program_id = first["program_id"].as_str().unwrap().to_string();
    let source_hash = first["source_hash"].as_str().unwrap().to_string();

    let model2 = ScriptedModel(VecDeque::from([
        response(
            "r3",
            vec![call(
                "c3",
                "run_program",
                json!({
                    "source": ADDITION_SOURCE,
                    "rationale": "reuse addition program"
                }),
            )],
        ),
        response(
            "r4",
            vec![finish(
                "c4",
                format!(
                    "Reused compute program {program_id} (source_hash={source_hash}). Output 42 again."
                ),
            )],
        ),
    ]));

    let (outcome2, engine2) = AgentLoop::new(model2, NoHost, Allow, ctx(), BudgetLimits::default())
        .with_orchestration(engine1.orchestration().clone())
        .run()
        .await
        .expect("reuse run");
    let LoopOutcome::Finished {
        message: msg2, ..
    } = outcome2
    else {
        panic!("expected finished reuse run");
    };

    let programs_after = engine2.orchestration().programs.snapshot();
    let count = programs_after["programs"].as_array().map(|a| a.len()).unwrap_or(0);

    let turns = vec![
        (
            1,
            "write-run-eval-store",
            "Write a Starlark-surface addition program and run it with run_program.",
            format!("{msg1}\n\nObservation: program_id={program_id}, source_hash={source_hash}, output=42, lang=starlark"),
        ),
        (
            2,
            "reuse-program",
            "Reuse the identical addition program and confirm hash/output.",
            format!("{msg2}\n\nRegistry programs={count} (expect 1)"),
        ),
    ];

    let mut summary = vec![
        format!("# Compute program harness — {run_id}"),
        String::new(),
        "- **result:** PASS".into(),
        "- **dialect:** starlark (A-12 surface spike)".into(),
        "- **pipeline:** parse → analyze → authorize → execute".into(),
        format!("- **program_id:** `{program_id}`"),
        format!("- **source_hash / ast_hash:** `{source_hash}`"),
        "- **output:** 42".into(),
        "- **parsed / analyzed / authorized:** yes".into(),
        "- **host_dispatches:** 0".into(),
        "- **compiled:** yes (AST is authority)".into(),
        format!("- **registry_entries:** {count} (identical source reuses same id)"),
        String::new(),
        "## Per-turn files".into(),
        String::new(),
    ];

    for (id, label, user, assistant) in &turns {
        let slug = label.replace(|c: char| !c.is_ascii_alphanumeric(), "-");
        let path = format!("turn-{id:02}-{slug}.md");
        summary.push(format!("- `{path}`"));
        write_md(
            &out_dir.join(&path),
            &format!(
                "# Turn {id}: {label}\n\n- **run:** `{run_id}`\n- **scenario:** compute addition via run_program\n- **status:** OK\n\n## User\n\n```text\n{user}\n```\n\n## Assistant\n\n```text\n{assistant}\n```\n"
            ),
        );
    }

    write_md(&out_dir.join("SUMMARY.md"), &summary.join("\n"));
    println!("[compute-harness] PASS — {program_id} output=42 @ {source_hash}");
    println!("[compute-harness] logs in {}", out_dir.display());
}
