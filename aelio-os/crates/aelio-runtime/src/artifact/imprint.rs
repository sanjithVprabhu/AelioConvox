//! Declared nominal imprints. A pin is useful only when it resolves to an admitted declaration;
//! structural `~hash` values remain runtime-derived and are never accepted as build interfaces.

use super::{
    Artifact, ArtifactClass, ArtifactEffect, ArtifactError, ArtifactInterface, ArtifactPins,
    ArtifactRepository, ArtifactStatus, ArtifactTier, Provenance,
};
use aelio_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::collections::{BTreeMap, HashSet};

const MAX_FIELDS: usize = 1_024;
const MAX_TYPE_DEPTH: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImprintSensitivity {
    Public,
    Internal,
    Pii,
    Secret,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ImprintType {
    Null,
    Bool,
    Int,
    Float,
    Str,
    List {
        item: Box<ImprintType>,
        max_items: u32,
    },
    Sol {
        imprint: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImprintField {
    pub name: String,
    pub ty: ImprintType,
    #[serde(default)]
    pub sensitivity: Option<ImprintSensitivity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImprintDeclaration {
    pub id: String,
    pub version: u32,
    #[serde(default)]
    pub required: Vec<ImprintField>,
    #[serde(default)]
    pub optional: Vec<ImprintField>,
    pub open: bool,
    pub default_sensitivity: ImprintSensitivity,
}

impl ImprintDeclaration {
    pub fn key(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }

    pub fn validate(&self) -> Result<(), ArtifactError> {
        super::validate_id(&self.id)?;
        if self.version == 0 || self.required.len() + self.optional.len() > MAX_FIELDS {
            return Err(ArtifactError::Invalid(
                "imprint version/field count is invalid".into(),
            ));
        }
        let mut names = HashSet::new();
        for field in self.required.iter().chain(&self.optional) {
            super::validate_id(&field.name)?;
            if !names.insert(field.name.as_str()) {
                return Err(ArtifactError::Invalid(
                    "imprint field names must be unique".into(),
                ));
            }
            validate_type(&field.ty, 1)?;
        }
        Ok(())
    }

    pub fn as_vendor_artifact(&self) -> Result<Artifact, ArtifactError> {
        self.validate()?;
        Artifact::new(
            self.id.clone(),
            self.version,
            ArtifactClass::Imprint,
            ArtifactTier::Locked,
            "1",
            env!("CARGO_PKG_VERSION"),
            ArtifactInterface {
                inputs: vec![],
                output: "aelio.imprint.declaration@1".into(),
            },
            format!("Declared imprint {}", self.key()),
            vec!["imprint".into(), "vendor".into()],
            vec![ArtifactEffect::Pure],
            ArtifactPins::default(),
            vec![],
            serde_json::json!({"declaration": self}),
            Provenance {
                requester: Some("human-bootstrap".into()),
                metadata: serde_json::Map::from_iter([(
                    "origin".into(),
                    Json::String("vendor".into()),
                )]),
                ..Provenance::default()
            },
        )
    }
}

fn validate_type(ty: &ImprintType, depth: usize) -> Result<(), ArtifactError> {
    if depth > MAX_TYPE_DEPTH {
        return Err(ArtifactError::Invalid(
            "imprint type exceeds max depth".into(),
        ));
    }
    match ty {
        ImprintType::List { item, max_items } => {
            if *max_items == 0 || *max_items > 10_000 {
                return Err(ArtifactError::Invalid(
                    "imprint list requires max_items within 1..=10000".into(),
                ));
            }
            validate_type(item, depth + 1)
        }
        ImprintType::Sol { imprint } => super::validate_pin(imprint),
        ImprintType::Null
        | ImprintType::Bool
        | ImprintType::Int
        | ImprintType::Float
        | ImprintType::Str => Ok(()),
    }
}

#[derive(Clone)]
pub struct ImprintRegistry<S: Store + Clone> {
    artifacts: ArtifactRepository<S>,
}

impl<S: Store + Clone> ImprintRegistry<S> {
    pub fn open(store: S) -> Result<Self, ArtifactError> {
        Ok(Self {
            artifacts: ArtifactRepository::open(store)?,
        })
    }

    pub fn resolve(&self, tenant: &str, pin: &str) -> Result<ImprintDeclaration, ArtifactError> {
        super::validate_tenant(tenant)?;
        let (id, version) = parse_pin(pin)?;
        let tenant_record = self.artifacts.get(tenant, id, version)?;
        let record = match tenant_record {
            Some(record) => record,
            None => self
                .artifacts
                .get(crate::mint::VENDOR_ARTIFACT_TENANT, id, version)?
                .ok_or_else(|| ArtifactError::NotFound(format!("unknown imprint `{pin}`")))?,
        };
        if record.artifact.class != ArtifactClass::Imprint
            || !matches!(
                record.status,
                ArtifactStatus::Canary | ArtifactStatus::Promoted
            )
        {
            return Err(ArtifactError::Invalid(format!(
                "imprint `{pin}` is not admitted"
            )));
        }
        let declaration: ImprintDeclaration = serde_json::from_value(
            record
                .artifact
                .body
                .get("declaration")
                .cloned()
                .ok_or_else(|| ArtifactError::Corrupt("imprint body lacks declaration".into()))?,
        )
        .map_err(|error| ArtifactError::Corrupt(error.to_string()))?;
        declaration.validate()?;
        if declaration.id != id || declaration.version != version {
            return Err(ArtifactError::Corrupt(
                "imprint declaration identity differs from artifact".into(),
            ));
        }
        Ok(declaration)
    }

    pub fn validate_value(
        &self,
        tenant: &str,
        pin: &str,
        value: &Json,
    ) -> Result<(), ArtifactError> {
        let mut path = Vec::new();
        self.validate_declared(tenant, pin, value, &mut path)
    }

    fn validate_declared(
        &self,
        tenant: &str,
        pin: &str,
        value: &Json,
        path: &mut Vec<String>,
    ) -> Result<(), ArtifactError> {
        if path.len() >= MAX_TYPE_DEPTH || path.iter().any(|ancestor| ancestor == pin) {
            return Err(ArtifactError::Invalid(format!(
                "imprint reference cycle/depth at `{pin}`"
            )));
        }
        path.push(pin.into());
        let declaration = self.resolve(tenant, pin)?;
        let map = value
            .as_object()
            .ok_or_else(|| ArtifactError::Invalid(format!("value for `{pin}` must be a map")))?;
        let required: BTreeMap<_, _> = declaration
            .required
            .iter()
            .map(|field| (field.name.as_str(), field))
            .collect();
        let optional: BTreeMap<_, _> = declaration
            .optional
            .iter()
            .map(|field| (field.name.as_str(), field))
            .collect();
        for (name, field) in &required {
            let child = map.get(*name).ok_or_else(|| {
                ArtifactError::Invalid(format!("value for `{pin}` lacks required key `{name}`"))
            })?;
            self.validate_type_value(tenant, &field.ty, child, path)?;
        }
        for (name, child) in map {
            if let Some(field) = required
                .get(name.as_str())
                .or_else(|| optional.get(name.as_str()))
            {
                self.validate_type_value(tenant, &field.ty, child, path)?;
            } else if !declaration.open {
                return Err(ArtifactError::Invalid(format!(
                    "closed imprint `{pin}` rejects key `{name}`"
                )));
            }
        }
        path.pop();
        Ok(())
    }

    fn validate_type_value(
        &self,
        tenant: &str,
        ty: &ImprintType,
        value: &Json,
        path: &mut Vec<String>,
    ) -> Result<(), ArtifactError> {
        let valid = match ty {
            ImprintType::Null => value.is_null(),
            ImprintType::Bool => value.is_boolean(),
            ImprintType::Int => value.as_i64().is_some(),
            ImprintType::Float => value.as_f64().is_some() && !value.is_i64() && !value.is_u64(),
            ImprintType::Str => value.is_string(),
            ImprintType::List { item, max_items } => {
                let Some(values) = value.as_array() else {
                    return Err(ArtifactError::Invalid("expected list value".into()));
                };
                if values.len() > *max_items as usize {
                    return Err(ArtifactError::Invalid(
                        "imprint list exceeds max_items".into(),
                    ));
                }
                for child in values {
                    self.validate_type_value(tenant, item, child, path)?;
                }
                true
            }
            ImprintType::Sol { imprint } => {
                self.validate_declared(tenant, imprint, value, path)?;
                true
            }
        };
        if valid {
            Ok(())
        } else {
            Err(ArtifactError::Invalid(
                "value does not match declared imprint field type".into(),
            ))
        }
    }
}

fn parse_pin(pin: &str) -> Result<(&str, u32), ArtifactError> {
    super::validate_pin(pin)?;
    let (id, version) = pin
        .rsplit_once('@')
        .ok_or_else(|| ArtifactError::Invalid("invalid imprint pin".into()))?;
    let version = version
        .parse::<u32>()
        .map_err(|_| ArtifactError::Invalid("invalid imprint pin version".into()))?;
    Ok((id, version))
}

pub fn base_vendor_imprints() -> Vec<ImprintDeclaration> {
    let open_map = |id: &str| ImprintDeclaration {
        id: id.into(),
        version: 1,
        required: vec![],
        optional: vec![],
        open: true,
        default_sensitivity: ImprintSensitivity::Internal,
    };
    vec![
        open_map("aelio.imprint.declaration"),
        open_map("aelio.turn.input"),
        open_map("aelio.turn.output"),
        open_map("aelio.build_spec"),
        open_map("aelio.build_result"),
        open_map("aelio.selection_result"),
        open_map("aelio.compose_result"),
        open_map("aelio.decompose_result"),
    ]
}
