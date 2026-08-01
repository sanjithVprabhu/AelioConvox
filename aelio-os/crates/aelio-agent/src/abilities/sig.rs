//! Response signature machinery.
//! Learn structure. Declare meaning. Mismatch → never guess.

use crate::types::{AelioError, AelioResult, ReasonCode, ResponseRole, ShapeSig, Value};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionPlan {
    pub sig_hash: String,
    pub fields: IndexMap<String, ExtractField>,
    pub status: PlanStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractField {
    pub path: String,
    pub type_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Proposed,
    Candidate,
    Promoted,
    Suspended,
}

#[derive(Debug, Default, Clone)]
pub struct SignatureRegistry {
    /// sig_hash → plan
    pub plans: IndexMap<String, ExtractionPlan>,
}

pub fn compute(raw: &Value) -> ShapeSig {
    raw.shape_sig()
}

pub fn hash(shape: &ShapeSig) -> String {
    shape.hash()
}

pub fn match_plan(reg: &SignatureRegistry, sig_hash: &str) -> Option<ExtractionPlan> {
    reg.plans
        .get(sig_hash)
        .filter(|p| p.status == PlanStatus::Promoted)
        .cloned()
}

/// Apply a promoted extraction plan. Missing fields or incompatible declared types are a hard
/// signature mismatch; partial extraction is never returned.
pub fn extract(raw: &Value, plan: &ExtractionPlan) -> AelioResult<IndexMap<String, Value>> {
    let mut out = IndexMap::new();
    for (name, field) in &plan.fields {
        let value = crate::path::get_path(raw, &field.path)?.ok_or_else(|| {
            AelioError::new(
                ReasonCode::SigMismatch,
                format!("signature field {} is missing at {}", name, field.path),
            )
        })?;
        if field.type_name != "auto"
            && !field
                .type_name
                .eq_ignore_ascii_case(&value.type_tag().to_string())
        {
            return Err(AelioError::new(
                ReasonCode::SigMismatch,
                format!(
                    "signature field {} expected {}, got {}",
                    name,
                    field.type_name,
                    value.type_tag()
                ),
            ));
        }
        out.insert(name.clone(), value);
    }
    Ok(out)
}

pub fn propose(raw: &Value, interpreted: &IndexMap<String, Value>) -> ExtractionPlan {
    propose_with_paths(raw, interpreted, &IndexMap::new())
}

/// Build a plan from the declared semantic field paths. Semantic names frequently do not match
/// top-level response keys, so inventing `$.{name}` here would create a plan which passes on the
/// cold call and fails as soon as it is promoted.
pub fn propose_with_paths(
    raw: &Value,
    interpreted: &IndexMap<String, Value>,
    declared_paths: &IndexMap<String, String>,
) -> ExtractionPlan {
    let shape = compute(raw);
    let sig_hash = hash(&shape);
    let mut fields = IndexMap::new();
    for (name, value) in interpreted {
        fields.insert(
            name.clone(),
            ExtractField {
                path: declared_paths
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| format!("$.{name}")),
                type_name: value.type_tag().to_string(),
            },
        );
    }
    ExtractionPlan {
        sig_hash,
        fields,
        status: PlanStatus::Proposed,
    }
}

pub fn classify(raw: &Value, role_hint: Option<&str>) -> ResponseRole {
    if raw
        .as_map()
        .is_some_and(|values| values.contains_key("error"))
    {
        return ResponseRole::Error;
    }
    if let Some(hint) = role_hint {
        return match hint {
            "effect_confirmation" => ResponseRole::EffectConfirmation,
            "continuation" => ResponseRole::Continuation,
            "error" => ResponseRole::Error,
            _ => ResponseRole::Data,
        };
    }
    if let Some(m) = raw.as_map() {
        if m.contains_key("continuation") || m.contains_key("next") {
            return ResponseRole::Continuation;
        }
        if m.get("ok").and_then(|v| v.as_bool()) == Some(true) && m.len() <= 3 {
            return ResponseRole::EffectConfirmation;
        }
    }
    ResponseRole::Data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mismatch_when_hash_differs() {
        let raw = Value::Map(indexmap::indexmap! {
            "otp".into() => Value::str("123456"),
        });
        let shape = compute(&raw);
        let h = hash(&shape);
        let mut reg = SignatureRegistry::default();
        reg.plans.insert(
            "other_hash".into(),
            ExtractionPlan {
                sig_hash: "other_hash".into(),
                fields: IndexMap::new(),
                status: PlanStatus::Promoted,
            },
        );
        assert!(match_plan(&reg, &h).is_none());
    }

    #[test]
    fn extract_promoted_plan() {
        let raw = Value::Map(indexmap::indexmap! {
            "otp".into() => Value::str("434543"),
            "ok".into() => Value::Bool(true),
        });
        let plan = ExtractionPlan {
            sig_hash: "x".into(),
            fields: indexmap::indexmap! {
                "otp".into() => ExtractField { path: "otp".into(), type_name: "str".into() },
            },
            status: PlanStatus::Promoted,
        };
        let out = extract(&raw, &plan).unwrap();
        assert_eq!(out.get("otp").unwrap().as_str().unwrap(), "434543");
    }

    #[test]
    fn proposed_plan_preserves_declared_nested_path() {
        let raw = Value::Map(indexmap::indexmap! {
            "payload".into() => Value::Map(indexmap::indexmap! {
                "code".into() => Value::str("434543"),
            }),
        });
        let interpreted = indexmap::indexmap! { "otp".into() => Value::str("434543") };
        let paths = indexmap::indexmap! { "otp".into() => "payload.code".into() };
        let mut plan = propose_with_paths(&raw, &interpreted, &paths);
        plan.status = PlanStatus::Promoted;
        assert_eq!(extract(&raw, &plan).unwrap(), interpreted);
    }
}
