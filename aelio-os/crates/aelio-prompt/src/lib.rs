//! # aelio-prompt — registered templates, never free strings (§10.3)
//!
//! Composition is deterministic: versioned layers + pinned template + args-only slots.
//! The composed prompt's hash is what the ledger records (`prompt_hash`).

use aelio_sol::{value_hash, SolValue};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct SlotDecl {
    pub name: String,
    pub ty: String,
    pub sensitivity: String,
}

#[derive(Debug, Clone)]
pub struct Template {
    pub id: String,
    pub version: String,
    pub body: String,
    pub slots: Vec<SlotDecl>,
    pub layers: Vec<String>,
}

#[derive(Default)]
pub struct TemplateRegistry {
    templates: BTreeMap<String, Template>,
    layers: BTreeMap<String, String>,
}

impl TemplateRegistry {
    pub fn register_layer(&mut self, id: impl Into<String>, text: impl Into<String>) {
        self.layers.insert(id.into(), text.into());
    }

    pub fn register(&mut self, t: Template) {
        let key = format!("{}@{}", t.id, t.version);
        self.templates.insert(key, t);
    }

    pub fn get(&self, id_at_v: &str) -> Option<&Template> {
        self.templates.get(id_at_v)
    }

    /// Compose system layers + template with **args-only** slot fill (§10.3.1).
    /// Unknown slots in `args` are ignored; missing required slots ⇒ error.
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
            let placeholder = format!("{{{{{}}}}}", slot.name);
            if !body.contains(&placeholder) {
                return Err(format!("declared slot `{}` is absent from body", slot.name));
            }
            let val = arg_map
                .and_then(|m| m.get(&slot.name))
                .ok_or_else(|| format!("missing slot `{}`", slot.name))?;
            if val.type_tag().signature() != slot.ty {
                return Err(format!(
                    "slot `{}` expected {}, got {}",
                    slot.name,
                    slot.ty,
                    val.type_tag().signature()
                ));
            }
            projected_args.insert(slot.name.clone(), val.clone());
            // App J: render canonical JSON literals so strings remain unambiguous data.
            let rendered_value = if matches!(slot.sensitivity.as_str(), "pii" | "secret") {
                match val {
                    SolValue::Str(value) => SolValue::str(mask_shape(value)),
                    other => SolValue::str(mask_shape(&aelio_sol::canonical_string(other))),
                }
            } else {
                val.clone()
            };
            body = body.replace(&placeholder, &aelio_sol::canonical_string(&rendered_value));
        }
        if contains_placeholder(&body) {
            return Err("template body contains an undeclared slot or template logic".into());
        }
        parts.push(body);
        let text = parts.join("\n");
        let composed_hash = value_hash(&SolValue::map([
            ("template", SolValue::str(id_at_v)),
            (
                "layers",
                SolValue::list(parts[..parts.len() - 1].iter().cloned().map(SolValue::str)),
            ),
            ("body", SolValue::str(t.body.clone())),
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

#[derive(Debug, Clone)]
pub struct ComposedPrompt {
    pub text: String,
    pub composed_hash: String,
    pub prompt_hash: String,
    pub template: String,
}

fn contains_placeholder(body: &str) -> bool {
    body.contains("{{") || body.contains("}}")
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

    #[test]
    fn compose_is_args_only_and_hash_stable() {
        let mut reg = TemplateRegistry::default();
        reg.register_layer("policy", "Be concise.");
        reg.register(Template {
            id: "ask_phone".into(),
            version: "1".into(),
            body: "Ask for phone in {{lang}}.".into(),
            slots: vec![SlotDecl {
                name: "lang".into(),
                ty: "str".into(),
                sensitivity: "public".into(),
            }],
            layers: vec!["policy".into()],
        });
        let args = SolValue::map([("lang", SolValue::str("en"))]);
        let a = reg.compose("ask_phone@1", &args).unwrap();
        let b = reg.compose("ask_phone@1", &args).unwrap();
        assert_eq!(a.prompt_hash, b.prompt_hash);
        assert!(a.text.contains("Be concise."));
        assert!(a.text.contains("\"en\""));
        // Ambient state cannot appear — only args.
        assert!(!a.text.contains("session"));
    }

    #[test]
    fn composition_rejects_type_mismatch_and_undeclared_template_logic() {
        let mut reg = TemplateRegistry::default();
        reg.register(Template {
            id: "typed".into(),
            version: "1".into(),
            body: "Count={{count}} {{unknown}}".into(),
            slots: vec![SlotDecl {
                name: "count".into(),
                ty: "int".into(),
                sensitivity: "public".into(),
            }],
            layers: vec![],
        });
        assert!(reg
            .compose(
                "typed@1",
                &SolValue::map([("count", SolValue::str("not-an-int"))])
            )
            .is_err());
        assert!(reg
            .compose("typed@1", &SolValue::map([("count", SolValue::Int(2))]))
            .is_err());
    }
}
