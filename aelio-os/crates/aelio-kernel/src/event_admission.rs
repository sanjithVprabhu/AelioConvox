//! Normalized event admission (Phase 4.2).
//!
//! Durably dedupe and store [`NormalizedEventV1`] before Conductor scheduling.
//! Dedupe key: `(tenant, source, source_message_id)` or fallback `event_id`.

use crate::harness_syscalls::{
    decide_deterministic, run_conductor_root_deterministic, DeterministicConductorDecision,
};
use crate::os_contract::NormalizedEventV1;
use aelio_sol::SolValue;
use aelio_store::{PutIfAbsent, Store, StoreError};

pub const EVENT_TABLE: &str = "normalized_events_v1";
pub const EVENT_DEDUPE_TABLE: &str = "event_dedupe_v1";

#[derive(Debug, Clone, PartialEq)]
pub enum AdmitStatus {
    /// First time this delivery key is seen.
    Accepted,
    /// Duplicate delivery; prior event_id returned.
    Duplicate { prior_event_id: String },
}

#[derive(Debug, Clone)]
pub struct AdmitResult {
    pub status: AdmitStatus,
    pub event: NormalizedEventV1,
    /// Present when `user.message` was admitted (including duplicates replaying decision).
    pub conductor: Option<ConductorAdmitOutcome>,
}

#[derive(Debug, Clone)]
pub struct ConductorAdmitOutcome {
    pub decision: DeterministicConductorDecision,
    pub route: String,
    pub reply_text: Option<String>,
    pub bag_hash: Option<String>,
}

fn dedupe_key(event: &NormalizedEventV1) -> String {
    let source = event
        .delivery
        .source
        .as_deref()
        .unwrap_or("unknown");
    if let Some(mid) = event.delivery.source_message_id.as_deref() {
        if !mid.is_empty() {
            return format!("{source}|{mid}");
        }
    }
    format!("event_id|{}", event.event_id)
}

fn encode_event(event: &NormalizedEventV1) -> Result<SolValue, StoreError> {
    let json = serde_json::to_string(event)
        .map_err(|e| StoreError::Internal(format!("serialize event: {e}")))?;
    Ok(SolValue::map([
        ("kind", SolValue::str("normalized_event_v1")),
        ("json", SolValue::str(json)),
        ("event_id", SolValue::str(&event.event_id)),
        ("event_type", SolValue::str(&event.event_type)),
        ("subject_id", SolValue::str(&event.subject_id)),
    ]))
}

fn decode_event(value: &SolValue) -> Result<NormalizedEventV1, StoreError> {
    let map = value
        .as_map()
        .ok_or_else(|| StoreError::Internal("event envelope must be a map".into()))?;
    let json = map
        .get("json")
        .and_then(|v| match v {
            SolValue::Str(s) => Some(s.as_str()),
            _ => None,
        })
        .ok_or_else(|| StoreError::Internal("event missing json".into()))?;
    serde_json::from_str(json).map_err(|e| StoreError::Internal(format!("decode event: {e}")))
}

/// Admit a normalized event: validate → dedupe → durable store → optional deterministic Conductor.
pub fn admit_event(
    store: &mut dyn Store,
    event: NormalizedEventV1,
    run_conductor: bool,
) -> Result<AdmitResult, StoreError> {
    event
        .validate()
        .map_err(|e| StoreError::Internal(format!("event validation: {e}")))?;

    let tenant = event.tenant_id.clone();
    let dkey = dedupe_key(&event);
    let claim = SolValue::map([
        ("event_id", SolValue::str(&event.event_id)),
        ("subject_id", SolValue::str(&event.subject_id)),
    ]);

    let status = match store.put_if_absent(&tenant, EVENT_DEDUPE_TABLE, &dkey, claim)? {
        PutIfAbsent::Inserted { .. } => AdmitStatus::Accepted,
        PutIfAbsent::Existing(row) => {
            let prior = row
                .value
                .as_map()
                .and_then(|m| m.get("event_id"))
                .and_then(|v| match v {
                    SolValue::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            // Load prior event body if present.
            let prior_event = store
                .get(&tenant, EVENT_TABLE, &prior)?
                .map(|r| decode_event(&r.value))
                .transpose()?
                .unwrap_or(event.clone());
            let conductor = if run_conductor {
                Some(run_deterministic_conductor(&prior_event)?)
            } else {
                None
            };
            return Ok(AdmitResult {
                status: AdmitStatus::Duplicate {
                    prior_event_id: prior,
                },
                event: prior_event,
                conductor,
            });
        }
    };

    // First acceptance: store event body under event_id.
    let body = encode_event(&event)?;
    match store.put_if_absent(&tenant, EVENT_TABLE, &event.event_id, body)? {
        PutIfAbsent::Inserted { .. } => {}
        PutIfAbsent::Existing(_) => {
            // Same event_id already stored (should not race with dedupe claim).
        }
    }

    let conductor = if run_conductor {
        Some(run_deterministic_conductor(&event)?)
    } else {
        None
    };

    Ok(AdmitResult {
        status,
        event,
        conductor,
    })
}

fn run_deterministic_conductor(
    event: &NormalizedEventV1,
) -> Result<ConductorAdmitOutcome, StoreError> {
    let text = event
        .payload
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let decision = decide_deterministic(&event.event_type, text);
    let route = match decision {
        DeterministicConductorDecision::QuickReplyGreeting
        | DeterministicConductorDecision::QuickReply => "quick_reply",
        DeterministicConductorDecision::UnderstandIntent => "understand_intent",
        DeterministicConductorDecision::SpawnWaitForUser
        | DeterministicConductorDecision::WaitForUser => "spawn_wait",
        DeterministicConductorDecision::MemoryAttach => "memory_attach",
        DeterministicConductorDecision::SpawnAverage => "spawn_average",
        DeterministicConductorDecision::Escalate => "escalate",
    };
    let (dec2, bag, hash) = run_conductor_root_deterministic(text, &event.event_type)
        .map_err(|e| StoreError::Internal(format!("conductor: {}", e.detail)))?;
    debug_assert_eq!(dec2, decision);
    let reply_text = bag
        .as_map()
        .and_then(|m| m.get("reply"))
        .and_then(|r| r.as_map())
        .and_then(|m| m.get("text"))
        .and_then(|v| match v {
            SolValue::Str(s) => Some(s.clone()),
            _ => None,
        });
    Ok(ConductorAdmitOutcome {
        decision,
        route: route.into(),
        reply_text,
        bag_hash: Some(hash),
    })
}

/// Build a NormalizedEventV1 from a classic turn utterance (v1 adapter).
pub fn event_from_user_utterance(
    tenant_id: &str,
    subject_id: &str,
    channel: &str,
    utterance: &str,
    event_id: &str,
    source: &str,
    source_message_id: Option<&str>,
) -> NormalizedEventV1 {
    use crate::os_contract::{EventDeliveryV1, EventIdentityV1};
    use std::collections::BTreeMap;
    NormalizedEventV1 {
        event_id: event_id.into(),
        event_type: "user.message".into(),
        tenant_id: tenant_id.into(),
        subject_id: subject_id.into(),
        channel: channel.into(),
        occurred_at: unix_rfc3339_approx(),
        causation_id: None,
        correlation_id: Some(subject_id.into()),
        payload: serde_json::json!({ "text": utterance }),
        identity: EventIdentityV1 {
            assurance: "anonymous".into(),
        },
        delivery: EventDeliveryV1 {
            source: Some(source.into()),
            source_message_id: source_message_id.map(str::to_string),
        },
        metadata: BTreeMap::new(),
    }
}

/// Approximate RFC3339 timestamp without a chrono dependency (admission does not parse it).
fn unix_rfc3339_approx() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("1970-01-01T00:00:{secs}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use aelio_store::MemoryStore;

    #[test]
    fn admit_dedupes_and_runs_conductor_greeting() {
        let mut store = MemoryStore::new();
        let event = event_from_user_utterance(
            "tenant-1",
            "user-1",
            "web",
            "hello",
            "evt-1",
            "widget",
            Some("msg-1"),
        );
        let first = admit_event(&mut store, event.clone(), true).unwrap();
        assert!(matches!(first.status, AdmitStatus::Accepted));
        let c = first.conductor.unwrap();
        assert!(matches!(
            c.decision,
            DeterministicConductorDecision::QuickReply
                | DeterministicConductorDecision::QuickReplyGreeting
        ));
        assert_eq!(c.reply_text.as_deref(), Some("Hey! I can help you."));

        let second = admit_event(&mut store, event, true).unwrap();
        assert!(matches!(
            second.status,
            AdmitStatus::Duplicate {
                prior_event_id: ref id
            } if id == "evt-1"
        ));
    }

    #[test]
    fn admit_average_route() {
        let mut store = MemoryStore::new();
        let event = event_from_user_utterance(
            "t",
            "u",
            "web",
            "please average these numbers",
            "evt-avg",
            "api",
            None,
        );
        let r = admit_event(&mut store, event, true).unwrap();
        assert_eq!(
            r.conductor.unwrap().decision,
            DeterministicConductorDecision::SpawnAverage
        );
    }
}
