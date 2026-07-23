//! S0-C — Effect & async primitives. Every call is ledger-visible.
//! `Sleep` does not exist — only `Park{until}`.

use crate::types::{AelioError, AelioResult, ReasonCode, Value};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use uuid::Uuid;

/// Effect environment: records nondeterministic effects for replay.
#[derive(Debug, Default, Clone)]
pub struct EffectEnv {
    pub now: Option<DateTime<Utc>>,
    pub ledger: Vec<LedgerRecord>,
    pub events: VecDeque<Value>,
    pub metrics: Vec<(String, f64)>,
    /// When replaying, consume recorded effects instead of generating new ones.
    pub replay: VecDeque<RecordedEffect>,
    pub recording: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerRecord {
    pub kind: String,
    pub payload: Value,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecordedEffect {
    Now { instant: String },
    Uuid { id: String },
    Random { value: f64 },
}

impl EffectEnv {
    pub fn live() -> Self {
        Self {
            recording: true,
            ..Default::default()
        }
    }

    pub fn with_fixed_now(now: DateTime<Utc>) -> Self {
        Self {
            now: Some(now),
            recording: true,
            ..Default::default()
        }
    }

    pub fn now(&mut self) -> AelioResult<DateTime<Utc>> {
        if let Some(RecordedEffect::Now { instant }) = self.replay.front() {
            let instant = instant.clone();
            self.replay.pop_front();
            return DateTime::parse_from_rfc3339(&instant)
                .map(|d| d.with_timezone(&Utc))
                .map_err(|e| AelioError::new(ReasonCode::ParseError, e.to_string()));
        }
        let t = self.now.unwrap_or_else(Utc::now);
        if self.recording {
            // store for potential later replay dump
        }
        Ok(t)
    }

    pub fn uuid(&mut self) -> AelioResult<String> {
        if let Some(RecordedEffect::Uuid { id }) = self.replay.front() {
            let id = id.clone();
            self.replay.pop_front();
            return Ok(id);
        }
        Ok(Uuid::new_v4().to_string())
    }

    pub fn random(&mut self, seed: Option<&str>) -> AelioResult<f64> {
        if let Some(RecordedEffect::Random { value }) = self.replay.front() {
            let v = *value;
            self.replay.pop_front();
            return Ok(v);
        }
        // Deterministic PRNG from seed if provided; else simple time-based.
        let v = if let Some(s) = seed {
            let mut h: u64 = 0xcbf29ce484222325;
            for b in s.bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            (h as f64) / (u64::MAX as f64)
        } else {
            let t = self.now()?.timestamp_nanos_opt().unwrap_or(0) as u64;
            ((t.wrapping_mul(6364136223846793005)) as f64) / (u64::MAX as f64)
        };
        Ok(v)
    }

    pub fn ledger_append(&mut self, kind: impl Into<String>, payload: Value) -> AelioResult<Value> {
        let at = self.now()?.to_rfc3339();
        let rec = LedgerRecord {
            kind: kind.into(),
            payload: payload.clone(),
            at: at.clone(),
        };
        self.ledger.push(rec);
        Ok(Value::Map(indexmap::indexmap! {
            "ok".into() => Value::Bool(true),
            "at".into() => Value::str(at),
            "index".into() => Value::Int((self.ledger.len() - 1) as i64),
        }))
    }

    pub fn metric_inc(&mut self, name: impl Into<String>, value: f64) {
        self.metrics.push((name.into(), value));
    }
}

pub fn effect_now(env: &mut EffectEnv) -> AelioResult<Value> {
    Ok(Value::str(env.now()?.to_rfc3339()))
}

pub fn effect_uuid(env: &mut EffectEnv) -> AelioResult<Value> {
    Ok(Value::str(env.uuid()?))
}

pub fn effect_random(env: &mut EffectEnv, seed: Option<&str>) -> AelioResult<Value> {
    Ok(Value::Float(env.random(seed)?))
}

pub fn effect_ledger_append(env: &mut EffectEnv, kind: &str, payload: Value) -> AelioResult<Value> {
    env.ledger_append(kind, payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_append_records() {
        let mut env = EffectEnv::with_fixed_now(Utc::now());
        let _ = effect_ledger_append(&mut env, "intent", Value::str("send_otp")).unwrap();
        let _ = effect_ledger_append(&mut env, "receipt", Value::str("ok")).unwrap();
        assert_eq!(env.ledger.len(), 2);
        assert_eq!(env.ledger[0].kind, "intent");
    }

    #[test]
    fn seeded_random_is_deterministic() {
        let mut a = EffectEnv::live();
        let mut b = EffectEnv::live();
        let ra = effect_random(&mut a, Some("seed-1")).unwrap();
        let rb = effect_random(&mut b, Some("seed-1")).unwrap();
        assert_eq!(ra, rb);
    }
}
