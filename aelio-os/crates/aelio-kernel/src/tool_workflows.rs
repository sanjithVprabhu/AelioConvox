//! Phase 5 — tool/memory workflow runners over installed Sol harnesses.
//!
//! After admin promote (or vendor library install), these helpers load the
//! program from the contract table and execute it with the sandbox registry.

use crate::compile;
use crate::driver::{Instance, TurnOutcome};
use crate::error::{ErrV1, ReasonCode};
use crate::registry::{EffectClass, Registry};
use crate::sol_harness_lib::load_contract;
use crate::stdlib_targets::registry_with_p0_stdlib;
use aelio_sol::SolValue;
use aelio_store::Store;

fn tool_registry() -> Registry {
    let mut r = registry_with_p0_stdlib();
    r.register("tool.act_stub@1", EffectClass::External, |_| {
        Ok(SolValue::map([
            ("ok", SolValue::Bool(true)),
            ("tool", SolValue::str("act_stub")),
        ]))
    });
    r.register("tool.send_otp@1", EffectClass::External, |args| {
        let phone = args
            .as_map()
            .and_then(|m| m.get("phone"))
            .cloned()
            .unwrap_or_else(|| SolValue::str(""));
        Ok(SolValue::map([
            ("ok", SolValue::Bool(true)),
            ("otp_sent", SolValue::Bool(true)),
            ("phone", phone),
        ]))
    });
    r.register("memory.search@1", EffectClass::Read, |args| {
        let q = args
            .as_map()
            .and_then(|m| m.get("query"))
            .cloned()
            .unwrap_or_else(|| SolValue::str(""));
        Ok(SolValue::map([(
            "snippet",
            SolValue::str(format!("memory hit for {q:?}")),
        )]))
    });
    r.register("context.attach@1", EffectClass::Write, |args| {
        let snippet = args
            .as_map()
            .and_then(|m| m.get("snippet"))
            .cloned()
            .unwrap_or_else(|| SolValue::str(""));
        Ok(SolValue::map([("note", snippet)]))
    });
    r.register("understand.classify@1", EffectClass::Read, |args| {
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
    });
    r
}

/// Run a promoted/installed harness by id with an input bag.
pub fn run_installed_harness(
    store: &dyn Store,
    tenant: &str,
    harness_id: &str,
    bag: SolValue,
) -> Result<(SolValue, String), ErrV1> {
    let contract = load_contract(store, tenant, harness_id)
        .map_err(|e| ErrV1::new(ReasonCode::Internal, harness_id, format!("{e:?}")))?
        .ok_or_else(|| {
            ErrV1::new(
                ReasonCode::Missing,
                harness_id,
                "harness not installed (promote or install_from_manifest first)",
            )
        })?;
    let program = compile(&contract.program_json)?;
    let mut registry = tool_registry();
    let mut inst = Instance::new(program, &mut registry);
    match inst.start(bag)? {
        TurnOutcome::Completed { bag, bag_hash } => Ok((bag, bag_hash)),
        TurnOutcome::Parked(p) => {
            // Return partial bag with park marker for wait_for_user style programs.
            Ok((
                p.bag,
                format!("parked:{}", p.park_nid),
            ))
        }
    }
}

/// Confirm-then-act: bag must include `confirmed: true` to effect; else asks.
pub fn run_confirm_then_act(
    store: &dyn Store,
    tenant: &str,
    confirmed: bool,
) -> Result<(SolValue, String), ErrV1> {
    let bag = SolValue::map([("confirmed", SolValue::Bool(confirmed))]);
    run_installed_harness(store, tenant, "workflow.confirm_then_act", bag)
}

/// Send OTP workflow: bag.phone required for meaningful tool args.
pub fn run_send_otp(
    store: &dyn Store,
    tenant: &str,
    phone: &str,
) -> Result<(SolValue, String), ErrV1> {
    let bag = SolValue::map([("phone", SolValue::str(phone))]);
    run_installed_harness(store, tenant, "tool.send_otp", bag)
}

/// Which installed tool/memory/conversation harness (if any) should handle this utterance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolHarnessIntent {
    SendOtp { phone: String },
    ConfirmThenAct { confirmed: bool },
    MemoryAttach { query: String },
    RunOnce,
    UnderstandIntent { utterance: String },
    WaitForUser,
    /// Catch-all free-form reply (avoids cold ProposePath when installed).
    FullReply { utterance: String },
}

impl ToolHarnessIntent {
    pub fn harness_id(&self) -> &'static str {
        match self {
            Self::SendOtp { .. } => "tool.send_otp",
            Self::ConfirmThenAct { .. } => "workflow.confirm_then_act",
            Self::MemoryAttach { .. } => "memory.attach",
            Self::RunOnce => "tool.run_once",
            Self::UnderstandIntent { .. } => "understand_intent",
            Self::WaitForUser => "wait_for_user",
            Self::FullReply { .. } => "full_reply",
        }
    }

    pub fn input_bag(&self) -> SolValue {
        match self {
            Self::SendOtp { phone } => SolValue::map([("phone", SolValue::str(phone.as_str()))]),
            Self::ConfirmThenAct { confirmed } => {
                SolValue::map([("confirmed", SolValue::Bool(*confirmed))])
            }
            Self::MemoryAttach { query } => {
                SolValue::map([("query", SolValue::str(query.as_str()))])
            }
            Self::RunOnce => SolValue::Null,
            Self::UnderstandIntent { utterance } | Self::FullReply { utterance } => {
                SolValue::map([("utterance", SolValue::str(utterance.as_str()))])
            }
            Self::WaitForUser => SolValue::Null,
        }
    }
}

/// Extract a phone-like token (last 10–15 digit run, optional leading +).
pub fn extract_phone(text: &str) -> Option<String> {
    let mut best: Option<String> = None;
    let mut cur = String::new();
    let mut saw_plus = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            cur.push(ch);
        } else if ch == '+' && cur.is_empty() {
            saw_plus = true;
        } else {
            if cur.len() >= 10 && cur.len() <= 15 {
                let mut s = String::new();
                if saw_plus {
                    s.push('+');
                }
                s.push_str(&cur);
                best = Some(s);
            }
            cur.clear();
            saw_plus = false;
        }
    }
    if cur.len() >= 10 && cur.len() <= 15 {
        let mut s = String::new();
        if saw_plus {
            s.push('+');
        }
        s.push_str(&cur);
        best = Some(s);
    }
    best
}

/// Map user utterance → installed tool harness intent (rule-first).
pub fn resolve_tool_harness_intent(utterance: &str) -> Option<ToolHarnessIntent> {
    let t = utterance.trim().to_lowercase();
    if t.is_empty() {
        return None;
    }
    if t.contains("send otp") || t.contains("send the otp") || t.contains("otp to") {
        let phone = extract_phone(utterance).unwrap_or_else(|| "unknown".into());
        return Some(ToolHarnessIntent::SendOtp { phone });
    }
    if t.contains("login") || t.contains("log in") {
        // Login demo uses OTP send when a phone is present.
        if let Some(phone) = extract_phone(utterance) {
            return Some(ToolHarnessIntent::SendOtp { phone });
        }
    }
    // Confirm workflow — explicit phrases only (avoid "confirm" buried in long free text).
    if t == "please confirm"
        || t == "confirm"
        || t.starts_with("please confirm ")
        || t.contains("confirm and")
        || t.contains("confirm then")
        || t.contains("i confirm")
        || (t.contains("confirm")
            && (t.contains("yes") || t.contains("proceed") || t.contains("do it")))
    {
        let confirmed = t.contains("yes")
            || t.contains("i confirm")
            || t.contains("proceed")
            || t.contains("do it")
            || t.contains("confirm and")
            || t.contains("confirm then");
        // Bare "please confirm" / "confirm" → not yet confirmed.
        let confirmed = confirmed && t != "please confirm" && t != "confirm";
        return Some(ToolHarnessIntent::ConfirmThenAct { confirmed });
    }
    if t.contains("remember") || t.contains("recall") || t.contains("from memory") {
        return Some(ToolHarnessIntent::MemoryAttach {
            query: utterance.trim().to_string(),
        });
    }
    if t.contains("run tool once") || t.contains("invoke stub") {
        return Some(ToolHarnessIntent::RunOnce);
    }
    // Conversation starters as Sol (retire agent dual-IR for these).
    if t.contains("help me")
        || t.contains("i want to")
        || t.contains("i need to")
        || (t.contains('?')
            && (t.contains("how") || t.contains("why") || t.contains("which") || t.contains("should")))
        || t.len() > 80
    {
        return Some(ToolHarnessIntent::UnderstandIntent {
            utterance: utterance.trim().to_string(),
        });
    }
    if t.contains("ask me")
        || t.contains("what do you need")
        || t.contains("hold on")
        || (t.contains("wait") && t.split_whitespace().count() <= 6)
    {
        return Some(ToolHarnessIntent::WaitForUser);
    }
    // Greets/acks are owned by conductor.root cutover — not full_reply.
    if crate::should_cutover_deterministic(utterance) {
        return None;
    }
    // Catch-all: free-form chat stays on Sol when full_reply is installed.
    Some(ToolHarnessIntent::FullReply {
        utterance: utterance.trim().to_string(),
    })
}

/// Run installed harness for intent if present in the store; else `None`.
pub fn try_run_tool_intent(
    store: &dyn Store,
    tenant: &str,
    intent: &ToolHarnessIntent,
) -> Result<Option<(SolValue, String, &'static str)>, ErrV1> {
    let id = intent.harness_id();
    match load_contract(store, tenant, id) {
        Ok(Some(_)) => {
            let (bag, hash) = run_installed_harness(store, tenant, id, intent.input_bag())?;
            Ok(Some((bag, hash, id)))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(ErrV1::new(ReasonCode::Internal, id, format!("{e:?}"))),
    }
}

/// User-facing reply text from a tool harness bag (best-effort).
pub fn tool_bag_reply_text(harness_id: &str, bag: &SolValue) -> String {
    let map = bag.as_map();
    match harness_id {
        "tool.send_otp" => {
            let phone = map
                .and_then(|m| m.get("otp_result"))
                .and_then(|v| v.as_map())
                .and_then(|m| m.get("phone"))
                .and_then(|v| match v {
                    SolValue::Str(s) => Some(s.as_str()),
                    _ => None,
                })
                .unwrap_or("your number");
            format!("OTP sent to {phone}.")
        }
        "workflow.confirm_then_act" => {
            if let Some(text) = map
                .and_then(|m| m.get("out"))
                .and_then(|v| v.as_map())
                .and_then(|m| m.get("text"))
                .and_then(|v| match v {
                    SolValue::Str(s) => Some(s.clone()),
                    _ => None,
                })
            {
                return text;
            }
            if map.and_then(|m| m.get("sent")).is_some() {
                return "Done — action executed.".into();
            }
            "Please confirm to continue.".into()
        }
        "memory.attach" => "Attached memory to context.".into(),
        "tool.run_once" => "Tool ran once.".into(),
        "understand_intent" => {
            if let Some(text) = map
                .and_then(|m| m.get("out"))
                .and_then(|v| v.as_map())
                .and_then(|m| m.get("text"))
                .and_then(|v| match v {
                    SolValue::Str(s) => Some(s.clone()),
                    _ => None,
                })
            {
                return text;
            }
            "I'm clarifying what you need next.".into()
        }
        "wait_for_user" => {
            if let Some(text) = map
                .and_then(|m| m.get("ask1"))
                .and_then(|v| v.as_map())
                .and_then(|m| m.get("text"))
                .and_then(|v| match v {
                    SolValue::Str(s) => Some(s.clone()),
                    _ => None,
                })
            {
                return text;
            }
            "What should I clarify first?".into()
        }
        "full_reply" => {
            if let Some(text) = map
                .and_then(|m| m.get("out"))
                .and_then(|v| v.as_map())
                .and_then(|m| m.get("text"))
                .and_then(|v| match v {
                    SolValue::Str(s) => Some(s.clone()),
                    _ => None,
                })
            {
                return text;
            }
            "I'm here — tell me more.".into()
        }
        _ => format!("Harness `{harness_id}` completed."),
    }
}

/// Memory attach: bag.query.
pub fn run_memory_attach(
    store: &dyn Store,
    tenant: &str,
    query: &str,
) -> Result<(SolValue, String), ErrV1> {
    let bag = SolValue::map([("query", SolValue::str(query))]);
    run_installed_harness(store, tenant, "memory.attach", bag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_bundle::{install_from_manifest, resolve_library_root};
    use aelio_store::MemoryStore;

    fn installed_store() -> MemoryStore {
        let mut store = MemoryStore::new();
        let root = resolve_library_root(None).expect("library");
        install_from_manifest(&mut store, "tenant-tools", &root).unwrap();
        store
    }

    #[test]
    fn confirm_then_act_requires_confirm() {
        let store = installed_store();
        let (bag, _) = run_confirm_then_act(&store, "tenant-tools", false).unwrap();
        let text = bag
            .as_map()
            .and_then(|m| m.get("out"))
            .and_then(|v| v.as_map())
            .and_then(|m| m.get("text"))
            .and_then(|v| match v {
                SolValue::Str(s) => Some(s.as_str()),
                _ => None,
            });
        assert_eq!(text, Some("Please confirm to continue."));
    }

    #[test]
    fn confirm_then_act_runs_tool_when_confirmed() {
        let store = installed_store();
        let (bag, hash) = run_confirm_then_act(&store, "tenant-tools", true).unwrap();
        assert!(!hash.is_empty());
        assert!(bag.as_map().unwrap().contains_key("sent"));
    }

    #[test]
    fn send_otp_and_memory_attach_run() {
        let store = installed_store();
        let (otp, _) = run_send_otp(&store, "tenant-tools", "+15551212").unwrap();
        assert_eq!(
            otp.as_map()
                .and_then(|m| m.get("otp_result"))
                .and_then(|v| v.as_map())
                .and_then(|m| m.get("otp_sent"))
                .cloned(),
            Some(SolValue::Bool(true))
        );
        let (mem, _) = run_memory_attach(&store, "tenant-tools", "preferences").unwrap();
        assert!(mem.as_map().unwrap().contains_key("note"));
    }

    #[test]
    fn resolve_send_otp_intent_and_run() {
        let store = installed_store();
        let intent = resolve_tool_harness_intent("send otp to 9611266596").unwrap();
        assert!(matches!(intent, ToolHarnessIntent::SendOtp { .. }));
        let (bag, hash, id) = try_run_tool_intent(&store, "tenant-tools", &intent)
            .unwrap()
            .expect("installed");
        assert_eq!(id, "tool.send_otp");
        assert!(!hash.is_empty());
        let text = tool_bag_reply_text(id, &bag);
        assert!(text.contains("OTP sent"), "{text}");
    }

    #[test]
    fn extract_phone_from_utterance() {
        assert_eq!(
            extract_phone("send otp to 9611266596"),
            Some("9611266596".into())
        );
        assert_eq!(
            extract_phone("login +919876543210 please"),
            Some("+919876543210".into())
        );
    }
}
