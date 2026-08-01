//! # aelio-prompt — registered templates, never free strings (§10.3)
//!
//! Composition is deterministic: versioned layers + pinned template + args-only slots.
//! The composed prompt's hash is what the ledger records (`prompt_hash`).
//!
//! **Mint** (cold path) authors new App J artifacts from the human **root** axiom.
//! Root is never minted. See [`mint`].

pub mod mint;

pub use mint::{
    install_root, mint_artifact, parse_mint_request, prompt_artifact_hash, root_artifact, root_id,
    root_system_text, validate_artifact, Exemplar, MintDraft, MintDrafter, MintError, MintRequest,
    MintResult, MintSlot, MockMintDrafter, ModelPin, PromptArtifact, ROOT_ID, ROOT_VERSION,
};

use aelio_sol::{value_hash, SolValue};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlotDecl {
    pub name: String,
    pub ty: String,
    pub sensitivity: String,
    /// When false, a missing arg fills as `null` rather than failing compose.
    #[serde(default = "default_required")]
    pub required: bool,
}

fn default_required() -> bool {
    true
}

impl SlotDecl {
    pub fn new(
        name: impl Into<String>,
        ty: impl Into<String>,
        sensitivity: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            ty: ty.into(),
            sensitivity: sensitivity.into(),
            required: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Template {
    pub id: String,
    pub version: String,
    pub body: String,
    pub slots: Vec<SlotDecl>,
    pub layers: Vec<String>,
    /// Human description — what this judgement is for (Mint shelf).
    pub description: String,
    /// Declared output imprint id (`judgement.classify@1`). Empty = legacy compose-only.
    pub output_imprint: String,
    /// True only for the human root axiom — Mint must refuse to overwrite.
    pub is_axiom: bool,
}

impl Template {
    pub fn key(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }
}

#[derive(Default)]
pub struct TemplateRegistry {
    templates: BTreeMap<String, Template>,
    layers: BTreeMap<String, String>,
}

impl TemplateRegistry {
    pub fn register_layer(
        &mut self,
        id: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<(), String> {
        let id = id.into();
        let text = text.into();
        if !pinned(&id) || text.is_empty() || text.len() > 64 * 1024 {
            return Err("prompt layer requires pinned id@version and 1..=65536 byte text".into());
        }
        if self.layers.insert(id.clone(), text).is_some() {
            return Err(format!("prompt layer `{id}` already exists"));
        }
        Ok(())
    }

    pub fn register(&mut self, t: Template) -> Result<(), String> {
        validate_template_shape(&t)?;
        let key = t.key();
        if t.is_axiom && key != root_id() {
            return Err("only aelio.mint.root@1 may be marked is_axiom".into());
        }
        if self.templates.insert(key.clone(), t).is_some() {
            return Err(format!("template `{key}` already exists"));
        }
        Ok(())
    }

    /// Register or replace — used only for loading a shelf from durable storage.
    pub fn upsert(&mut self, t: Template) -> Result<(), String> {
        validate_template_shape(&t)?;
        let key = t.key();
        if t.is_axiom && key != root_id() {
            return Err("only aelio.mint.root@1 may be marked is_axiom".into());
        }
        self.templates.insert(key, t);
        Ok(())
    }

    pub fn get(&self, id_at_v: &str) -> Option<&Template> {
        self.templates.get(id_at_v)
    }

    pub fn contains(&self, id_at_v: &str) -> bool {
        self.templates.contains_key(id_at_v)
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.templates.keys()
    }

    /// Compose system layers + template with **args-only** slot fill (§10.3.1).
    pub fn compose(&self, id_at_v: &str, args: &SolValue) -> Result<ComposedPrompt, String> {
        let t = self
            .get(id_at_v)
            .ok_or_else(|| format!("unknown template `{id_at_v}`"))?;
        let mut parts: Vec<String> = Vec::new();
        for layer_id in &t.layers {
            let text = self
                .layers
                .get(layer_id)
                .ok_or_else(|| format!("missing layer `{layer_id}`"))?;
            parts.push(text.clone());
        }
        let mut body = t.body.clone();
        let arg_map = args.as_map();
        let mut names = BTreeSet::new();
        let mut projected_args = BTreeMap::new();
        for slot in &t.slots {
            if !names.insert(slot.name.clone()) {
                return Err(format!("duplicate slot `{}`", slot.name));
            }
            if !matches!(
                slot.sensitivity.as_str(),
                "public" | "internal" | "pii" | "secret"
            ) {
                return Err(format!(
                    "slot `{}` has unknown sensitivity `{}`",
                    slot.name, slot.sensitivity
                ));
            }
            if slot.sensitivity == "secret" {
                return Err(format!(
                    "secret slot `{}` must never be projected into a prompt",
                    slot.name
                ));
            }
            let placeholder = format!("{{{{{}}}}}", slot.name);
            if !body.contains(&placeholder) {
                return Err(format!("declared slot `{}` is absent from body", slot.name));
            }
            let val = match arg_map.and_then(|m| m.get(&slot.name)) {
                Some(v) => v.clone(),
                None if slot.required => {
                    return Err(format!("missing required slot `{}`", slot.name));
                }
                None => SolValue::Null,
            };
            if !matches!(val, SolValue::Null) && val.type_tag().signature() != slot.ty {
                return Err(format!(
                    "slot `{}` expected {}, got {}",
                    slot.name,
                    slot.ty,
                    val.type_tag().signature()
                ));
            }
            projected_args.insert(slot.name.clone(), val.clone());
            let rendered_value = if slot.sensitivity == "pii" {
                match &val {
                    SolValue::Str(value) => SolValue::str(mask_shape(value)),
                    SolValue::Null => SolValue::Null,
                    other => SolValue::str(mask_shape(&aelio_sol::canonical_string(other))),
                }
            } else {
                val
            };
            body = body.replace(&placeholder, &aelio_sol::canonical_string(&rendered_value));
        }
        // Do not parse again after substitution. Slot values are canonical data strings and may
        // legitimately contain `{{...}}` or adjacent JSON braces; substitution is deliberately
        // non-recursive, so those bytes can never become template logic.
        parts.push(body);
        let text = parts.join("\n");
        let composed_hash = value_hash(&SolValue::map([
            ("template", SolValue::str(id_at_v)),
            (
                "layers",
                SolValue::list(parts[..parts.len() - 1].iter().cloned().map(SolValue::str)),
            ),
            ("body", SolValue::str(t.body.clone())),
            ("output_imprint", SolValue::str(t.output_imprint.clone())),
        ]));
        let prompt_hash = value_hash(&SolValue::list([
            SolValue::str(composed_hash.clone()),
            SolValue::Map(projected_args),
        ]));
        Ok(ComposedPrompt {
            text,
            composed_hash,
            prompt_hash,
            template: id_at_v.into(),
        })
    }
}

pub(crate) fn validate_template_shape(t: &Template) -> Result<(), String> {
    if t.id.is_empty()
        || t.version.is_empty()
        || t.body.is_empty()
        || t.body.len() > 64 * 1024
        || t.layers.iter().any(|layer| !pinned(layer))
    {
        return Err("template requires id, version, bounded body, and pinned layer ids".into());
    }
    let mut names = BTreeSet::new();
    for slot in &t.slots {
        if slot.sensitivity == "secret" {
            return Err(format!(
                "secret slot `{}` must never be projected into a prompt",
                slot.name
            ));
        }
        if slot.name.is_empty()
            || !names.insert(slot.name.clone())
            || !matches!(
                slot.sensitivity.as_str(),
                "public" | "internal" | "pii" | "secret"
            )
            || !matches!(
                slot.ty.as_str(),
                "null" | "bool" | "int" | "float" | "str" | "list" | "map"
            )
            || !t.body.contains(&format!("{{{{{}}}}}", slot.name))
        {
            return Err(format!(
                "invalid or duplicate template slot `{}`",
                slot.name
            ));
        }
    }
    let mut remainder = t.body.as_str();
    while let Some(start) = remainder.find("{{") {
        let after = &remainder[start + 2..];
        let end = after
            .find("}}")
            .ok_or("template contains an unclosed slot")?;
        let name = &after[..end];
        if !names.contains(name) {
            return Err(format!("template body contains undeclared slot `{name}`"));
        }
        remainder = &after[end + 2..];
    }
    if remainder.contains("}}") {
        return Err("template contains an unmatched slot terminator".into());
    }
    Ok(())
}

pub(crate) fn pinned(id: &str) -> bool {
    id.split_once('@')
        .is_some_and(|(name, version)| !name.is_empty() && !version.is_empty())
}

#[derive(Debug, Clone)]
pub struct ComposedPrompt {
    pub text: String,
    pub composed_hash: String,
    pub prompt_hash: String,
    pub template: String,
}

fn mask_shape(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_digit() {
                '#'
            } else if c.is_alphabetic() {
                'X'
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(name: &str, ty: &str) -> SlotDecl {
        SlotDecl::new(name, ty, "public")
    }

    #[test]
    fn compose_is_args_only_and_hash_stable() {
        let mut reg = TemplateRegistry::default();
        reg.register_layer("policy@1", "Be concise.").unwrap();
        reg.register(Template {
            id: "ask_phone".into(),
            version: "1".into(),
            body: "Ask for phone in {{lang}}.".into(),
            slots: vec![slot("lang", "str")],
            layers: vec!["policy@1".into()],
            description: String::new(),
            output_imprint: String::new(),
            is_axiom: false,
        })
        .unwrap();
        let args = SolValue::map([("lang", SolValue::str("en"))]);
        let a = reg.compose("ask_phone@1", &args).unwrap();
        let b = reg.compose("ask_phone@1", &args).unwrap();
        assert_eq!(a.prompt_hash, b.prompt_hash);
        assert!(a.text.contains("Be concise."));
        assert!(a.text.contains("\"en\""));
        assert!(!a.text.contains("session"));
    }

    #[test]
    fn composition_rejects_type_mismatch_and_undeclared_template_logic() {
        let mut reg = TemplateRegistry::default();
        assert!(reg
            .register(Template {
                id: "typed".into(),
                version: "1".into(),
                body: "Count={{count}} {{unknown}}".into(),
                slots: vec![slot("count", "int")],
                layers: vec![],
                description: String::new(),
                output_imprint: String::new(),
                is_axiom: false,
            })
            .is_err());
        reg.register(Template {
            id: "typed".into(),
            version: "2".into(),
            body: "Count={{count}}".into(),
            slots: vec![slot("count", "int")],
            layers: vec![],
            description: String::new(),
            output_imprint: String::new(),
            is_axiom: false,
        })
        .unwrap();
        assert!(reg
            .compose(
                "typed@2",
                &SolValue::map([("count", SolValue::str("not-an-int"))])
            )
            .is_err());
        assert!(reg
            .compose("typed@2", &SolValue::map([("count", SolValue::Int(2))]))
            .is_ok());
    }

    #[test]
    fn optional_slot_may_be_absent() {
        let mut reg = TemplateRegistry::default();
        let mut tier = slot("tier", "str");
        tier.required = false;
        reg.register(Template {
            id: "opt".into(),
            version: "1".into(),
            body: "msg={{msg}} tier={{tier}}".into(),
            slots: vec![slot("msg", "str"), tier],
            layers: vec![],
            description: String::new(),
            output_imprint: String::new(),
            is_axiom: false,
        })
        .unwrap();
        let composed = reg
            .compose("opt@1", &SolValue::map([("msg", SolValue::str("hi"))]))
            .unwrap();
        assert!(composed.text.contains("null"));
    }

    #[test]
    fn slot_data_that_looks_like_template_syntax_is_inert() {
        let mut registry = TemplateRegistry::default();
        registry
            .register(Template {
                id: "safe.data".into(),
                version: "1".into(),
                body: "Payload={{payload}}".into(),
                slots: vec![SlotDecl {
                    name: "payload".into(),
                    ty: "str".into(),
                    sensitivity: "public".into(),
                    required: true,
                }],
                layers: vec![],
                description: String::new(),
                output_imprint: "safe.output@1".into(),
                is_axiom: false,
            })
            .unwrap();
        let composed = registry
            .compose(
                "safe.data@1",
                &SolValue::map([("payload", SolValue::str(r#"{{override}} {"nested":{}}"#))]),
            )
            .unwrap();
        assert!(composed.text.contains("{{override}}"));
        assert_eq!(composed.template, "safe.data@1");
    }
}
