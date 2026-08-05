//! Multi-turn OTP journey: send → wait for code → verify.
//!
//! Two layers:
//! 1. **Sol program** `tool.otp_login` with Park (kernel Instance park/resume, tests).
//! 2. **Session table** for agent-api multi-turn without owning the whole Instance store.

use crate::compile;
use crate::driver::{Instance, InstanceConfig, TurnOutcome};
use crate::error::{ErrV1, ReasonCode};
use crate::registry::{EffectClass, Registry};
use aelio_sol::SolValue;
use aelio_store::{PutIfAbsent, Store, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const OTP_SESSION_TABLE: &str = "otp_sessions_v1";

/// Sol program: send OTP → ask for code → Park → verify → reply.
pub fn otp_login_program_json() -> String {
    json!({
        "nid": "root",
        "op": "Seq",
        "steps": [
            {
                "nid": "send",
                "op": "Once",
                "body": {
                    "nid": "otp_send",
                    "op": "Call",
                    "id": "tool.send_otp@1",
                    "args": { "phone": { "pull": "phone" } },
                    "into": "otp_result"
                }
            },
            {
                "nid": "ask",
                "op": "Call",
                "id": "express.say@1",
                "args": {
                    "text": { "lit": "OTP sent. Enter the 6-digit code." }
                },
                "into": "ask_code"
            },
            {
                "nid": "wait_code",
                "op": "Park",
                "until": { "kind": "event" },
                "into": "code_wake"
            },
            {
                "nid": "verify",
                "op": "Call",
                "id": "tool.verify_otp@1",
                "args": {
                    "phone": { "pull": "phone" },
                    "code": { "pull": "code_wake" }
                },
                "into": "verify_result"
            },
            {
                "nid": "gate",
                "op": "Branch",
                "pred": {
                    "fn": "eq",
                    "args": [{ "pull": "verify_result.ok" }, { "lit": true }]
                },
                "then": {
                    "nid": "ok",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": { "text": { "lit": "Login verified." } },
                    "into": "out"
                },
                "else": {
                    "nid": "bad",
                    "op": "Call",
                    "id": "express.say@1",
                    "args": { "text": { "lit": "Invalid OTP. Try again." } },
                    "into": "out"
                }
            }
        ]
    })
    .to_string()
}

fn otp_registry() -> Registry {
    use crate::registry::{Boundedness, Declaration, Origin, TargetClass};
    let mut r = Registry::default();
    // Declared production targets required by Instance::with_store / plan_with_registry.
    let say = Declaration {
        id: "express.say@1".into(),
        class: TargetClass::Io,
        input_imprint: "aelio.text@1".into(),
        output_imprint: "aelio.text@1".into(),
        boundedness: Boundedness::CostEnvelope { max_units: 1 },
        effect_class: EffectClass::Read,
        policy_tags: vec![],
        tenant: "vendor".into(),
        origin: Origin::Vendor,
    };
    r.register_declared(say, |args| {
        let text = args
            .as_map()
            .and_then(|m| m.get("text"))
            .cloned()
            .unwrap_or_else(|| SolValue::str(""));
        Ok(SolValue::map([("text", text)]))
    })
    .expect("express.say@1");
    let hold = Declaration {
        id: "compute.hold@1".into(),
        class: TargetClass::Compute,
        input_imprint: "aelio.any@1".into(),
        output_imprint: "aelio.any@1".into(),
        boundedness: Boundedness::CostEnvelope { max_units: 1 },
        effect_class: EffectClass::Pure,
        policy_tags: vec![],
        tenant: "vendor".into(),
        origin: Origin::Vendor,
    };
    r.register_declared(hold, |args| {
        Ok(args
            .as_map()
            .and_then(|m| m.get("v"))
            .cloned()
            .unwrap_or(SolValue::Null))
    })
    .expect("compute.hold@1");
    // Keep pure stdlib for completeness (math.* etc.).
    let _ = crate::register_p0_pure_stdlib(&mut r);
    let send_decl = Declaration {
        id: "tool.send_otp@1".into(),
        class: TargetClass::Tool,
        input_imprint: "aelio.phone@1".into(),
        output_imprint: "aelio.otp.sent@1".into(),
        boundedness: Boundedness::DeadlineCompliant { max_ms: 5_000 },
        effect_class: EffectClass::External,
        policy_tags: vec!["tool.send_otp".into()],
        tenant: "vendor".into(),
        origin: Origin::Vendor,
    };
    r.register_declared(send_decl, |args| {
        let phone = args
            .as_map()
            .and_then(|m| m.get("phone"))
            .cloned()
            .unwrap_or_else(|| SolValue::str(""));
        Ok(SolValue::map([
            ("ok", SolValue::Bool(true)),
            ("otp_sent", SolValue::Bool(true)),
            ("phone", phone),
            ("expected", SolValue::str("123456")),
        ]))
    })
    .expect("declare tool.send_otp@1");
    let verify_decl = Declaration {
        id: "tool.verify_otp@1".into(),
        class: TargetClass::Tool,
        input_imprint: "aelio.otp.verify@1".into(),
        output_imprint: "aelio.otp.result@1".into(),
        boundedness: Boundedness::DeadlineCompliant { max_ms: 5_000 },
        effect_class: EffectClass::External,
        policy_tags: vec!["tool.verify_otp".into()],
        tenant: "vendor".into(),
        origin: Origin::Vendor,
    };
    r.register_declared(verify_decl, |args| {
        let code = match args.as_map().and_then(|m| m.get("code")) {
            Some(SolValue::Str(s)) => s.clone(),
            Some(SolValue::Int(n)) => n.to_string(),
            Some(SolValue::Map(inner)) => match inner.get("text") {
                Some(SolValue::Str(s)) => s.clone(),
                _ => String::new(),
            },
            _ => match args {
                SolValue::Str(s) => s.clone(),
                _ => String::new(),
            },
        };
        let ok = code.trim() == "123456";
        Ok(SolValue::map([("ok", SolValue::Bool(ok))]))
    })
    .expect("declare tool.verify_otp@1");
    r
}

/// Run Sol OTP journey until Park (kernel proof of nested send→park).
pub fn otp_login_start_until_park(
    store: Box<dyn Store + Send>,
    tenant: &str,
    instance_id: &str,
    phone: &str,
) -> Result<(String, /* parked */ bool), ErrV1> {
    let program = compile(&otp_login_program_json())?;
    let mut registry = otp_registry();
    let config = InstanceConfig {
        tenant: tenant.into(),
        instance_id: instance_id.into(),
        flow_id: "tool.otp_login".into(),
        flow_rev: "1".into(),
        event_key_secret: [7; 32],
    };
    let mut inst = Instance::with_store(program, &mut registry, store, config)?;
    let bag = SolValue::map([("phone", SolValue::str(phone))]);
    match inst.start(bag)? {
        TurnOutcome::Parked(p) => {
            let ask = p
                .bag
                .as_map()
                .and_then(|m| m.get("ask_code"))
                .and_then(|v| v.as_map())
                .and_then(|m| m.get("text"))
                .and_then(|v| match v {
                    SolValue::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| "OTP sent. Enter the 6-digit code.".into());
            Ok((ask, true))
        }
        TurnOutcome::Completed { .. } => Err(ErrV1::new(
            ReasonCode::Internal,
            "otp_login",
            "expected Park after send",
        )),
    }
}

/// Resume parked OTP journey with a code wake value.
pub fn otp_login_resume_with_code(
    store: Box<dyn Store + Send>,
    tenant: &str,
    instance_id: &str,
    code: &str,
) -> Result<String, ErrV1> {
    let program = compile(&otp_login_program_json())?;
    let mut registry = otp_registry();
    let config = InstanceConfig {
        tenant: tenant.into(),
        instance_id: instance_id.into(),
        flow_id: "tool.otp_login".into(),
        flow_rev: "1".into(),
        event_key_secret: [7; 32],
    };
    let mut inst = Instance::with_store(program, &mut registry, store, config)?;
    let wake = SolValue::str(code);
    match inst.resume_stored(wake)? {
        TurnOutcome::Completed { bag, .. } => {
            let text = bag
                .as_map()
                .and_then(|m| m.get("out"))
                .and_then(|v| v.as_map())
                .and_then(|m| m.get("text"))
                .and_then(|v| match v {
                    SolValue::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| "done".into());
            Ok(text)
        }
        TurnOutcome::Parked(_) => Err(ErrV1::new(
            ReasonCode::Internal,
            "otp_login",
            "unexpected second park",
        )),
    }
}

// ── Lightweight session for agent-api multi-turn (no Instance store ownership fight) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtpSessionV1 {
    pub phone: String,
    pub expected_code: String,
    pub subject_id: String,
}

fn session_key(subject_id: &str) -> String {
    subject_id.to_string()
}

pub fn otp_session_put(
    store: &mut dyn Store,
    tenant: &str,
    session: &OtpSessionV1,
) -> Result<(), StoreError> {
    let json = serde_json::to_string(session)
        .map_err(|e| StoreError::Internal(e.to_string()))?;
    let value = SolValue::map([
        ("kind", SolValue::str("otp_session_v1")),
        ("json", SolValue::str(json)),
    ]);
    let key = session_key(&session.subject_id);
    match store.put_if_absent(tenant, OTP_SESSION_TABLE, &key, value.clone())? {
        PutIfAbsent::Inserted { .. } => Ok(()),
        PutIfAbsent::Existing(row) => {
            store.cas(tenant, OTP_SESSION_TABLE, &key, row.version, value)?;
            Ok(())
        }
    }
}

pub fn otp_session_get(
    store: &dyn Store,
    tenant: &str,
    subject_id: &str,
) -> Result<Option<OtpSessionV1>, StoreError> {
    match store.get(tenant, OTP_SESSION_TABLE, &session_key(subject_id))? {
        None => Ok(None),
        Some(row) => {
            let map = row
                .value
                .as_map()
                .ok_or_else(|| StoreError::Internal("otp session map".into()))?;
            let kind = map
                .get("kind")
                .and_then(|v| match v {
                    SolValue::Str(s) => Some(s.as_str()),
                    _ => None,
                })
                .unwrap_or("");
            if kind != "otp_session_v1" {
                return Ok(None);
            }
            let json = map
                .get("json")
                .and_then(|v| match v {
                    SolValue::Str(s) => Some(s.as_str()),
                    _ => None,
                })
                .ok_or_else(|| StoreError::Internal("otp session missing json".into()))?;
            if json.trim() == "{}" {
                return Ok(None);
            }
            Ok(Some(
                serde_json::from_str(json).map_err(|e| StoreError::Internal(e.to_string()))?,
            ))
        }
    }
}

pub fn otp_session_clear(
    store: &mut dyn Store,
    tenant: &str,
    subject_id: &str,
) -> Result<(), StoreError> {
    // Soft-clear: overwrite with tombstone via CAS if present.
    let key = session_key(subject_id);
    if let Some(row) = store.get(tenant, OTP_SESSION_TABLE, &key)? {
        let tomb = SolValue::map([
            ("kind", SolValue::str("otp_session_v1_cleared")),
            ("json", SolValue::str("{}")),
        ]);
        let _ = store.cas(tenant, OTP_SESSION_TABLE, &key, row.version, tomb);
    }
    Ok(())
}

/// True if utterance looks like an OTP code (4–8 digits).
pub fn looks_like_otp_code(text: &str) -> bool {
    let t = text.trim();
    (4..=8).contains(&t.len()) && t.chars().all(|c| c.is_ascii_digit())
}

/// API helper: start session after send_otp harness success.
pub fn begin_otp_session_after_send(
    store: &mut dyn Store,
    tenant: &str,
    subject_id: &str,
    phone: &str,
) -> Result<(), StoreError> {
    otp_session_put(
        store,
        tenant,
        &OtpSessionV1 {
            phone: phone.into(),
            expected_code: "123456".into(),
            subject_id: subject_id.into(),
        },
    )
}

/// API helper: if pending session and code utterance, verify and clear.
pub fn try_complete_otp_session(
    store: &mut dyn Store,
    tenant: &str,
    subject_id: &str,
    utterance: &str,
) -> Result<Option<(bool, String)>, StoreError> {
    if !looks_like_otp_code(utterance) {
        return Ok(None);
    }
    let Some(session) = otp_session_get(store, tenant, subject_id)? else {
        return Ok(None);
    };
    // Tombstone sessions have empty phone.
    if session.phone.is_empty() {
        return Ok(None);
    }
    let ok = utterance.trim() == session.expected_code;
    otp_session_clear(store, tenant, subject_id)?;
    let msg = if ok {
        format!("Login verified for {}.", session.phone)
    } else {
        "Invalid OTP. Try again.".into()
    };
    Ok(Some((ok, msg)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aelio_store::MemoryStore;

    #[test]
    fn sol_otp_park_then_resume_verifies() {
        let store = MemoryStore::new();
        let (ask, parked) = otp_login_start_until_park(
            Box::new(store.clone()),
            "t1",
            "otp-user-1",
            "+15551212",
        )
        .unwrap();
        assert!(parked);
        assert!(ask.contains("OTP") || ask.contains("code"));

        let msg = otp_login_resume_with_code(
            Box::new(store),
            "t1",
            "otp-user-1",
            "123456",
        )
        .unwrap();
        assert!(msg.contains("verified") || msg.contains("Login"), "{msg}");
    }

    #[test]
    fn sol_otp_bad_code_fails_closed() {
        let store = MemoryStore::new();
        let _ = otp_login_start_until_park(
            Box::new(store.clone()),
            "t1",
            "otp-user-2",
            "9611266596",
        )
        .unwrap();
        let msg =
            otp_login_resume_with_code(Box::new(store), "t1", "otp-user-2", "000000").unwrap();
        assert!(msg.to_lowercase().contains("invalid"), "{msg}");
    }

    #[test]
    fn session_table_round_trip() {
        let mut store = MemoryStore::new();
        begin_otp_session_after_send(&mut store, "t", "u1", "9611266596").unwrap();
        let done = try_complete_otp_session(&mut store, "t", "u1", "123456")
            .unwrap()
            .unwrap();
        assert!(done.0);
        assert!(try_complete_otp_session(&mut store, "t", "u1", "123456")
            .unwrap()
            .is_none());
    }
}
