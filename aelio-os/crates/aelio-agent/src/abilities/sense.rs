//! Sense.* — default awareness. All pure, free, READ only.

use crate::types::Value;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSense {
    pub now: String,
    pub tz: String,
    pub locale: String,
    pub session_age_secs: u64,
    pub turn_index: u64,
    pub channel: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionSense {
    pub open_loops: Vec<String>,
    pub active_flow: Option<String>,
    pub pending_step: Option<String>,
    pub last_seen: Option<String>,
    pub parked_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetSense {
    pub tokens_left: u64,
    pub ms_left: u64,
    pub calls_left: u64,
    pub cost_spent: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SelfSense {
    pub escalation_rate: f64,
    pub recent_failures: u64,
    pub degraded_abilities: Vec<String>,
    pub cache_hit_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantSense {
    pub tenant_id: String,
    pub plan: String,
    pub feature_flags: Vec<String>,
    pub tool_count: u64,
}

pub fn sense_env(
    now: DateTime<Utc>,
    tz: &str,
    locale: &str,
    session_age_secs: u64,
    turn_index: u64,
    channel: &str,
) -> EnvSense {
    EnvSense {
        now: now.to_rfc3339(),
        tz: tz.into(),
        locale: locale.into(),
        session_age_secs,
        turn_index,
        channel: channel.into(),
    }
}

pub fn sense_session(
    open_loops: Vec<String>,
    active_flow: Option<String>,
    pending_step: Option<String>,
    last_seen: Option<String>,
    parked_at: Option<String>,
) -> SessionSense {
    SessionSense {
        open_loops,
        active_flow,
        pending_step,
        last_seen,
        parked_at,
    }
}

pub fn sense_budget(tokens: u64, ms: u64, calls: u64, cost: f64) -> BudgetSense {
    BudgetSense {
        tokens_left: tokens,
        ms_left: ms,
        calls_left: calls,
        cost_spent: cost,
    }
}

pub fn env_to_value(e: &EnvSense) -> Value {
    Value::Map(indexmap::indexmap! {
        "now".into() => Value::str(&e.now),
        "tz".into() => Value::str(&e.tz),
        "locale".into() => Value::str(&e.locale),
        "session_age_secs".into() => Value::Int(e.session_age_secs as i64),
        "turn_index".into() => Value::Int(e.turn_index as i64),
        "channel".into() => Value::str(&e.channel),
    })
}

pub fn session_to_value(s: &SessionSense) -> Value {
    let mut m = IndexMap::new();
    m.insert(
        "open_loops".into(),
        Value::List(s.open_loops.iter().map(Value::str).collect()),
    );
    m.insert(
        "active_flow".into(),
        s.active_flow
            .as_ref()
            .map(Value::str)
            .unwrap_or(Value::Null),
    );
    m.insert(
        "pending_step".into(),
        s.pending_step
            .as_ref()
            .map(Value::str)
            .unwrap_or(Value::Null),
    );
    m.insert(
        "last_seen".into(),
        s.last_seen.as_ref().map(Value::str).unwrap_or(Value::Null),
    );
    Value::Map(m)
}
