//! # Mint — prompt factory inside Aelio (§10.3 / App J)
//!
//! Root (`aelio.mint.root@1`) is the human axiom: it only mints other system prompts.
//! Mint never authors or overwrites root. Minted coins are App J records (body + slots +
//! output contract + description) ready for the Aelio DB shelf.

use crate::{pinned, validate_template_shape, SlotDecl, Template, TemplateRegistry};
use aelio_sol::{value_hash, SolValue};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};

pub const ROOT_ID: &str = "aelio.mint.root";
pub const ROOT_VERSION: &str = "1";

pub fn root_id() -> String {
    format!("{ROOT_ID}@{ROOT_VERSION}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MintSlot {
    pub name: String,
    pub ty: String,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default = "default_public")]
    pub sensitivity: String,
}

fn default_true() -> bool {
    true
}
fn default_public() -> String {
    "public".into()
}

impl From<&MintSlot> for SlotDecl {
    fn from(s: &MintSlot) -> Self {
        SlotDecl {
            name: s.name.clone(),
            ty: s.ty.clone(),
            sensitivity: s.sensitivity.clone(),
            required: s.required,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MintRequest {
    pub tenant: String,
    /// What judgement the new system prompt is for.
    pub objective: String,
    /// Slots the *minted* prompt must accept (your `system_input`).
    pub system_input: Vec<MintSlot>,
    /// Output contract: field name → fundamental type (not a sample value).
    pub output: BTreeMap<String, String>,
    /// Optional worked context for this mint call (your `input`).
    #[serde(default)]
    pub input: Option<Json>,
    /// Optional stable id; otherwise derived from the objective.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default = "default_version")]
    pub version: String,
}

fn default_version() -> String {
    "1".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPin {
    pub id: String,
    #[serde(default)]
    pub params: BTreeMap<String, Json>,
}

impl Default for ModelPin {
    fn default() -> Self {
        Self {
            id: "aelio.model.stub@1".into(),
            params: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exemplar {
    pub slots: BTreeMap<String, Json>,
    pub output: BTreeMap<String, Json>,
}

/// Full minted coin — App J shaped, stored in Aelio DB + registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArtifact {
    pub template_format: u32,
    pub id: String,
    pub version: String,
    pub description: String,
    pub objective: String,
    pub body: String,
    pub slots: Vec<SlotDecl>,
    pub layers: Vec<String>,
    pub output_imprint: String,
    pub output_fields: BTreeMap<String, String>,
    pub model: ModelPin,
    pub exemplars: Vec<Exemplar>,
    pub is_axiom: bool,
    /// Which root version minted this (empty for root itself).
    pub root_version: String,
    pub composed_hash: String,
    /// Whether root judged inputs sufficient for the objective.
    pub inputs_sufficient: bool,
    #[serde(default)]
    pub sufficiency_note: String,
}

impl PromptArtifact {
    pub fn key(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }

    pub fn to_template(&self) -> Template {
        Template {
            id: self.id.clone(),
            version: self.version.clone(),
            body: self.body.clone(),
            slots: self.slots.clone(),
            layers: self.layers.clone(),
            description: self.description.clone(),
            output_imprint: self.output_imprint.clone(),
            is_axiom: self.is_axiom,
        }
    }

    pub fn how_to_input_json(&self) -> Json {
        let slots: Vec<Json> = self
            .slots
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "type": s.ty,
                    "required": s.required,
                    "sensitivity": s.sensitivity,
                })
            })
            .collect();
        serde_json::json!({
            "slots": slots,
            "output_imprint": self.output_imprint,
            "output_fields": self.output_fields,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintDraft {
    pub id: String,
    pub description: String,
    pub body: String,
    pub output_imprint: String,
    pub inputs_sufficient: bool,
    pub sufficiency_note: String,
    #[serde(default)]
    pub exemplars: Vec<Exemplar>,
    #[serde(default)]
    pub model: ModelPin,
}

#[derive(Debug, Clone, Serialize)]
pub struct MintResult {
    pub accepted: bool,
    pub stored: bool,
    pub artifact: PromptArtifact,
    pub drafter: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MintError {
    Invalid(String),
    Refused(String),
    Drafter(String),
}

impl std::fmt::Display for MintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(s) => write!(f, "mint invalid: {s}"),
            Self::Refused(s) => write!(f, "mint refused: {s}"),
            Self::Drafter(s) => write!(f, "mint drafter: {s}"),
        }
    }
}

impl std::error::Error for MintError {}

pub trait MintDrafter: Send + Sync {
    fn name(&self) -> &'static str;
    fn draft(&self, root_text: &str, request: &MintRequest) -> Result<MintDraft, MintError>;
}

/// Human axiom text — the only prompt that mints other prompts.
pub fn root_system_text() -> &'static str {
    r#"You are Aelio Mint root — the axiom prompt factory.
Your only job is to mint new system prompts for small judgements (classify, check, synthesize).
You never rewrite yourself. You never emit free-form chat.

Given:
- objective: what the new prompt must decide
- system_input: typed slots the new prompt will receive
- output: field→type contract the new prompt must return
- optional input: a worked example context

You must:
1. Judge whether system_input can actually answer the objective. If not, set inputs_sufficient=false and explain.
2. Write a system prompt body that uses only {{slot}} placeholders matching system_input names. No conditionals/loops.
3. Write a short description of what the prompt is for and how to feed inputs.
4. Choose output_imprint id like judgement.<slug>@1 matching the output contract.
5. Optionally add one exemplar whose output keys match the output contract.

Respond as a single JSON object with keys:
id, description, body, output_imprint, inputs_sufficient, sufficiency_note, exemplars, model."#
}

pub fn root_artifact() -> PromptArtifact {
    let body = root_system_text().to_string();
    let slots = vec![
        SlotDecl {
            name: "objective".into(),
            ty: "str".into(),
            sensitivity: "internal".into(),
            required: true,
        },
        SlotDecl {
            name: "system_input".into(),
            ty: "str".into(),
            sensitivity: "internal".into(),
            required: true,
        },
        SlotDecl {
            name: "output".into(),
            ty: "str".into(),
            sensitivity: "internal".into(),
            required: true,
        },
        SlotDecl {
            name: "input".into(),
            ty: "str".into(),
            sensitivity: "internal".into(),
            required: false,
        },
    ];
    // Root body is prose instructions; slot placeholders are not in the axiom text —
    // root is composed specially by Mint (args projected into the drafter), not via {{slot}}.
    // Store a compose-safe body for registry discipline:
    let compose_body = "Mint root axiom. objective={{objective}} system_input={{system_input}} output={{output}} input={{input}}".to_string();
    let mut art = PromptArtifact {
        template_format: 1,
        id: ROOT_ID.into(),
        version: ROOT_VERSION.into(),
        description: "Human axiom: mints other system prompts; never self-authored.".into(),
        objective: "mint system prompts".into(),
        body: compose_body,
        slots,
        layers: vec![],
        output_imprint: "mint.artifact@1".into(),
        output_fields: [
            ("id", "str"),
            ("description", "str"),
            ("body", "str"),
            ("output_imprint", "str"),
            ("inputs_sufficient", "bool"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect(),
        model: ModelPin::default(),
        exemplars: vec![],
        is_axiom: true,
        root_version: String::new(),
        composed_hash: String::new(),
        inputs_sufficient: true,
        sufficiency_note: "root is human-authored".into(),
    };
    // Keep the instructional text available via description+layer convention:
    art.description = format!("{}\n\n---\n{}", art.description, body);
    art.composed_hash = prompt_artifact_hash(&art);
    art
}

/// Install root into a registry (idempotent upsert).
pub fn install_root(reg: &mut TemplateRegistry) -> Result<PromptArtifact, MintError> {
    let art = root_artifact();
    reg.upsert(art.to_template()).map_err(MintError::Invalid)?;
    Ok(art)
}

pub fn validate_artifact(art: &PromptArtifact) -> Result<(), MintError> {
    if art.template_format != 1 {
        return Err(MintError::Invalid("template_format must be 1".into()));
    }
    if art.id.trim().is_empty() || art.version.trim().is_empty() {
        return Err(MintError::Invalid("id and version required".into()));
    }
    if art.is_axiom && art.key() != root_id() {
        return Err(MintError::Invalid(
            "only aelio.mint.root@1 may be an axiom".into(),
        ));
    }
    if !art.is_axiom && (art.id == ROOT_ID || art.key() == root_id()) {
        return Err(MintError::Refused(
            "Mint must not create or overwrite root — root is a human axiom".into(),
        ));
    }
    if art.description.trim().is_empty() {
        return Err(MintError::Invalid("description required".into()));
    }
    if art.output_imprint.trim().is_empty() || !pinned(&art.output_imprint) {
        return Err(MintError::Invalid(
            "output_imprint must be pinned id@version".into(),
        ));
    }
    if art.output_fields.is_empty() {
        return Err(MintError::Invalid(
            "output contract must declare at least one field".into(),
        ));
    }
    for (name, ty) in &art.output_fields {
        if name.trim().is_empty()
            || !matches!(
                ty.as_str(),
                "null" | "bool" | "int" | "float" | "str" | "list" | "map"
            )
        {
            return Err(MintError::Invalid(format!(
                "invalid output field `{name}:{ty}`"
            )));
        }
    }
    let tmpl = art.to_template();
    validate_template_shape(&tmpl).map_err(MintError::Invalid)?;
    for ex in &art.exemplars {
        for key in art.output_fields.keys() {
            if !ex.output.contains_key(key) {
                return Err(MintError::Invalid(format!(
                    "exemplar missing output field `{key}`"
                )));
            }
        }
    }
    Ok(())
}

pub fn parse_mint_request(j: &Json) -> Result<MintRequest, MintError> {
    serde_json::from_value(j.clone()).map_err(|e| MintError::Invalid(e.to_string()))
}

/// Draft via root + validate into a [`PromptArtifact`] (does not store).
pub fn mint_artifact(
    request: &MintRequest,
    drafter: &dyn MintDrafter,
) -> Result<MintResult, MintError> {
    validate_request(request)?;
    let root = root_artifact();
    let draft = drafter.draft(&root.description, request)?;
    if !draft.inputs_sufficient {
        return Err(MintError::Refused(format!(
            "inputs insufficient for objective: {}",
            draft.sufficiency_note
        )));
    }

    let id = request
        .id
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| draft.id.clone());
    let id = if id.is_empty() {
        slugify_id(&request.objective)
    } else {
        id
    };
    if id == ROOT_ID || format!("{id}@{}", request.version) == root_id() {
        return Err(MintError::Refused(
            "cannot mint root — root is a human axiom".into(),
        ));
    }

    let slots: Vec<SlotDecl> = request.system_input.iter().map(SlotDecl::from).collect();
    let mut art = PromptArtifact {
        template_format: 1,
        id,
        version: request.version.clone(),
        description: draft.description,
        objective: request.objective.clone(),
        body: draft.body,
        slots,
        layers: vec![],
        output_imprint: draft.output_imprint,
        output_fields: request.output.clone(),
        model: draft.model,
        exemplars: draft.exemplars,
        is_axiom: false,
        root_version: ROOT_VERSION.into(),
        composed_hash: String::new(),
        inputs_sufficient: true,
        sufficiency_note: draft.sufficiency_note,
    };
    if art.output_imprint.trim().is_empty() {
        art.output_imprint = format!("judgement.{}@1", slugify_id(&request.objective));
    }
    art.composed_hash = prompt_artifact_hash(&art);
    validate_artifact(&art)?;

    Ok(MintResult {
        accepted: true,
        stored: false,
        artifact: art,
        drafter: drafter.name().into(),
    })
}

fn validate_request(request: &MintRequest) -> Result<(), MintError> {
    if request.tenant.trim().is_empty() {
        return Err(MintError::Invalid("tenant must not be empty".into()));
    }
    if request.objective.trim().is_empty() {
        return Err(MintError::Invalid("objective must not be empty".into()));
    }
    if request.objective.len() > 8 * 1024 {
        return Err(MintError::Invalid("objective too large".into()));
    }
    if request.system_input.is_empty() {
        return Err(MintError::Invalid(
            "system_input must declare at least one slot".into(),
        ));
    }
    if request.system_input.len() > 64 {
        return Err(MintError::Invalid("system_input capped at 64 slots".into()));
    }
    let mut names = BTreeSet::new();
    for slot in &request.system_input {
        if slot.name.trim().is_empty() || !names.insert(slot.name.clone()) {
            return Err(MintError::Invalid(format!(
                "invalid/duplicate system_input slot `{}`",
                slot.name
            )));
        }
        if !matches!(
            slot.ty.as_str(),
            "null" | "bool" | "int" | "float" | "str" | "list" | "map"
        ) {
            return Err(MintError::Invalid(format!(
                "slot `{}` has invalid type `{}`",
                slot.name, slot.ty
            )));
        }
        if !matches!(
            slot.sensitivity.as_str(),
            "public" | "internal" | "pii" | "secret"
        ) {
            return Err(MintError::Invalid(format!(
                "slot `{}` has invalid sensitivity",
                slot.name
            )));
        }
    }
    if request.output.is_empty() {
        return Err(MintError::Invalid(
            "output contract must declare at least one field".into(),
        ));
    }
    for (name, ty) in &request.output {
        if name.trim().is_empty()
            || !matches!(
                ty.as_str(),
                "null" | "bool" | "int" | "float" | "str" | "list" | "map"
            )
        {
            return Err(MintError::Invalid(format!(
                "invalid output field `{name}:{ty}`"
            )));
        }
    }
    if request.version.trim().is_empty() {
        return Err(MintError::Invalid("version must not be empty".into()));
    }
    Ok(())
}

/// Stable identity of an App-J prompt coin. Runtime vendor installers use the same function so
/// stock and Mint-authored prompt artifacts cannot drift onto different hash rules.
pub fn prompt_artifact_hash(art: &PromptArtifact) -> String {
    let json = serde_json::to_string(&serde_json::json!({
        "id": art.id,
        "version": art.version,
        "body": art.body,
        "slots": art.slots,
        "output_imprint": art.output_imprint,
        "output_fields": art.output_fields,
        "model": art.model,
        "exemplars": art.exemplars,
        "is_axiom": art.is_axiom,
    }))
    .unwrap_or_default();
    value_hash(&SolValue::str(json))
}

pub fn slugify_id(objective: &str) -> String {
    let mut out = String::from("mint.");
    for c in objective.chars().flat_map(|c| c.to_lowercase()) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if matches!(c, ' ' | '-' | '_' | '.') && !out.ends_with('.') {
            out.push('.');
        }
        if out.len() > 48 {
            break;
        }
    }
    while out.ends_with('.') {
        out.pop();
    }
    if out == "mint" || out.is_empty() {
        "mint.prompt".into()
    } else {
        out
    }
}

/// Deterministic local drafter — no LLM. Refuses known-impossible objectives.
pub struct MockMintDrafter;

impl MintDrafter for MockMintDrafter {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn draft(&self, _root_text: &str, request: &MintRequest) -> Result<MintDraft, MintError> {
        let objective = request.objective.to_lowercase();
        let slot_names: BTreeSet<_> = request
            .system_input
            .iter()
            .map(|s| s.name.to_lowercase())
            .collect();

        // Sufficiency: blood type cannot be minted from demographic names alone.
        if objective.contains("blood")
            && !slot_names
                .iter()
                .any(|n| n.contains("blood") || n.contains("genotype") || n.contains("lab"))
        {
            return Ok(MintDraft {
                id: slugify_id(&request.objective),
                description: "refused".into(),
                body: "unused".into(),
                output_imprint: "judgement.refused@1".into(),
                inputs_sufficient: false,
                sufficiency_note:
                    "blood type cannot be determined from the declared system_input slots".into(),
                exemplars: vec![],
                model: ModelPin::default(),
            });
        }

        let mut body = String::from(
            "You are a closed judgement prompt. Answer only with JSON matching the output contract.\n",
        );
        body.push_str("Objective: ");
        body.push_str(request.objective.trim());
        body.push('\n');
        for slot in &request.system_input {
            // Emit literal `{{slot}}` placeholders for App J composition.
            body.push_str(&format!(
                "Input {label} ({ty}{req}): {{{{{name}}}}}\n",
                label = slot.name,
                ty = slot.ty,
                req = if slot.required {
                    ", required"
                } else {
                    ", optional"
                },
                name = slot.name,
            ));
        }
        body.push_str("Return JSON with fields: ");
        body.push_str(
            &request
                .output
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
        );
        body.push('.');

        let id = request
            .id
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| slugify_id(&request.objective));

        let mut exemplar_output = BTreeMap::new();
        for (k, ty) in &request.output {
            exemplar_output.insert(k.clone(), stub_json_for_type(ty));
        }
        let mut exemplar_slots = BTreeMap::new();
        if let Some(Json::Object(map)) = &request.input {
            for (k, v) in map {
                exemplar_slots.insert(k.clone(), v.clone());
            }
        }
        for slot in &request.system_input {
            exemplar_slots
                .entry(slot.name.clone())
                .or_insert_with(|| stub_json_for_type(&slot.ty));
        }

        Ok(MintDraft {
            id,
            description: format!(
                "Minted judgement for: {}. Feed slots {:?}; expect {:?}.",
                request.objective.trim(),
                request
                    .system_input
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>(),
                request.output.keys().collect::<Vec<_>>()
            ),
            body,
            output_imprint: format!("judgement.{}@1", slugify_id(&request.objective)),
            inputs_sufficient: true,
            sufficiency_note: "mock sufficiency pass".into(),
            exemplars: vec![Exemplar {
                slots: exemplar_slots,
                output: exemplar_output,
            }],
            model: ModelPin::default(),
        })
    }
}

fn stub_json_for_type(ty: &str) -> Json {
    match ty {
        "bool" => Json::Bool(true),
        "int" => Json::Number(0.into()),
        "float" => Json::from(0.0),
        "list" => Json::Array(vec![]),
        "map" => Json::Object(Default::default()),
        "null" => Json::Null,
        _ => Json::String("example".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify_req() -> MintRequest {
        MintRequest {
            tenant: "demo".into(),
            objective: "classify an incoming customer message".into(),
            system_input: vec![
                MintSlot {
                    name: "message_text".into(),
                    ty: "str".into(),
                    required: true,
                    sensitivity: "pii".into(),
                },
                MintSlot {
                    name: "customer_tier".into(),
                    ty: "str".into(),
                    required: false,
                    sensitivity: "internal".into(),
                },
            ],
            output: [
                ("class".into(), "str".into()),
                ("confidence".into(), "float".into()),
            ]
            .into_iter()
            .collect(),
            input: Some(serde_json::json!({"message_text": "this is broken"})),
            id: Some("classify.customer".into()),
            version: "1".into(),
        }
    }

    #[test]
    fn mints_building_block_and_installs_root() {
        let mut reg = TemplateRegistry::default();
        let root = install_root(&mut reg).unwrap();
        assert!(root.is_axiom);
        assert_eq!(root.key(), root_id());

        let result = mint_artifact(&classify_req(), &MockMintDrafter).unwrap();
        assert!(result.accepted);
        assert!(!result.artifact.is_axiom);
        assert!(result.artifact.body.contains("{{message_text}}"));
        assert!(result.artifact.body.contains("{{customer_tier}}"));
        validate_artifact(&result.artifact).unwrap();
        reg.register(result.artifact.to_template()).unwrap();
        assert!(reg.contains("classify.customer@1"));
    }

    #[test]
    fn refuses_blood_type_from_demographics() {
        let req = MintRequest {
            tenant: "demo".into(),
            objective: "Create a system prompt for deciphering the blood type".into(),
            system_input: vec![
                MintSlot {
                    name: "name".into(),
                    ty: "str".into(),
                    required: true,
                    sensitivity: "pii".into(),
                },
                MintSlot {
                    name: "age".into(),
                    ty: "int".into(),
                    required: true,
                    sensitivity: "pii".into(),
                },
                MintSlot {
                    name: "location".into(),
                    ty: "str".into(),
                    required: false,
                    sensitivity: "pii".into(),
                },
                MintSlot {
                    name: "nationality".into(),
                    ty: "str".into(),
                    required: false,
                    sensitivity: "pii".into(),
                },
            ],
            output: [("type".into(), "str".into())].into_iter().collect(),
            input: Some(serde_json::json!({"name":"Sanjith","age":29})),
            id: None,
            version: "1".into(),
        };
        let err = mint_artifact(&req, &MockMintDrafter).unwrap_err();
        assert!(matches!(err, MintError::Refused(_)), "{err}");
    }

    #[test]
    fn refuses_minting_root() {
        let mut req = classify_req();
        req.id = Some(ROOT_ID.into());
        let err = mint_artifact(&req, &MockMintDrafter).unwrap_err();
        assert!(matches!(err, MintError::Refused(_)), "{err}");
    }

    #[test]
    fn refuses_secret_prompt_slots() {
        let mut req = classify_req();
        req.system_input[0].sensitivity = "secret".into();
        let err = mint_artifact(&req, &MockMintDrafter).unwrap_err();
        assert!(matches!(err, MintError::Invalid(_)), "{err}");
        assert!(err.to_string().contains("must never be projected"));
    }
}
