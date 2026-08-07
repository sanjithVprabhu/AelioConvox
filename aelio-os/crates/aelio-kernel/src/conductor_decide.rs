//! `conductor.decide@1` — Conductor selection Call (Phase 4 baby → production).
//!
//! **Contract (locked for this slice):**
//!
//! ```text
//! in:
//!   utterance: str                 // user / event text
//!   catalog:   [{id, summary}, …]  // installed harnesses (selection surface)
//!   prompt_id?: str                // reserved for live LLM pin (optional today)
//!
//! out:
//!   kind:        "spawn" | "rough_chat" | "quick_reply"
//!   harness_id?: str               // required when kind=spawn; must be in catalog
//!   x?: int, y?: int               // demo spawn args for demo.sum_ok (provisional)
//!   confidence?: float
//!   source:      "scripted" | "model" | "injected"
//! ```
//!
//! Sol Conductor only Branches on `kind` and invokes by `harness_id`.
//! It does **not** keyword-match. Heuristics / model live **inside** this Call.
//!
//! Replay: External Call — inject the recorded decision map via [`decide_injected`].

use crate::error::{ErrV1, ReasonCode};
use crate::registry::{EffectClass, Registry};
use aelio_sol::SolValue;
use std::sync::Arc;

pub const DECIDE_CALL_ID: &str = "conductor.decide@1";

/// Kinds the Conductor Sol program is allowed to Branch on.
pub const KIND_SPAWN: &str = "spawn";
pub const KIND_ROUGH_CHAT: &str = "rough_chat";
pub const KIND_QUICK_REPLY: &str = "quick_reply";

#[derive(Debug, Clone, PartialEq)]
pub struct DecideArgs {
    pub utterance: String,
    pub catalog: SolValue,
    pub prompt_id: Option<String>,
}

impl DecideArgs {
    pub fn from_sol(args: &SolValue) -> Result<Self, ErrV1> {
        let map = args.as_map().ok_or_else(|| {
            ErrV1::new(ReasonCode::Type, DECIDE_CALL_ID, "args must be a map")
        })?;
        let utterance = map
            .get("utterance")
            .and_then(|v| match v {
                SolValue::Str(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let catalog = map
            .get("catalog")
            .cloned()
            .unwrap_or_else(|| SolValue::List(vec![]));
        if !matches!(catalog, SolValue::List(_)) {
            return Err(ErrV1::new(
                ReasonCode::Type,
                DECIDE_CALL_ID,
                "catalog must be a list of {id, summary} maps",
            ));
        }
        let prompt_id = map.get("prompt_id").and_then(|v| match v {
            SolValue::Str(s) => Some(s.clone()),
            _ => None,
        });
        Ok(Self {
            utterance,
            catalog,
            prompt_id,
        })
    }
}

/// Structured verdict from a model (or model-shaped host) before sealing `source=model`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelVerdict {
    pub kind: String,
    pub harness_id: Option<String>,
    pub confidence: f64,
    pub x: Option<i64>,
    pub y: Option<i64>,
    /// User-facing text when kind is rough_chat / quick_reply (optional for spawn).
    pub reply_text: Option<String>,
}

impl ModelVerdict {
    pub fn to_decision_map(&self, source: &str) -> Result<SolValue, ErrV1> {
        use std::collections::BTreeMap;
        let conf = SolValue::float(self.confidence).map_err(|e| {
            ErrV1::new(
                ReasonCode::Type,
                DECIDE_CALL_ID,
                format!("confidence: {e}"),
            )
        })?;
        let mut map = BTreeMap::new();
        map.insert("kind".into(), SolValue::str(&self.kind));
        map.insert("confidence".into(), conf);
        map.insert("source".into(), SolValue::str(source));
        if let Some(id) = &self.harness_id {
            map.insert("harness_id".into(), SolValue::str(id));
        }
        if let Some(x) = self.x {
            map.insert("x".into(), SolValue::Int(x));
        }
        if let Some(y) = self.y {
            map.insert("y".into(), SolValue::Int(y));
        }
        if let Some(text) = &self.reply_text {
            map.insert("reply_text".into(), SolValue::str(text));
        }
        Ok(SolValue::Map(map))
    }
}

/// Catalog ids present in a decide `catalog` arg.
pub fn catalog_ids(catalog: &SolValue) -> Vec<String> {
    let Some(items) = catalog.as_list() else {
        return vec![];
    };
    items
        .iter()
        .filter_map(|item| {
            item.as_map().and_then(|m| m.get("id")).and_then(|v| match v {
                SolValue::Str(s) => Some(s.clone()),
                _ => None,
            })
        })
        .collect()
}

fn catalog_has(catalog: &SolValue, id: &str) -> bool {
    catalog_ids(catalog).iter().any(|x| x == id)
}

/// Reject spawn decisions that name a harness not in the catalog.
pub fn validate_decision(decision: &SolValue, catalog: &SolValue) -> Result<(), ErrV1> {
    let map = decision.as_map().ok_or_else(|| {
        ErrV1::new(ReasonCode::Type, DECIDE_CALL_ID, "decision must be a map")
    })?;
    let kind = map
        .get("kind")
        .and_then(|v| match v {
            SolValue::Str(s) => Some(s.as_str()),
            _ => None,
        })
        .ok_or_else(|| {
            ErrV1::new(ReasonCode::Missing, DECIDE_CALL_ID, "decision.kind required")
        })?;
    if kind == KIND_SPAWN {
        let hid = map
            .get("harness_id")
            .and_then(|v| match v {
                SolValue::Str(s) => Some(s.as_str()),
                _ => None,
            })
            .ok_or_else(|| {
                ErrV1::new(
                    ReasonCode::Missing,
                    DECIDE_CALL_ID,
                    "spawn requires harness_id",
                )
            })?;
        if !catalog_has(catalog, hid) {
            return Err(ErrV1::new(
                ReasonCode::Policy,
                DECIDE_CALL_ID,
                format!("spawn harness_id={hid} not in catalog"),
            ));
        }
    }
    Ok(())
}

/// Scripted backend (same **out** shape as model-backed decide).
///
/// Temporary heuristics live here — not in Conductor Sol.
pub fn scripted_decide(args: &DecideArgs) -> Result<SolValue, ErrV1> {
    let text = args.utterance.trim().to_lowercase();
    let decision = if catalog_has(&args.catalog, "demo.sum_ok")
        && (text.contains("check") || text.contains("sum") || text.contains("2 and 3"))
    {
        SolValue::map([
            ("kind", SolValue::str(KIND_SPAWN)),
            ("harness_id", SolValue::str("demo.sum_ok")),
            ("x", SolValue::Int(2)),
            ("y", SolValue::Int(3)),
            ("confidence", SolValue::float(0.9).expect("0.9 float")),
            ("source", SolValue::str("scripted")),
            ("reply_text", SolValue::str("")),
        ])
    } else if text.is_empty()
        || text.split_whitespace().count() <= 3
            && ["hi", "hello", "hey", "thanks", "ok", "bye"]
                .iter()
                .any(|w| text == *w || text.starts_with(&format!("{w} ")))
    {
        SolValue::map([
            ("kind", SolValue::str(KIND_QUICK_REPLY)),
            ("confidence", SolValue::float(0.95).expect("float")),
            ("source", SolValue::str("scripted")),
            ("reply_text", SolValue::str("Hey! I can help you.")),
        ])
    } else {
        SolValue::map([
            ("kind", SolValue::str(KIND_ROUGH_CHAT)),
            ("confidence", SolValue::float(0.6).expect("float")),
            ("source", SolValue::str("scripted")),
            (
                "reply_text",
                SolValue::str("Tell me more about what you need."),
            ),
        ])
    };
    validate_decision(&decision, &args.catalog)?;
    Ok(decision)
}

/// Seal a model verdict into the decide out-map (`source=model`) + catalog check.
pub fn model_decide(args: &DecideArgs, verdict: ModelVerdict) -> Result<SolValue, ErrV1> {
    let decision = verdict.to_decision_map("model")?;
    validate_decision(&decision, &args.catalog)?;
    Ok(decision)
}

/// Replay / host path: use a previously ledgered decision (must still pass catalog policy).
pub fn decide_injected(recorded: SolValue, catalog: &SolValue) -> Result<SolValue, ErrV1> {
    validate_decision(&recorded, catalog)?;
    let mut map = recorded
        .as_map()
        .cloned()
        .ok_or_else(|| ErrV1::new(ReasonCode::Type, DECIDE_CALL_ID, "injected must be a map"))?;
    if !map.contains_key("source") {
        map.insert("source".into(), SolValue::str("injected"));
    }
    Ok(SolValue::Map(map))
}

/// Entry point for the default (scripted) registry Call.
pub fn decide(args: &SolValue) -> Result<SolValue, ErrV1> {
    let parsed = DecideArgs::from_sol(args)?;
    scripted_decide(&parsed)
}

/// Register `conductor.decide@1` on an existing registry (scripted backend).
pub fn register_conductor_decide(registry: &mut Registry) {
    registry.register(DECIDE_CALL_ID, EffectClass::External, |args| decide(&args));
}

/// Register `conductor.decide@1` with an injectable model classifier (`source=model`).
pub fn register_conductor_decide_model<F>(registry: &mut Registry, classify: F)
where
    F: Fn(&DecideArgs) -> Result<ModelVerdict, ErrV1> + Send + Sync + 'static,
{
    let classify = Arc::new(classify);
    registry.register(DECIDE_CALL_ID, EffectClass::External, move |args| {
        let parsed = DecideArgs::from_sol(args)?;
        let verdict = classify(&parsed)?;
        model_decide(&parsed, verdict)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog_with_sum_ok() -> SolValue {
        SolValue::List(vec![SolValue::map([
            ("id", SolValue::str("demo.sum_ok")),
            ("summary", SolValue::str("Add x+y; ok if 5")),
        ])])
    }

    #[test]
    fn scripted_spawns_sum_ok_when_in_catalog() {
        let args = DecideArgs {
            utterance: "please check if 2 and 3 make five".into(),
            catalog: catalog_with_sum_ok(),
            prompt_id: None,
        };
        let d = scripted_decide(&args).unwrap();
        let m = d.as_map().unwrap();
        assert_eq!(m.get("kind"), Some(&SolValue::str(KIND_SPAWN)));
        assert_eq!(m.get("harness_id"), Some(&SolValue::str("demo.sum_ok")));
        assert_eq!(m.get("source"), Some(&SolValue::str("scripted")));
    }

    #[test]
    fn spawn_rejected_if_not_in_catalog() {
        let decision = SolValue::map([
            ("kind", SolValue::str(KIND_SPAWN)),
            ("harness_id", SolValue::str("demo.sum_ok")),
        ]);
        let err = validate_decision(&decision, &SolValue::List(vec![])).unwrap_err();
        assert!(err.detail.contains("not in catalog"));
    }

    #[test]
    fn rough_chat_when_no_match() {
        let args = DecideArgs {
            utterance: "tell me a story about the ocean".into(),
            catalog: catalog_with_sum_ok(),
            prompt_id: None,
        };
        let d = scripted_decide(&args).unwrap();
        assert_eq!(
            d.as_map().unwrap().get("kind"),
            Some(&SolValue::str(KIND_ROUGH_CHAT))
        );
    }

    #[test]
    fn model_decide_sets_source_model() {
        let args = DecideArgs {
            utterance: "go".into(),
            catalog: catalog_with_sum_ok(),
            prompt_id: Some("prompt.conductor.decide".into()),
        };
        let d = model_decide(
            &args,
            ModelVerdict {
                kind: KIND_SPAWN.into(),
                harness_id: Some("demo.sum_ok".into()),
                confidence: 0.91,
                x: Some(2),
                y: Some(3),
                reply_text: None,
            },
        )
        .unwrap();
        let m = d.as_map().unwrap();
        assert_eq!(m.get("source"), Some(&SolValue::str("model")));
        assert_eq!(m.get("kind"), Some(&SolValue::str(KIND_SPAWN)));
    }

    #[test]
    fn injected_decision_round_trip() {
        let catalog = catalog_with_sum_ok();
        let recorded = SolValue::map([
            ("kind", SolValue::str(KIND_ROUGH_CHAT)),
            ("confidence", SolValue::float(0.4).unwrap()),
        ]);
        let out = decide_injected(recorded, &catalog).unwrap();
        assert_eq!(
            out.as_map().unwrap().get("source"),
            Some(&SolValue::str("injected"))
        );
    }
}
