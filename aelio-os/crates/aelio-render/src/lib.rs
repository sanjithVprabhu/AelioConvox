//! # aelio-render — Render Protocol (AELIO_RENDER_PROTOCOL.md)
//!
//! Closed down/up seam between Aelio server and the chat SDK.
//! No HTML/JS/CSS crosses the wire. Kinds are versioned and closed.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};

pub const PROTOCOL_ID: &str = "aelio-render@v1";
pub const MAX_NESTING: u32 = 4;
pub const MAX_BLOCKS_PER_FRAME: usize = 64;
pub const CORE_KINDS: &[&str] = &[
    "text@1",
    "code@1",
    "table@1",
    "chart@1",
    "diagram@1",
    "metric@1",
    "form@1",
    "choice@1",
    "confirm@1",
    "status@1",
    "media@1",
    "group@1",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    Invalid(String),
    Rejected(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(s) => write!(f, "render invalid: {s}"),
            Self::Rejected(s) => write!(f, "render rejected: {s}"),
        }
    }
}

impl std::error::Error for RenderError {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub protocol: String,
    pub kinds: Vec<String>,
    #[serde(default)]
    pub limits: ClientLimits,
    #[serde(default)]
    pub resume: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct ClientLimits {
    #[serde(default = "default_max_blocks")]
    pub max_blocks: usize,
    #[serde(default = "default_max_nesting")]
    pub max_nesting: u32,
}

fn default_max_blocks() -> usize {
    MAX_BLOCKS_PER_FRAME
}
fn default_max_nesting() -> u32 {
    MAX_NESTING
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Welcome {
    pub session_id: String,
    pub accepted_kinds: Vec<String>,
    pub server_limits: ClientLimits,
    pub protocol: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RenderMode {
    Append,
    Patch,
    ReplaceTurn,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BlockMeta {
    #[serde(default)]
    pub width: Option<String>,
    #[serde(default)]
    pub collapsed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Block {
    pub block_id: String,
    pub kind: String,
    pub body: Json,
    /// Mandatory unless kind is text@1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<Box<Block>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub on: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<BlockMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RenderFrame {
    #[serde(rename = "frame")]
    pub frame_type: String,
    pub frame_id: String,
    pub mode: RenderMode,
    pub turn_id: String,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EventPayload {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
    #[serde(default)]
    pub value: Json,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EventFrame {
    #[serde(rename = "frame")]
    pub frame_type: String,
    pub frame_id: String,
    pub turn_id: String,
    pub event: EventPayload,
}

/// Session capability set after Welcome.
#[derive(Debug, Clone)]
pub struct CapabilitySet {
    pub accepted_kinds: BTreeSet<String>,
    pub limits: ClientLimits,
}

pub fn core_kind_set() -> BTreeSet<String> {
    CORE_KINDS.iter().map(|s| (*s).to_string()).collect()
}

pub fn handshake(
    hello: &Hello,
    session_id: impl Into<String>,
) -> Result<(Welcome, CapabilitySet), RenderError> {
    if hello.protocol != PROTOCOL_ID {
        return Err(RenderError::Rejected(
            "handshake_version_unsupported".into(),
        ));
    }
    if hello.kinds.is_empty() {
        return Err(RenderError::Rejected("kind_set_empty".into()));
    }
    let offered: BTreeSet<_> = hello.kinds.iter().cloned().collect();
    let core = core_kind_set();
    let accepted: Vec<String> = offered.intersection(&core).cloned().collect();
    if accepted.is_empty() {
        return Err(RenderError::Rejected("kind_set_empty".into()));
    }
    if !accepted.iter().any(|k| k == "text@1") {
        return Err(RenderError::Rejected(
            "accepted kinds must include text@1".into(),
        ));
    }
    let limits = ClientLimits {
        max_blocks: hello.limits.max_blocks.clamp(1, MAX_BLOCKS_PER_FRAME),
        max_nesting: hello.limits.max_nesting.clamp(1, MAX_NESTING),
    };
    let welcome = Welcome {
        session_id: session_id.into(),
        accepted_kinds: accepted.clone(),
        server_limits: limits.clone(),
        protocol: PROTOCOL_ID.into(),
    };
    Ok((
        welcome,
        CapabilitySet {
            accepted_kinds: accepted.into_iter().collect(),
            limits,
        },
    ))
}

/// Wrap plain assistant text as a valid single-block RenderFrame.
pub fn text_frame(turn_id: &str, frame_id: &str, md: &str) -> RenderFrame {
    RenderFrame {
        frame_type: "render".into(),
        frame_id: frame_id.into(),
        mode: RenderMode::Append,
        turn_id: turn_id.into(),
        blocks: vec![text_block("b0", md)],
    }
}

pub fn text_block(block_id: &str, md: &str) -> Block {
    Block {
        block_id: block_id.into(),
        kind: "text@1".into(),
        body: serde_json::json!({ "md": md }),
        fallback: None,
        on: BTreeMap::new(),
        meta: None,
    }
}

pub fn status_block(block_id: &str, state: &str, label: &str) -> Block {
    Block {
        block_id: block_id.into(),
        kind: "status@1".into(),
        body: serde_json::json!({ "state": state, "label": label }),
        fallback: Some(Box::new(text_block(&format!("{block_id}_fb"), label))),
        on: BTreeMap::new(),
        meta: None,
    }
}

pub fn choice_block(
    block_id: &str,
    options: &[(&str, &str)],
    multi: bool,
    fallback_md: &str,
) -> Block {
    let opts: Vec<Json> = options
        .iter()
        .map(|(id, label)| serde_json::json!({ "id": id, "label": label }))
        .collect();
    Block {
        block_id: block_id.into(),
        kind: "choice@1".into(),
        body: serde_json::json!({ "options": opts, "multi": multi }),
        fallback: Some(Box::new(text_block(&format!("{block_id}_fb"), fallback_md))),
        on: BTreeMap::new(),
        meta: None,
    }
}

pub fn confirm_block(
    block_id: &str,
    prompt: &str,
    yes: &str,
    no: &str,
    danger: bool,
    binding: Option<&str>,
) -> Block {
    let mut on = BTreeMap::new();
    if let Some(target) = binding {
        on.insert("confirmed".into(), target.into());
    }
    Block {
        block_id: block_id.into(),
        kind: "confirm@1".into(),
        body: serde_json::json!({
            "prompt": prompt,
            "yes_label": yes,
            "no_label": no,
            "danger": danger
        }),
        fallback: Some(Box::new(text_block(
            &format!("{block_id}_fb"),
            &format!("{prompt} (yes/no)"),
        ))),
        on,
        meta: None,
    }
}

pub fn validate_render_frame(
    frame: &RenderFrame,
    caps: Option<&CapabilitySet>,
) -> Result<(), RenderError> {
    if frame.frame_type != "render" {
        return Err(RenderError::Invalid("frame_malformed".into()));
    }
    if frame.frame_id.trim().is_empty() || frame.turn_id.trim().is_empty() {
        return Err(RenderError::Invalid("frame_malformed".into()));
    }
    if frame.blocks.is_empty() {
        return Err(RenderError::Invalid("frame_malformed: empty blocks".into()));
    }
    let max_blocks = caps
        .map(|c| c.limits.max_blocks)
        .unwrap_or(MAX_BLOCKS_PER_FRAME);
    if frame.blocks.len() > max_blocks {
        return Err(RenderError::Invalid(format!(
            "frame exceeds max_blocks {max_blocks}"
        )));
    }
    let mut ids = BTreeSet::new();
    let max_nest = caps.map(|c| c.limits.max_nesting).unwrap_or(MAX_NESTING);
    for block in &frame.blocks {
        validate_block(block, caps, 0, max_nest, &mut ids)?;
    }
    Ok(())
}

fn validate_block(
    block: &Block,
    caps: Option<&CapabilitySet>,
    depth: u32,
    max_nest: u32,
    ids: &mut BTreeSet<String>,
) -> Result<(), RenderError> {
    if depth > max_nest {
        return Err(RenderError::Invalid("nesting depth exceeded".into()));
    }
    if block.block_id.trim().is_empty() {
        return Err(RenderError::Invalid("block_id required".into()));
    }
    if !ids.insert(block.block_id.clone()) {
        return Err(RenderError::Invalid(format!(
            "duplicate block_id `{}`",
            block.block_id
        )));
    }
    if !is_pinned_kind(&block.kind) {
        return Err(RenderError::Invalid(format!(
            "kind must be pinned id@version, got `{}`",
            block.kind
        )));
    }
    if let Some(caps) = caps {
        if !caps.accepted_kinds.contains(&block.kind) {
            return Err(RenderError::Invalid(format!(
                "kind `{}` not in accepted_kinds",
                block.kind
            )));
        }
    } else if !CORE_KINDS.contains(&block.kind.as_str()) {
        return Err(RenderError::Invalid(format!(
            "unknown core kind `{}`",
            block.kind
        )));
    }
    validate_body(&block.kind, &block.body)?;
    if block.kind != "text@1" {
        let Some(fb) = &block.fallback else {
            return Err(RenderError::Invalid("fallback_missing".into()));
        };
        if fb.kind != "text@1" {
            return Err(RenderError::Invalid("fallback must be kind text@1".into()));
        }
        validate_body("text@1", &fb.body)?;
    }
    for (event, target) in &block.on {
        if event.trim().is_empty() || !is_pinned_kind(target) {
            return Err(RenderError::Invalid("binding_unpinned".into()));
        }
        // write/external consent: confirm@1 only (R-13) — heuristic by event name.
        if event == "confirmed" && block.kind != "confirm@1" {
            return Err(RenderError::Invalid("consent_required".into()));
        }
    }
    if block.kind == "group@1" {
        let blocks = block
            .body
            .get("blocks")
            .and_then(Json::as_array)
            .ok_or_else(|| RenderError::Invalid("group@1 body.blocks required".into()))?;
        for child in blocks {
            let child: Block = serde_json::from_value(child.clone())
                .map_err(|e| RenderError::Invalid(format!("group child: {e}")))?;
            validate_block(&child, caps, depth + 1, max_nest, ids)?;
        }
    }
    Ok(())
}

fn is_pinned_kind(s: &str) -> bool {
    s.split_once('@')
        .is_some_and(|(n, v)| !n.is_empty() && !v.is_empty())
}

fn validate_body(kind: &str, body: &Json) -> Result<(), RenderError> {
    let o = body
        .as_object()
        .ok_or_else(|| RenderError::Invalid("block_body_invalid".into()))?;
    match kind {
        "text@1" => {
            req_str(o, "md")?;
            Ok(())
        }
        "code@1" => {
            req_str(o, "lang")?;
            req_str(o, "source")?;
            Ok(())
        }
        "status@1" => {
            let state = req_str(o, "state")?;
            if !matches!(state, "running" | "done" | "error") {
                return Err(RenderError::Invalid("status state invalid".into()));
            }
            req_str(o, "label")?;
            Ok(())
        }
        "choice@1" => {
            let opts = o
                .get("options")
                .and_then(Json::as_array)
                .ok_or_else(|| RenderError::Invalid("choice options required".into()))?;
            if opts.is_empty() {
                return Err(RenderError::Invalid("choice options empty".into()));
            }
            Ok(())
        }
        "confirm@1" => {
            req_str(o, "prompt")?;
            req_str(o, "yes_label")?;
            req_str(o, "no_label")?;
            Ok(())
        }
        "metric@1" => {
            if !o.get("items").map(Json::is_array).unwrap_or(false) {
                return Err(RenderError::Invalid("metric items required".into()));
            }
            Ok(())
        }
        "table@1" => {
            if !o.get("columns").map(Json::is_array).unwrap_or(false)
                || !o.get("rows").map(Json::is_array).unwrap_or(false)
            {
                return Err(RenderError::Invalid("table columns/rows required".into()));
            }
            Ok(())
        }
        "chart@1" => {
            req_str(o, "type")?;
            if !o.get("series").map(Json::is_array).unwrap_or(false) {
                return Err(RenderError::Invalid("chart series required".into()));
            }
            Ok(())
        }
        "diagram@1" => {
            if !o.get("nodes").map(Json::is_array).unwrap_or(false)
                || !o.get("edges").map(Json::is_array).unwrap_or(false)
            {
                return Err(RenderError::Invalid("diagram nodes/edges required".into()));
            }
            Ok(())
        }
        "form@1" => {
            if !o.get("fields").map(Json::is_array).unwrap_or(false) {
                return Err(RenderError::Invalid("form fields required".into()));
            }
            Ok(())
        }
        "media@1" => {
            req_str(o, "handle")?;
            req_str(o, "alt")?;
            Ok(())
        }
        "group@1" => {
            req_str(o, "direction")?;
            Ok(())
        }
        other => Err(RenderError::Invalid(format!("unknown kind `{other}`"))),
    }
}

fn req_str<'a>(o: &'a serde_json::Map<String, Json>, key: &str) -> Result<&'a str, RenderError> {
    o.get(key)
        .and_then(Json::as_str)
        .filter(|s| !s.is_empty() || key == "md" || key == "source")
        .ok_or_else(|| RenderError::Invalid(format!("block_body_invalid: missing `{key}`")))
}

pub fn validate_event_frame(
    frame: &EventFrame,
    live_blocks: &BTreeMap<String, Block>,
) -> Result<(), RenderError> {
    if frame.frame_type != "event" {
        return Err(RenderError::Invalid("frame_malformed".into()));
    }
    if frame.frame_id.trim().is_empty() || frame.turn_id.trim().is_empty() {
        return Err(RenderError::Invalid("frame_malformed".into()));
    }
    let t = frame.event.event_type.as_str();
    const TYPES: &[&str] = &[
        "message",
        "copied",
        "row_selected",
        "point_selected",
        "node_selected",
        "submitted",
        "chosen",
        "confirmed",
        "render_degraded",
        "patch_orphan",
    ];
    if !TYPES.contains(&t) {
        return Err(RenderError::Rejected("event_undeclared_type".into()));
    }
    if t == "message" {
        if frame.event.block_id.is_some() {
            return Err(RenderError::Invalid(
                "message events must not carry block_id".into(),
            ));
        }
        let text = frame
            .event
            .value
            .get("text")
            .and_then(Json::as_str)
            .unwrap_or("");
        if text.trim().is_empty() {
            return Err(RenderError::Invalid("message text required".into()));
        }
        return Ok(());
    }
    let Some(block_id) = &frame.event.block_id else {
        return Err(RenderError::Rejected("event_orphan_block".into()));
    };
    let Some(block) = live_blocks.get(block_id) else {
        return Err(RenderError::Rejected("event_orphan_block".into()));
    };
    if !event_allowed_for_kind(&block.kind, t) {
        return Err(RenderError::Rejected("event_invalid".into()));
    }
    Ok(())
}

fn event_allowed_for_kind(kind: &str, event: &str) -> bool {
    match event {
        "render_degraded" | "patch_orphan" => true,
        "copied" => kind == "code@1",
        "row_selected" => kind == "table@1",
        "point_selected" => kind == "chart@1",
        "node_selected" => kind == "diagram@1",
        "submitted" => kind == "form@1",
        "chosen" => kind == "choice@1",
        "confirmed" => kind == "confirm@1",
        _ => false,
    }
}

/// Flatten a frame to plain text for non-web channels (WhatsApp etc.).
pub fn flatten_to_text(frame: &RenderFrame) -> String {
    let mut parts = Vec::new();
    for b in &frame.blocks {
        parts.push(block_to_text(b));
    }
    parts.join("\n\n")
}

fn block_to_text(b: &Block) -> String {
    match b.kind.as_str() {
        "text@1" => b
            .body
            .get("md")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string(),
        "status@1" => b
            .body
            .get("label")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string(),
        "choice@1" => {
            let opts = b
                .body
                .get("options")
                .and_then(Json::as_array)
                .cloned()
                .unwrap_or_default();
            let labels: Vec<_> = opts
                .iter()
                .filter_map(|o| o.get("label").and_then(Json::as_str))
                .collect();
            format!("Choose: {}", labels.join(" | "))
        }
        "confirm@1" => b
            .body
            .get("prompt")
            .and_then(Json::as_str)
            .unwrap_or("Confirm?")
            .to_string(),
        _ => b
            .fallback
            .as_ref()
            .map(|f| block_to_text(f))
            .unwrap_or_else(|| format!("[{}]", b.kind)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_intersects_kinds() {
        let hello = Hello {
            protocol: PROTOCOL_ID.into(),
            kinds: vec!["text@1".into(), "choice@1".into(), "x.custom@1".into()],
            limits: ClientLimits::default(),
            resume: None,
        };
        let (w, caps) = handshake(&hello, "s1").unwrap();
        assert!(w.accepted_kinds.contains(&"text@1".into()));
        assert!(w.accepted_kinds.contains(&"choice@1".into()));
        assert!(!caps.accepted_kinds.contains("x.custom@1"));
    }

    #[test]
    fn text_frame_validates() {
        let f = text_frame("t1", "f1", "Hello **world**");
        validate_render_frame(&f, None).unwrap();
        assert_eq!(flatten_to_text(&f), "Hello **world**");
    }

    #[test]
    fn choice_requires_fallback() {
        let mut b = choice_block("c1", &[("a", "A")], false, "pick A");
        b.fallback = None;
        let f = RenderFrame {
            frame_type: "render".into(),
            frame_id: "f1".into(),
            mode: RenderMode::Append,
            turn_id: "t1".into(),
            blocks: vec![b],
        };
        assert!(validate_render_frame(&f, None).is_err());
    }

    #[test]
    fn event_message_and_chosen() {
        let f = text_frame("t1", "f1", "hi");
        let mut live = BTreeMap::new();
        let choice = choice_block("c1", &[("yes", "Yes")], false, "yes?");
        live.insert("c1".into(), choice.clone());

        let msg = EventFrame {
            frame_type: "event".into(),
            frame_id: "e1".into(),
            turn_id: "t1".into(),
            event: EventPayload {
                event_type: "message".into(),
                block_id: None,
                value: serde_json::json!({ "text": "hello" }),
            },
        };
        validate_event_frame(&msg, &live).unwrap();

        let chosen = EventFrame {
            frame_type: "event".into(),
            frame_id: "e2".into(),
            turn_id: "t1".into(),
            event: EventPayload {
                event_type: "chosen".into(),
                block_id: Some("c1".into()),
                value: serde_json::json!({ "ids": ["yes"] }),
            },
        };
        validate_event_frame(&chosen, &live).unwrap();
        let _ = f;
    }
}
