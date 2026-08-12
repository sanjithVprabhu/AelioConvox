//! Starlark-surface dialect for `run_program` — A-12 spike (F-033/F-034/F-043).
//!
//! Pipeline is strictly: **parse → analyze → authorize → execute**.
//! Pure compute programs authorize with an empty effect set (zero host dispatch).
//! The Meta `starlark` crate is currently blocked (hashbrown/allocative conflict); this
//! in-tree Starlark-surface evaluator is the durable spike until that dependency lands.

use crate::{ToolError, ToolErrorClass};
use blake3;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_SOURCE_BYTES: usize = 16 * 1024;
const MAX_EVAL_STEPS: u32 = 10_000;
const MAX_CALL_DEPTH: u32 = 32;
const FORBIDDEN_CALLS: &[&str] = &["load", "print", "eval", "getattr", "hasattr", "dir", "type"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Expr {
    Lit { value: i64 },
    Var { name: String },
    Bin { op: BinOp, left: Box<Expr>, right: Box<Expr> },
    Call { name: String, args: Vec<Expr> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FnDef {
    pub name: String,
    pub params: Vec<String>,
    pub body: Expr,
}

/// Authoritative compiled form. Hashing uses this AST, not raw text (F-034).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComputeAst {
    pub functions: Vec<FnDef>,
    pub entry: Expr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComputeProgramV0 {
    pub version: u32,
    pub kind: String,
    pub lang: String,
    pub code: String,
    pub ast: ComputeAst,
    pub ast_hash: String,
    #[serde(default)]
    pub rationale: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EffectSet {
    pub tool_paths: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputeRunResult {
    pub output: i64,
    pub expect_ok: Option<bool>,
    pub steps: u32,
    pub effects: EffectSet,
}

pub fn is_compute_source(source: &str) -> bool {
    let trimmed = source.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        if value.get("kind").and_then(Value::as_str) == Some("compute") {
            return true;
        }
        if matches!(
            value.get("lang").and_then(Value::as_str),
            Some("expr_v0" | "starlark")
        ) {
            return true;
        }
        if value.get("steps").is_some() {
            return false;
        }
        if value.get("code").and_then(Value::as_str).is_some() {
            return true;
        }
    }
    let head = trimmed.lines().next().unwrap_or("").trim_start();
    head.starts_with("def ") || head.starts_with("fn ") || looks_like_expr_line(head)
}

fn looks_like_expr_line(line: &str) -> bool {
    !line.starts_with('{')
        && (line.contains('+')
            || line.contains("add(")
            || line.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '('))
}

/// Parse + compile to AST (authority). Language tag is `starlark`; `expr_v0` is accepted as alias.
pub fn compile_compute(source: &str, rationale: &str) -> Result<ComputeProgramV0, ToolError> {
    let trimmed = source.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_SOURCE_BYTES {
        return Err(invalid("starlark source must be 1..=16384 characters"));
    }

    let (code, expect, rationale) = if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        let code = value
            .get("code")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| invalid("compute JSON requires a string `code` field"))?;
        let expect = value
            .get("expect")
            .and_then(Value::as_i64)
            .or_else(|| value.get("expect").and_then(Value::as_u64).map(|n| n as i64));
        let rationale = value
            .get("rationale")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(rationale)
            .to_string();
        (code, expect, rationale)
    } else {
        (trimmed.to_string(), None, rationale.to_string())
    };

    let ast = parse_starlark_surface(&code)?;
    let ast_hash = ast_hash(&ast)?;
    Ok(ComputeProgramV0 {
        version: 1,
        kind: "compute".into(),
        lang: "starlark".into(),
        code,
        ast,
        ast_hash,
        rationale,
        expect,
    })
}

/// Static effect analysis (§10.3). Rejects forbidden / unresolved host call sites.
pub fn analyze_effects(ast: &ComputeAst) -> Result<EffectSet, ToolError> {
    let mut effects = EffectSet::default();
    let defined: BTreeSet<&str> = ast.functions.iter().map(|f| f.name.as_str()).collect();
    walk_expr_effects(&ast.entry, &defined, &mut effects)?;
    for func in &ast.functions {
        walk_expr_effects(&func.body, &defined, &mut effects)?;
    }
    Ok(effects)
}

fn walk_expr_effects(
    expr: &Expr,
    defined: &BTreeSet<&str>,
    effects: &mut EffectSet,
) -> Result<(), ToolError> {
    match expr {
        Expr::Lit { .. } | Expr::Var { .. } => Ok(()),
        Expr::Bin { left, right, .. } => {
            walk_expr_effects(left, defined, effects)?;
            walk_expr_effects(right, defined, effects)
        }
        Expr::Call { name, args } => {
            if FORBIDDEN_CALLS.contains(&name.as_str()) {
                return Err(invalid(&format!(
                    "starlark analyze rejected forbidden call `{name}`"
                )));
            }
            if name.contains('.') {
                effects.tool_paths.insert(name.clone());
            } else if !defined.contains(name.as_str()) {
                return Err(invalid(&format!(
                    "starlark analyze rejected unresolved call `{name}`"
                )));
            }
            for arg in args {
                walk_expr_effects(arg, defined, effects)?;
            }
            Ok(())
        }
    }
}

/// Authorize effect set. Pure compute must have empty effects (zero host dispatch).
pub fn authorize_effects(effects: &EffectSet) -> Result<(), ToolError> {
    if effects.tool_paths.is_empty() {
        return Ok(());
    }
    Err(ToolError {
        class: ToolErrorClass::NotAuthorized,
        retryable: false,
        detail: format!(
            "starlark authorize denied host effects (A-12 pure spike): {}",
            effects
                .tool_paths
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })
}

/// Full A-12 pipeline: parse → analyze → authorize → execute.
pub fn run_starlark_pipeline(
    source: &str,
    rationale: &str,
) -> Result<(ComputeProgramV0, ComputeRunResult), ToolError> {
    let program = compile_compute(source, rationale)?;
    let effects = analyze_effects(&program.ast)?;
    authorize_effects(&effects)?;
    let run = eval_compute_inner(&program, effects)?;
    Ok((program, run))
}

pub fn eval_compute(program: &ComputeProgramV0) -> Result<ComputeRunResult, ToolError> {
    let effects = analyze_effects(&program.ast)?;
    authorize_effects(&effects)?;
    eval_compute_inner(program, effects)
}

fn eval_compute_inner(
    program: &ComputeProgramV0,
    effects: EffectSet,
) -> Result<ComputeRunResult, ToolError> {
    let mut env = BTreeMap::new();
    let mut fns: BTreeMap<&str, &FnDef> = BTreeMap::new();
    for func in &program.ast.functions {
        if fns.insert(func.name.as_str(), func).is_some() {
            return Err(invalid(&format!("duplicate function `{}`", func.name)));
        }
    }
    let mut steps = 0u32;
    let output = eval_expr(&program.ast.entry, &mut env, &fns, 0, &mut steps)?;
    let expect_ok = program.expect.map(|expected| expected == output);
    if let Some(false) = expect_ok {
        return Err(ToolError {
            class: ToolErrorClass::Handler,
            retryable: false,
            detail: format!(
                "starlark output {output} did not match expect {}",
                program.expect.unwrap()
            ),
        });
    }
    Ok(ComputeRunResult {
        output,
        expect_ok,
        steps,
        effects,
    })
}

pub fn compute_observation(
    program: &ComputeProgramV0,
    source_hash: &str,
    program_id: &str,
    run: &ComputeRunResult,
) -> Value {
    json!({
        "kind": "compute",
        "lang": program.lang,
        "pipeline": ["parse", "analyze", "authorize", "execute"],
        "parsed": true,
        "analyzed": true,
        "authorized": true,
        "compiled": true,
        "ast_hash": program.ast_hash,
        "source_hash": source_hash,
        "program_id": program_id,
        "output": run.output,
        "expect": program.expect,
        "expect_ok": run.expect_ok,
        "eval_steps": run.steps,
        "effects": run.effects.tool_paths.iter().cloned().collect::<Vec<_>>(),
        "host_dispatches": 0,
        "stored": true,
        "code": program.code,
        "reusable": true,
    })
}

pub fn ast_hash(ast: &ComputeAst) -> Result<String, ToolError> {
    let bytes = serde_json::to_vec(ast).map_err(|error| invalid(&format!("ast encode: {error}")))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn parse_starlark_surface(code: &str) -> Result<ComputeAst, ToolError> {
    let mut functions = Vec::new();
    let mut entry: Option<Expr> = None;
    let mut lines = Vec::new();
    for raw in code.lines() {
        let line = strip_comment(raw).trim().to_string();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("load(") {
            return Err(invalid("starlark `load` is forbidden in the sandbox"));
        }
        lines.push(line);
    }
    if lines.is_empty() {
        return Err(invalid("starlark program is empty"));
    }

    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        if let Some(rest) = line.strip_prefix("def ").or_else(|| line.strip_prefix("fn ")) {
            let (name, params, body_opt) = parse_def_header(rest)?;
            let body = if let Some(body) = body_opt {
                body
            } else {
                i += 1;
                if i >= lines.len() {
                    return Err(invalid(&format!("function `{name}` missing body")));
                }
                let body_line = lines[i].trim();
                let expr_src = body_line
                    .strip_prefix("return ")
                    .ok_or_else(|| {
                        invalid(&format!("function `{name}` body must be `return <expr>`"))
                    })?
                    .trim();
                parse_expr(expr_src)?
            };
            functions.push(FnDef {
                name,
                params,
                body,
            });
        } else if let Some(expr_src) = line.strip_prefix("return ") {
            entry = Some(parse_expr(expr_src.trim())?);
        } else {
            entry = Some(parse_expr(line)?);
        }
        i += 1;
    }

    let entry = entry.ok_or_else(|| invalid("starlark program needs an entry expression"))?;
    Ok(ComputeAst { functions, entry })
}

fn parse_def_header(rest: &str) -> Result<(String, Vec<String>, Option<Expr>), ToolError> {
    let rest = rest.trim();
    let (sig, after) =
        split_once_char(rest, ':').ok_or_else(|| invalid("function header must contain `:`"))?;
    let sig = sig.trim();
    let (name, params_src) = split_call_name(sig)?;
    let params = parse_param_list(params_src)?;
    let after = after.trim();
    if after.is_empty() {
        return Ok((name, params, None));
    }
    let expr_src = after.strip_prefix("return ").unwrap_or(after).trim();
    Ok((name, params, Some(parse_expr(expr_src)?)))
}

fn parse_param_list(src: &str) -> Result<Vec<String>, ToolError> {
    let inner = src
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| invalid("function parameters must be `(a, b)`"))?
        .trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    let mut params = Vec::new();
    for part in inner.split(',') {
        let name = part.trim();
        if !is_ident(name) {
            return Err(invalid(&format!("invalid parameter `{name}`")));
        }
        params.push(name.to_string());
    }
    Ok(params)
}

fn parse_expr(input: &str) -> Result<Expr, ToolError> {
    let mut p = Parser {
        src: input.trim(),
        pos: 0,
    };
    let expr = p.parse_add()?;
    p.skip_ws();
    if p.pos != p.src.len() {
        return Err(invalid(&format!(
            "unexpected trailing input in expression: {}",
            &p.src[p.pos..]
        )));
    }
    Ok(expr)
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while self
            .src
            .as_bytes()
            .get(self.pos)
            .is_some_and(|b| b.is_ascii_whitespace())
        {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.skip_ws();
        self.src[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        self.skip_ws();
        let ch = self.src[self.pos..].chars().next()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }

    fn parse_add(&mut self) -> Result<Expr, ToolError> {
        let mut left = self.parse_mul()?;
        while let Some(op) = self.peek() {
            if op == '+' {
                self.bump();
                let right = self.parse_mul()?;
                left = Expr::Bin {
                    op: BinOp::Add,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else if op == '-' {
                self.bump();
                let right = self.parse_mul()?;
                left = Expr::Bin {
                    op: BinOp::Sub,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> Result<Expr, ToolError> {
        let mut left = self.parse_primary()?;
        while let Some(op) = self.peek() {
            if op == '*' {
                self.bump();
                let right = self.parse_primary()?;
                left = Expr::Bin {
                    op: BinOp::Mul,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else if op == '/' {
                self.bump();
                if self.peek() == Some('/') {
                    self.bump();
                }
                let right = self.parse_primary()?;
                left = Expr::Bin {
                    op: BinOp::Div,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_primary(&mut self) -> Result<Expr, ToolError> {
        match self.peek() {
            Some('(') => {
                self.bump();
                let expr = self.parse_add()?;
                if self.bump() != Some(')') {
                    return Err(invalid("expected `)`"));
                }
                Ok(expr)
            }
            Some(c) if c == '-' || c.is_ascii_digit() => self.parse_number(),
            Some(c) if is_ident_start(c) => self.parse_ident_or_call(),
            other => Err(invalid(&format!("expected expression, found {other:?}"))),
        }
    }

    fn parse_number(&mut self) -> Result<Expr, ToolError> {
        self.skip_ws();
        let start = self.pos;
        if self.src.as_bytes().get(self.pos) == Some(&b'-') {
            self.pos += 1;
        }
        while self
            .src
            .as_bytes()
            .get(self.pos)
            .is_some_and(|b| b.is_ascii_digit())
        {
            self.pos += 1;
        }
        let text = &self.src[start..self.pos];
        let value: i64 = text
            .parse()
            .map_err(|_| invalid(&format!("invalid integer `{text}`")))?;
        Ok(Expr::Lit { value })
    }

    fn parse_ident_or_call(&mut self) -> Result<Expr, ToolError> {
        self.skip_ws();
        let start = self.pos;
        while self
            .src
            .as_bytes()
            .get(self.pos)
            .copied()
            .is_some_and(|b| is_ident_byte(b))
        {
            self.pos += 1;
        }
        let name = self.src[start..self.pos].to_string();
        if !is_ident(&name) {
            return Err(invalid(&format!("invalid identifier `{name}`")));
        }
        if self.peek() == Some('(') {
            self.bump();
            let mut args = Vec::new();
            if self.peek() != Some(')') {
                loop {
                    args.push(self.parse_add()?);
                    match self.peek() {
                        Some(',') => {
                            self.bump();
                        }
                        Some(')') => break,
                        other => {
                            return Err(invalid(&format!(
                                "expected `,` or `)` in call, found {other:?}"
                            )))
                        }
                    }
                }
            }
            if self.bump() != Some(')') {
                return Err(invalid("expected `)` after call arguments"));
            }
            Ok(Expr::Call { name, args })
        } else {
            Ok(Expr::Var { name })
        }
    }
}

fn eval_expr(
    expr: &Expr,
    env: &mut BTreeMap<String, i64>,
    fns: &BTreeMap<&str, &FnDef>,
    depth: u32,
    steps: &mut u32,
) -> Result<i64, ToolError> {
    *steps = steps.saturating_add(1);
    if *steps > MAX_EVAL_STEPS {
        return Err(invalid("starlark evaluation exceeded step budget"));
    }
    if depth > MAX_CALL_DEPTH {
        return Err(invalid("starlark evaluation exceeded call depth"));
    }
    match expr {
        Expr::Lit { value } => Ok(*value),
        Expr::Var { name } => env
            .get(name)
            .copied()
            .ok_or_else(|| invalid(&format!("undefined variable `{name}`"))),
        Expr::Bin { op, left, right } => {
            let l = eval_expr(left, env, fns, depth, steps)?;
            let r = eval_expr(right, env, fns, depth, steps)?;
            match op {
                BinOp::Add => Ok(l.saturating_add(r)),
                BinOp::Sub => Ok(l.saturating_sub(r)),
                BinOp::Mul => Ok(l.saturating_mul(r)),
                BinOp::Div => {
                    if r == 0 {
                        return Err(invalid("division by zero"));
                    }
                    Ok(l / r)
                }
            }
        }
        Expr::Call { name, args } => {
            let func = fns
                .get(name.as_str())
                .ok_or_else(|| invalid(&format!("undefined function `{name}`")))?;
            if func.params.len() != args.len() {
                return Err(invalid(&format!(
                    "function `{name}` expects {} args, got {}",
                    func.params.len(),
                    args.len()
                )));
            }
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                values.push(eval_expr(arg, env, fns, depth, steps)?);
            }
            let mut child = BTreeMap::new();
            for (param, value) in func.params.iter().zip(values) {
                child.insert(param.clone(), value);
            }
            eval_expr(&func.body, &mut child, fns, depth + 1, steps)
        }
    }
}

fn strip_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or(line)
}

fn split_once_char(s: &str, ch: char) -> Option<(&str, &str)> {
    s.find(ch).map(|i| (&s[..i], &s[i + ch.len_utf8()..]))
}

fn split_call_name(sig: &str) -> Result<(String, &str), ToolError> {
    let open = sig
        .find('(')
        .ok_or_else(|| invalid("function signature must include `(params)`"))?;
    let name = sig[..open].trim();
    if !is_ident(name) {
        return Err(invalid(&format!("invalid function name `{name}`")));
    }
    Ok((name.to_string(), &sig[open..]))
}

fn is_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if is_ident_start(c) => chars.all(|c| c.is_ascii_alphanumeric() || c == '_'),
        _ => false,
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn invalid(detail: &str) -> ToolError {
    ToolError {
        class: ToolErrorClass::InvalidArguments,
        retryable: true,
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addition_pipeline_parses_analyzes_runs_and_hashes() {
        let code = "def add(a, b):\n  return a + b\n\nadd(40, 2)\n";
        let (program, run) = run_starlark_pipeline(code, "demo add").expect("pipeline");
        assert_eq!(program.lang, "starlark");
        assert_eq!(run.output, 42);
        assert!(run.effects.tool_paths.is_empty());
        let again = compile_compute(code, "demo add").expect("recompile");
        assert_eq!(program.ast_hash, again.ast_hash);
    }

    #[test]
    fn forbidden_print_is_rejected_at_analyze() {
        let err = run_starlark_pipeline("print(1)\n", "x").expect_err("print");
        assert!(err.detail.contains("forbidden") || err.detail.contains("unresolved"));
    }

    #[test]
    fn host_effect_is_denied_at_authorize() {
        let mut program =
            compile_compute("def add(a,b):\n  return a + b\n\nadd(1,2)\n", "x").unwrap();
        program.ast.entry = Expr::Call {
            name: "api.orders.list".into(),
            args: vec![],
        };
        let effects = analyze_effects(&program.ast).expect("analyze");
        assert!(effects.tool_paths.contains("api.orders.list"));
        let err = authorize_effects(&effects).expect_err("deny");
        assert!(err.detail.contains("denied"));
    }

    #[test]
    fn expect_mismatch_fails() {
        let source = r#"{"kind":"compute","lang":"starlark","code":"2 + 2","expect":5}"#;
        let err = run_starlark_pipeline(source, "bad").expect_err("expect");
        assert!(err.detail.contains("did not match expect"));
    }

    #[test]
    fn parse_error_on_trailing_junk() {
        let err = compile_compute("2 + 2 oops", "x").expect_err("junk");
        assert!(err.detail.contains("trailing"));
    }
}
