//! `run_program` — Sol tool recipes (v0) and compute `expr_v0` programs.

use crate::compute::{
    compute_observation, is_compute_source, run_starlark_pipeline, ComputeProgramV0,
};
use crate::{EffectClass, ToolCall, ToolDefinition, ToolError, ToolErrorClass};
use blake3;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramStep {
    pub tool: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolProgramV0 {
    pub version: u32,
    pub steps: Vec<ProgramStep>,
    #[serde(default)]
    pub rationale: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProgramBody {
    Sol(SolProgramV0),
    Compute(ComputeProgramV0),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredProgram {
    pub program_id: String,
    pub source_hash: String,
    pub program: ProgramBody,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProgramRegistry {
    programs: BTreeMap<String, StoredProgram>,
}

impl ProgramRegistry {
    pub fn store_body(&mut self, program: ProgramBody, source_hash: String) -> StoredProgram {
        let program_id = format!("prog-{}", &source_hash[..16.min(source_hash.len())]);
        let stored = StoredProgram {
            program_id: program_id.clone(),
            source_hash,
            program,
        };
        self.programs.insert(program_id, stored.clone());
        stored
    }

    pub fn store(&mut self, program: SolProgramV0, source_hash: String) -> StoredProgram {
        self.store_body(ProgramBody::Sol(program), source_hash)
    }

    pub fn store_compute(&mut self, program: ComputeProgramV0, source_hash: String) -> StoredProgram {
        self.store_body(ProgramBody::Compute(program), source_hash)
    }

    pub fn get(&self, program_id: &str) -> Option<&StoredProgram> {
        self.programs.get(program_id)
    }

    pub fn snapshot(&self) -> Value {
        json!({
            "programs": self.programs.values().cloned().collect::<Vec<_>>(),
        })
    }
}

pub fn parse_program(source: &str, rationale: &str) -> Result<SolProgramV0, ToolError> {
    let trimmed = source.trim();
    if trimmed.is_empty() || trimmed.len() > 64 * 1024 {
        return Err(invalid("source must be 1..=65536 characters"));
    }
    if is_compute_source(trimmed) {
        return Err(invalid(
            "source looks like a compute program; use kind=compute / expr_v0 code (not Sol steps)",
        ));
    }
    let mut value: Value =
        serde_json::from_str(trimmed).map_err(|error| invalid(&format!("invalid JSON: {error}")))?;
    if let Some(object) = value.as_object_mut() {
        if !object.contains_key("rationale") && !rationale.is_empty() {
            object.insert("rationale".into(), Value::String(rationale.to_string()));
        }
        if !object.contains_key("version") {
            object.insert("version".into(), json!(1));
        }
    }
    let program: SolProgramV0 =
        serde_json::from_value(value).map_err(|error| invalid(&format!("invalid program: {error}")))?;
    if program.version != 1 {
        return Err(invalid("only program version 1 is supported"));
    }
    if program.steps.is_empty() || program.steps.len() > 64 {
        return Err(invalid("program must contain 1..=64 steps"));
    }
    Ok(program)
}

pub fn validate_against_tools(
    program: &SolProgramV0,
    tools: &[ToolDefinition],
    kernel_names: &HashSet<&str>,
) -> Result<Vec<ToolCall>, ToolError> {
    let by_name: BTreeMap<&str, &ToolDefinition> = tools
        .iter()
        .map(|tool| (tool.name.as_str(), tool))
        .collect();
    let mut calls = Vec::with_capacity(program.steps.len());
    for (index, step) in program.steps.iter().enumerate() {
        if kernel_names.contains(step.tool.as_str()) {
            return Err(ToolError {
                class: ToolErrorClass::NotAuthorized,
                retryable: false,
                detail: format!(
                    "run_program step {index} cannot invoke kernel tool `{}`",
                    step.tool
                ),
            });
        }
        let tool = by_name.get(step.tool.as_str()).ok_or_else(|| ToolError {
            class: ToolErrorClass::UnknownTool,
            retryable: false,
            detail: format!("run_program step {index} references unknown tool `{}`", step.tool),
        })?;
        if matches!(
            tool.effect_class,
            EffectClass::Financial | EffectClass::AccessControl
        ) {
            return Err(ToolError {
                class: ToolErrorClass::NotAuthorized,
                retryable: false,
                detail: format!(
                    "run_program step {index} effect class {:?} is not executable via program sandbox",
                    tool.effect_class
                ),
            });
        }
        let arguments = if step.arguments.is_null() {
            json!({})
        } else {
            step.arguments.clone()
        };
        calls.push(ToolCall {
            id: format!(
                "program-step-{index}-{}",
                blake3::hash(format!("{}:{index}:{}", program.rationale, step.tool).as_bytes())
                    .to_hex()
                    .chars()
                    .take(12)
                    .collect::<String>()
            ),
            name: step.tool.clone(),
            arguments,
        });
    }
    Ok(calls)
}

pub fn source_hash(source: &str) -> String {
    blake3::hash(source.as_bytes()).to_hex().to_string()
}

/// Compile + run a Starlark-surface program and store it. Returns the Observation payload.
pub fn run_compute_program(source: &str, rationale: &str) -> Result<(Value, StoredProgram), ToolError> {
    let (program, run) = run_starlark_pipeline(source, rationale)?;
    let hash = program.ast_hash.clone();
    let mut registry = ProgramRegistry::default();
    let stored = registry.store_compute(program.clone(), hash.clone());
    let observation = compute_observation(&program, &hash, &stored.program_id, &run);
    Ok((observation, stored))
}

fn invalid(detail: &str) -> ToolError {
    ToolError {
        class: ToolErrorClass::InvalidArguments,
        retryable: true,
        detail: detail.to_string(),
    }
}
