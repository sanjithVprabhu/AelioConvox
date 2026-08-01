//! Canonical executable harness composition. Models may report nodes and seams; this closed form
//! is admitted only after the kernel resolves every pin and proves graph/interface closure.

use super::{validate_id, validate_pin, ArtifactError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HarnessSource {
    ParentInput { slot: String },
    NodeOutput { node: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HarnessSink {
    NodeInput { node: String, slot: String },
    ParentOutput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessSeam {
    pub from: HarnessSource,
    pub to: HarnessSink,
    pub from_imprint: String,
    pub to_imprint: String,
    /// `None` is legal only for identical imprints. Mismatches carry an admitted Glu pin.
    #[serde(default)]
    pub converter: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessNode {
    pub nid: String,
    pub artifact: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessBody {
    pub nodes: Vec<HarnessNode>,
    pub seams: Vec<HarnessSeam>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecompositionSeam {
    pub from: HarnessSource,
    pub to: HarnessSink,
}

impl HarnessBody {
    pub fn validate_shape(&self) -> Result<(), ArtifactError> {
        if self.nodes.is_empty() || self.nodes.len() > 64 || self.seams.len() > 1_024 {
            return Err(ArtifactError::Invalid(
                "harness requires 1..=64 nodes and at most 1024 seams".into(),
            ));
        }
        let mut nids = std::collections::HashSet::new();
        for node in &self.nodes {
            validate_id(&node.nid)?;
            validate_pin(&node.artifact)?;
            if !nids.insert(node.nid.as_str()) {
                return Err(ArtifactError::Invalid(
                    "harness node ids must be unique".into(),
                ));
            }
        }
        for seam in &self.seams {
            validate_source(&seam.from)?;
            validate_sink(&seam.to)?;
            validate_pin(&seam.from_imprint)?;
            validate_pin(&seam.to_imprint)?;
            if let Some(converter) = &seam.converter {
                validate_pin(converter)?;
            }
            if seam.from_imprint == seam.to_imprint && seam.converter.is_some() {
                return Err(ArtifactError::Invalid(
                    "identity seam must not carry a converter".into(),
                ));
            }
            if seam.from_imprint != seam.to_imprint && seam.converter.is_none() {
                return Err(ArtifactError::Invalid(
                    "mismatched seam requires a pinned converter".into(),
                ));
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_source(source: &HarnessSource) -> Result<(), ArtifactError> {
    match source {
        HarnessSource::ParentInput { slot } => validate_id(slot),
        HarnessSource::NodeOutput { node } => validate_id(node),
    }
}

pub(crate) fn validate_sink(sink: &HarnessSink) -> Result<(), ArtifactError> {
    match sink {
        HarnessSink::NodeInput { node, slot } => {
            validate_id(node)?;
            validate_id(slot)
        }
        HarnessSink::ParentOutput => Ok(()),
    }
}
