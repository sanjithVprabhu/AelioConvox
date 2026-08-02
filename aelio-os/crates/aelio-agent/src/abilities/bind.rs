//! Bind.* — deterministic argument resolution from declared sources.

use crate::ops::pure::{
    coerce_number_from_text, normalize_email, normalize_phone, normalize_whitespace,
    validate_format, validate_length, validate_pattern, validate_range, FormatKind,
};
use crate::path::get_path;
use crate::tenant::{ParamConstraint, ParamSource, ParamSpec, RepairFn, ToolSpec};
use crate::types::{AelioError, AelioResult, ReasonCode, Value};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveAllResult {
    pub bound: IndexMap<String, Value>,
    pub residual: Vec<String>,
}

/// Sources available to Bind.Resolve / ResolveAll.
#[derive(Debug, Clone, Default)]
pub struct BindSources {
    pub slots: IndexMap<String, Value>,
    pub state: IndexMap<String, Value>,
    pub env: IndexMap<String, Value>,
    pub tool_outputs: IndexMap<String, Value>,
    pub user_text: Option<String>,
}

pub fn resolve_param(spec: &ParamSpec, sources: &BindSources) -> AelioResult<Option<Value>> {
    let raw = match &spec.source {
        ParamSource::User => {
            // residual unless already in slots
            sources.slots.get(&spec.name).cloned()
        }
        ParamSource::Slot { name } => sources.slots.get(name).cloned(),
        ParamSource::State { path } => {
            let doc = Value::Map(sources.state.clone());
            get_path(&doc, path)?
        }
        ParamSource::Env { key } => sources.env.get(key).cloned(),
        ParamSource::Const { value } => Some(value.clone()),
        ParamSource::ToolOutput { ref_path } => {
            // ref_path: "instruction.field" or path into tool_outputs map
            let doc = Value::Map(sources.tool_outputs.clone());
            get_path(&doc, ref_path)?
        }
        ParamSource::Derived { expr } => {
            // Closed derived-expression grammar. Literals must use ParamSource::Const; accepting
            // arbitrary text here would turn a typo into a silently bound argument.
            if let Some(name) = expr.strip_prefix("slot:") {
                sources.slots.get(name).cloned()
            } else if let Some(key) = expr.strip_prefix("env:") {
                sources.env.get(key).cloned()
            } else {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    format!("unsupported derived expression `{expr}`"),
                ));
            }
        }
    };

    let Some(mut v) = raw.or_else(|| spec.default.clone()) else {
        return Ok(None);
    };

    if let Some(repair) = &spec.repair {
        v = apply_repair(&v, repair)?;
    }
    if let Some(c) = &spec.constraint {
        validate_constraint(&v, c)?;
    }
    Ok(Some(v))
}

pub fn resolve_all(tool: &ToolSpec, sources: &BindSources) -> AelioResult<ResolveAllResult> {
    let mut bound = IndexMap::new();
    let mut residual = Vec::new();
    for p in &tool.params {
        match resolve_param(p, sources)? {
            Some(v) => {
                bound.insert(p.name.clone(), v);
            }
            None if p.required => residual.push(p.name.clone()),
            None => {}
        }
    }
    Ok(ResolveAllResult { bound, residual })
}

fn apply_repair(v: &Value, repair: &RepairFn) -> AelioResult<Value> {
    match repair {
        RepairFn::NormalizePhone { region } => normalize_phone(v, region),
        RepairFn::NormalizeEmail => normalize_email(v),
        RepairFn::NormalizeWhitespace => normalize_whitespace(v),
        RepairFn::CoerceNumber => coerce_number_from_text(v),
    }
}

fn validate_constraint(v: &Value, c: &ParamConstraint) -> AelioResult<()> {
    match c {
        ParamConstraint::Range { lo, hi } => {
            validate_range(v, *lo, *hi)?;
        }
        ParamConstraint::Pattern { regex } => {
            validate_pattern(v, regex)?;
        }
        ParamConstraint::Enum { allowed } => {
            let s = v
                .as_str()
                .ok_or_else(|| AelioError::new(ReasonCode::TypeViolation, "enum expects str"))?;
            if !allowed.iter().any(|a| a == s) {
                return Err(AelioError::new(
                    ReasonCode::EnumViolation,
                    format!("{s} not allowed"),
                ));
            }
        }
        ParamConstraint::Length { min, max } => {
            validate_length(v, *min, *max)?;
        }
        ParamConstraint::Format { kind } => {
            let fk = match kind.to_lowercase().as_str() {
                "email" => FormatKind::Email,
                "e164" => FormatKind::E164,
                "url" => FormatKind::Url,
                "uuid" => FormatKind::Uuid,
                "iso8601" => FormatKind::Iso8601,
                "ipv4" => FormatKind::Ipv4,
                other => {
                    return Err(AelioError::new(
                        ReasonCode::Validation,
                        format!("unknown format {other}"),
                    ))
                }
            };
            validate_format(v, fk)?;
        }
    }
    Ok(())
}

pub fn validate_args(tool: &ToolSpec, args: &IndexMap<String, Value>) -> AelioResult<()> {
    for p in &tool.params {
        if p.required && !args.contains_key(&p.name) {
            return Err(AelioError::new(
                ReasonCode::Missing,
                format!("missing {}", p.name),
            ));
        }
        if let Some(v) = args.get(&p.name) {
            if let Some(c) = &p.constraint {
                validate_constraint(v, c)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Sensitivity;

    fn send_otp_tool() -> ToolSpec {
        ToolSpec {
            id: "send_otp".into(),
            name: "send_otp".into(),
            version: "1".into(),
            capability_tags: vec!["auth.otp.send".into()],
            effect: None,
            effectful: true,
            idempotent: false,
            dry_run_available: true,
            params: vec![
                ParamSpec {
                    name: "tenant_id".into(),
                    type_name: "str".into(),
                    required: true,
                    constraint: None,
                    source: ParamSource::Const {
                        value: Value::str("t1"),
                    },
                    repair: None,
                    prompt_hint: None,
                    sensitivity: Sensitivity::None,
                    default: None,
                    depends_on: vec![],
                },
                ParamSpec {
                    name: "timestamp".into(),
                    type_name: "str".into(),
                    required: true,
                    constraint: None,
                    source: ParamSource::Env { key: "now".into() },
                    repair: None,
                    prompt_hint: None,
                    sensitivity: Sensitivity::None,
                    default: None,
                    depends_on: vec![],
                },
                ParamSpec {
                    name: "phone".into(),
                    type_name: "str".into(),
                    required: true,
                    constraint: Some(ParamConstraint::Format {
                        kind: "e164".into(),
                    }),
                    source: ParamSource::User,
                    repair: Some(RepairFn::NormalizePhone {
                        region: "IN".into(),
                    }),
                    prompt_hint: Some("What number should I text the code to?".into()),
                    sensitivity: Sensitivity::Pii,
                    default: None,
                    depends_on: vec![],
                },
            ],
            output_semantics: crate::tenant::OutputSpec {
                fields: IndexMap::new(),
                role_hint: Some("effect_confirmation".into()),
            },
            continuations: vec!["auth.otp.verify".into()],
            errors: vec![],
        }
    }

    #[test]
    fn resolve_only_asks_for_phone() {
        let tool = send_otp_tool();
        let sources = BindSources {
            env: indexmap::indexmap! { "now".into() => Value::str("2026-07-21T09:14:00Z") },
            ..Default::default()
        };
        let r = resolve_all(&tool, &sources).unwrap();
        assert!(r.bound.contains_key("tenant_id"));
        assert!(r.bound.contains_key("timestamp"));
        assert_eq!(r.residual, vec!["phone".to_string()]);
    }

    #[test]
    fn phone_repair_and_validate() {
        let tool = send_otp_tool();
        let sources = BindSources {
            env: indexmap::indexmap! { "now".into() => Value::str("2026-07-21T09:14:00Z") },
            slots: indexmap::indexmap! { "phone".into() => Value::str("98765 43210") },
            ..Default::default()
        };
        let r = resolve_all(&tool, &sources).unwrap();
        assert!(r.residual.is_empty());
        assert_eq!(
            r.bound.get("phone").unwrap().as_str().unwrap(),
            "+919876543210"
        );
    }

    #[test]
    fn unknown_derived_expression_fails_closed() {
        let spec = ParamSpec {
            name: "tenant".into(),
            type_name: "str".into(),
            required: true,
            constraint: None,
            source: ParamSource::Derived {
                expr: "run_tenant_code()".into(),
            },
            repair: None,
            prompt_hint: None,
            sensitivity: Sensitivity::None,
            default: None,
            depends_on: vec![],
        };
        assert_eq!(
            resolve_param(&spec, &BindSources::default())
                .unwrap_err()
                .code,
            ReasonCode::Validation
        );
    }
}
