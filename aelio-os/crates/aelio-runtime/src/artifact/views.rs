//! Typed lowerings for the non-Flow artifact views shared by learning, query and conversion.
//!
//! These structures are deliberately data-only. They do not introduce another executor: a
//! Procedure pins an admitted Harness/Flow, a Pathway pins admitted candidate artifacts, a Dataset
//! names a bounded query surface, and a Converter lowers to the existing closed Glu rule set.

use super::{
    Artifact, ArtifactClass, ArtifactEffect, ArtifactError, ArtifactInput, ArtifactInterface,
    ArtifactPins, ArtifactTier, Provenance,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const VIEW_FORMAT: u32 = 1;
const MAX_PROTOTYPES: usize = 256;
const MAX_PROCEDURE_STEPS: usize = 64;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PathwayPrototype {
    pub artifact: String,
    pub embedding_hash: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PathwayArtifactDraft {
    pub id: String,
    pub version: u32,
    pub decision_point: String,
    pub fallback_artifact: String,
    pub instantiates_flow: bool,
    pub embedding_model: String,
    pub tau: f64,
    pub delta: f64,
    pub entropy_ceiling: f64,
    pub prototypes: Vec<PathwayPrototype>,
    #[serde(default)]
    pub examples: Vec<serde_json::Value>,
}

impl PathwayArtifactDraft {
    pub fn lower(self, provenance: Provenance) -> Result<Artifact, ArtifactError> {
        validate_pathway(&self)?;
        let body = serde_json::to_value(&self).map_err(encode_error)?;
        let mut candidates = self
            .prototypes
            .iter()
            .map(|prototype| prototype.artifact.clone())
            .collect::<Vec<_>>();
        candidates.push(self.fallback_artifact.clone());
        candidates.sort();
        candidates.dedup();
        let description = format!("Pathway classifier for {}", self.decision_point);
        let examples = self.examples.clone();
        Artifact::new(
            self.id,
            self.version,
            ArtifactClass::Pathway,
            ArtifactTier::Auto,
            "1",
            env!("CARGO_PKG_VERSION"),
            ArtifactInterface {
                inputs: vec![ArtifactInput {
                    name: "sense".into(),
                    imprint: "aelio.sense@1".into(),
                    required: true,
                    sensitivity: "internal".into(),
                }],
                output: "aelio.artifact_invocation@1".into(),
            },
            description,
            vec!["typed_view_v1".into(), "pathway".into()],
            vec![ArtifactEffect::Pure],
            ArtifactPins {
                artifacts: candidates,
                embeddings: vec![self.embedding_model.clone()],
                ..ArtifactPins::default()
            },
            examples,
            body,
            provenance,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcedureArtifactDraft {
    pub id: String,
    pub version: u32,
    pub situation_hash: String,
    pub signature: Vec<ProcedureSignatureStep>,
    /// The single admitted Flow/Harness that the reactor executes for this learned view.
    pub implementation: String,
    /// Ordered, immutable artifact pins. Raw tool ids are not executable procedure steps.
    pub steps: Vec<String>,
    #[serde(default)]
    pub tool_dependencies: Vec<String>,
    #[serde(default)]
    pub prompt_dependencies: Vec<String>,
    pub embedding_model: String,
    pub effect: ArtifactEffect,
    #[serde(default)]
    pub examples: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcedureSignatureStep {
    pub call_id: String,
    pub arg_shape: String,
}

impl ProcedureArtifactDraft {
    pub fn lower(self, provenance: Provenance) -> Result<Artifact, ArtifactError> {
        validate_procedure(&self)?;
        let body = serde_json::to_value(&self).map_err(encode_error)?;
        let tier = if matches!(
            self.effect,
            ArtifactEffect::Write | ArtifactEffect::External
        ) {
            ArtifactTier::Reviewed
        } else {
            ArtifactTier::Auto
        };
        let mut artifact_pins = self.steps.clone();
        artifact_pins.push(self.implementation.clone());
        artifact_pins.sort();
        artifact_pins.dedup();
        let description = format!("Learned procedure for situation {}", self.situation_hash);
        let examples = self.examples.clone();
        Artifact::new(
            self.id,
            self.version,
            ArtifactClass::Procedure,
            tier,
            "1",
            env!("CARGO_PKG_VERSION"),
            ArtifactInterface {
                inputs: vec![ArtifactInput {
                    name: "turn".into(),
                    imprint: "aelio.turn.input@1".into(),
                    required: true,
                    sensitivity: "internal".into(),
                }],
                output: "aelio.turn.output@1".into(),
            },
            description,
            vec!["typed_view_v1".into(), "procedure".into()],
            vec![self.effect],
            ArtifactPins {
                artifacts: artifact_pins,
                targets: self.tool_dependencies.clone(),
                prompts: self.prompt_dependencies.clone(),
                embeddings: vec![self.embedding_model.clone()],
                ..ArtifactPins::default()
            },
            examples,
            body,
            provenance,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetModality {
    Get,
    Range,
    TopkVector,
    TopkBm25,
    Traverse,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetArtifactDraft {
    pub id: String,
    pub version: u32,
    pub collection: String,
    pub schema_hash: String,
    pub modalities: Vec<DatasetModality>,
    pub max_limit: u32,
}

impl DatasetArtifactDraft {
    pub fn lower(self, provenance: Provenance) -> Result<Artifact, ArtifactError> {
        validate_dataset(&self)?;
        Artifact::new(
            self.id.clone(),
            self.version,
            ArtifactClass::Dataset,
            ArtifactTier::Locked,
            "1",
            env!("CARGO_PKG_VERSION"),
            ArtifactInterface {
                inputs: vec![ArtifactInput {
                    name: "query".into(),
                    imprint: "aelio.prism.query@1".into(),
                    required: true,
                    sensitivity: "internal".into(),
                }],
                output: "aelio.prism.result@1".into(),
            },
            format!("Dataset view over collection {}", self.collection),
            vec!["typed_view_v1".into(), "dataset".into()],
            vec![ArtifactEffect::Read],
            ArtifactPins::default(),
            Vec::new(),
            serde_json::to_value(self).map_err(encode_error)?,
            provenance,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConverterArtifactDraft {
    pub id: String,
    pub version: u32,
    pub from_imprint: String,
    pub to_imprint: String,
    pub rules: serde_json::Value,
    pub tier: ArtifactTier,
    #[serde(default)]
    pub examples: Vec<serde_json::Value>,
}

impl ConverterArtifactDraft {
    pub fn lower(self, provenance: Provenance) -> Result<Artifact, ArtifactError> {
        validate_converter(&self)?;
        let examples = self.examples.clone();
        Artifact::new(
            self.id.clone(),
            self.version,
            ArtifactClass::Glu,
            self.tier,
            "1",
            env!("CARGO_PKG_VERSION"),
            ArtifactInterface {
                inputs: vec![ArtifactInput {
                    name: "input".into(),
                    imprint: self.from_imprint.clone(),
                    required: true,
                    sensitivity: "internal".into(),
                }],
                output: self.to_imprint.clone(),
            },
            format!(
                "Closed converter {} -> {}",
                self.from_imprint, self.to_imprint
            ),
            vec!["typed_view_v1".into(), "converter".into()],
            vec![ArtifactEffect::Pure],
            ArtifactPins::default(),
            examples,
            serde_json::to_value(self).map_err(encode_error)?,
            provenance,
        )
    }
}

pub(super) fn validate_typed_body(artifact: &Artifact) -> Result<(), ArtifactError> {
    match artifact.class {
        ArtifactClass::Pathway => validate_pathway(&decode_body(artifact)?),
        ArtifactClass::Procedure => validate_procedure(&decode_body(artifact)?),
        ArtifactClass::Dataset => validate_dataset(&decode_body(artifact)?),
        ArtifactClass::Glu => validate_converter(&decode_body(artifact)?),
        _ => Ok(()),
    }
}

fn decode_body<T: for<'de> Deserialize<'de>>(artifact: &Artifact) -> Result<T, ArtifactError> {
    serde_json::from_value(artifact.body.clone()).map_err(|error| {
        ArtifactError::Invalid(format!(
            "{:?} body is not the canonical v{VIEW_FORMAT} view: {error}",
            artifact.class
        ))
    })
}

fn validate_pathway(value: &PathwayArtifactDraft) -> Result<(), ArtifactError> {
    validate_nonempty("decision_point", &value.decision_point)?;
    validate_pin("fallback_artifact", &value.fallback_artifact)?;
    validate_pin("embedding_model", &value.embedding_model)?;
    if value.prototypes.is_empty() || value.prototypes.len() > MAX_PROTOTYPES {
        return invalid("pathway prototypes must contain 1..=256 entries");
    }
    if !value.tau.is_finite()
        || !value.delta.is_finite()
        || !value.entropy_ceiling.is_finite()
        || !(0.0..=1.0).contains(&value.tau)
        || !(0.0..=1.0).contains(&value.delta)
        || !(0.0..=1.0).contains(&value.entropy_ceiling)
    {
        return invalid("pathway hygiene values must be finite within 0..=1");
    }
    let mut pins = BTreeSet::new();
    for prototype in &value.prototypes {
        validate_pin("prototype artifact", &prototype.artifact)?;
        validate_hash("prototype embedding_hash", &prototype.embedding_hash)?;
        if !pins.insert(&prototype.artifact) {
            return invalid("pathway prototype artifact pins must be unique");
        }
    }
    Ok(())
}

fn validate_procedure(value: &ProcedureArtifactDraft) -> Result<(), ArtifactError> {
    validate_hash("situation_hash", &value.situation_hash)?;
    validate_pin("embedding_model", &value.embedding_model)?;
    validate_pin("procedure implementation", &value.implementation)?;
    if value.steps.is_empty() || value.steps.len() > MAX_PROCEDURE_STEPS {
        return invalid("procedure steps must contain 1..=64 artifact pins");
    }
    if value.signature.len() != value.steps.len() {
        return invalid("procedure signature and executable steps must have identical lengths");
    }
    for signature in &value.signature {
        validate_nonempty("procedure signature call_id", &signature.call_id)?;
        if signature.arg_shape.trim().is_empty() || signature.arg_shape.len() > 4_096 {
            return invalid("procedure signature arg_shape must contain 1..=4096 bytes");
        }
    }
    for step in &value.steps {
        validate_pin("procedure step", step)?;
    }
    validate_unique_pins("tool dependencies", &value.tool_dependencies)?;
    validate_unique_pins("prompt dependencies", &value.prompt_dependencies)?;
    if value.effect == ArtifactEffect::Pure && !value.tool_dependencies.is_empty() {
        return invalid("pure procedures cannot declare tool dependencies");
    }
    Ok(())
}

fn validate_dataset(value: &DatasetArtifactDraft) -> Result<(), ArtifactError> {
    validate_nonempty("collection", &value.collection)?;
    validate_hash("schema_hash", &value.schema_hash)?;
    if value.modalities.is_empty() {
        return invalid("dataset must declare at least one query modality");
    }
    if value.modalities.iter().collect::<BTreeSet<_>>().len() != value.modalities.len() {
        return invalid("dataset modalities must be unique");
    }
    if value.max_limit == 0 || value.max_limit > 10_000 {
        return invalid("dataset max_limit must be within 1..=10000");
    }
    Ok(())
}

fn validate_converter(value: &ConverterArtifactDraft) -> Result<(), ArtifactError> {
    validate_pin("from_imprint", &value.from_imprint)?;
    validate_pin("to_imprint", &value.to_imprint)?;
    aelio_convert::parse_rules(&value.rules)
        .map(|_| ())
        .map_err(|error| ArtifactError::Invalid(format!("converter rules are invalid: {error}")))
}

fn validate_unique_pins(name: &str, values: &[String]) -> Result<(), ArtifactError> {
    let mut unique = BTreeSet::new();
    for value in values {
        validate_pin(name, value)?;
        if !unique.insert(value) {
            return invalid(&format!("{name} must be unique"));
        }
    }
    Ok(())
}

fn validate_pin(name: &str, value: &str) -> Result<(), ArtifactError> {
    let Some((id, version)) = value.rsplit_once('@') else {
        return invalid(&format!("{name} must be an immutable id@version pin"));
    };
    if id.trim().is_empty()
        || id.len() > 192
        || version.is_empty()
        || version.starts_with('0')
        || version.parse::<u32>().is_err()
    {
        return invalid(&format!(
            "{name} must be an immutable id@positive-version pin"
        ));
    }
    Ok(())
}

fn validate_hash(name: &str, value: &str) -> Result<(), ArtifactError> {
    if value.len() < 16 || value.len() > 128 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return invalid(&format!(
            "{name} must be a 16..=128 character hexadecimal digest"
        ));
    }
    Ok(())
}

fn validate_nonempty(name: &str, value: &str) -> Result<(), ArtifactError> {
    if value.trim().is_empty() || value.len() > 192 {
        return invalid(&format!("{name} must contain 1..=192 bytes"));
    }
    Ok(())
}

fn invalid<T>(message: &str) -> Result<T, ArtifactError> {
    Err(ArtifactError::Invalid(message.into()))
}

fn encode_error(error: serde_json::Error) -> ArtifactError {
    ArtifactError::Invalid(format!(
        "canonical artifact body cannot be encoded: {error}"
    ))
}
