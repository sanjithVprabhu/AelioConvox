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
    Any,
    Null,
    Bool,
    Int,
    Float,
    Str,
    Number,
    IntRange {
        min: i64,
        max: i64,
    },
    StrBound {
        max_bytes: u32,
    },
    StrEnum {
        values: Vec<String>,
    },
    Map {
        #[serde(default)]
        required: Vec<ImprintField>,
        #[serde(default)]
        optional: Vec<ImprintField>,
        open: bool,
    },
    OneOf {
        variants: Vec<ImprintType>,
    },
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
        ImprintType::IntRange { min, max } if min > max => Err(ArtifactError::Invalid(
            "imprint integer range is inverted".into(),
        )),
        ImprintType::StrBound { max_bytes } if *max_bytes == 0 || *max_bytes > 16 * 1024 * 1024 => {
            Err(ArtifactError::Invalid(
                "imprint string bound is invalid".into(),
            ))
        }
        ImprintType::StrEnum { values } => {
            let unique = values.iter().collect::<HashSet<_>>();
            if values.is_empty()
                || values.len() > 1_024
                || unique.len() != values.len()
                || values
                    .iter()
                    .any(|value| value.is_empty() || value.len() > 512)
            {
                Err(ArtifactError::Invalid(
                    "imprint string enum is invalid".into(),
                ))
            } else {
                Ok(())
            }
        }
        ImprintType::Map {
            required, optional, ..
        } => validate_inline_fields(required, optional, depth + 1),
        ImprintType::OneOf { variants } => {
            if variants.len() < 2 || variants.len() > 64 {
                return Err(ArtifactError::Invalid(
                    "imprint one_of requires 2..=64 variants".into(),
                ));
            }
            for variant in variants {
                validate_type(variant, depth + 1)?;
            }
            Ok(())
        }
        ImprintType::Any
        | ImprintType::Null
        | ImprintType::Bool
        | ImprintType::Int
        | ImprintType::Float
        | ImprintType::Str
        | ImprintType::Number
        | ImprintType::IntRange { .. }
        | ImprintType::StrBound { .. } => Ok(()),
    }
}

fn validate_inline_fields(
    required: &[ImprintField],
    optional: &[ImprintField],
    depth: usize,
) -> Result<(), ArtifactError> {
    if required.len() + optional.len() > MAX_FIELDS {
        return Err(ArtifactError::Invalid(
            "inline imprint map has too many fields".into(),
        ));
    }
    let mut names = HashSet::new();
    for field in required.iter().chain(optional) {
        super::validate_id(&field.name)?;
        if !names.insert(field.name.as_str()) {
            return Err(ArtifactError::Invalid(
                "inline imprint field names must be unique".into(),
            ));
        }
        validate_type(&field.ty, depth)?;
    }
    Ok(())
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
            ImprintType::Any => true,
            ImprintType::Null => value.is_null(),
            ImprintType::Bool => value.is_boolean(),
            ImprintType::Int => value.as_i64().is_some(),
            ImprintType::Float => value.as_f64().is_some() && !value.is_i64() && !value.is_u64(),
            ImprintType::Str => value.is_string(),
            ImprintType::Number => value.is_number(),
            ImprintType::IntRange { min, max } => value
                .as_i64()
                .is_some_and(|value| (*min..=*max).contains(&value)),
            ImprintType::StrBound { max_bytes } => value
                .as_str()
                .is_some_and(|value| value.len() <= *max_bytes as usize),
            ImprintType::StrEnum { values } => value
                .as_str()
                .is_some_and(|value| values.iter().any(|allowed| allowed == value)),
            ImprintType::Map {
                required,
                optional,
                open,
            } => {
                self.validate_inline_map(tenant, required, optional, *open, value, path)?;
                true
            }
            ImprintType::OneOf { variants } => variants.iter().any(|variant| {
                let mut branch = path.clone();
                self.validate_type_value(tenant, variant, value, &mut branch)
                    .is_ok()
            }),
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

    fn validate_inline_map(
        &self,
        tenant: &str,
        required: &[ImprintField],
        optional: &[ImprintField],
        open: bool,
        value: &Json,
        path: &mut Vec<String>,
    ) -> Result<(), ArtifactError> {
        let map = value
            .as_object()
            .ok_or_else(|| ArtifactError::Invalid("expected map value".into()))?;
        let required = required
            .iter()
            .map(|field| (field.name.as_str(), field))
            .collect::<BTreeMap<_, _>>();
        let optional = optional
            .iter()
            .map(|field| (field.name.as_str(), field))
            .collect::<BTreeMap<_, _>>();
        for (name, field) in &required {
            let child = map.get(*name).ok_or_else(|| {
                ArtifactError::Invalid(format!("inline imprint map lacks required key `{name}`"))
            })?;
            self.validate_type_value(tenant, &field.ty, child, path)?;
        }
        for (name, child) in map {
            if let Some(field) = required
                .get(name.as_str())
                .or_else(|| optional.get(name.as_str()))
            {
                self.validate_type_value(tenant, &field.ty, child, path)?;
            } else if !open {
                return Err(ArtifactError::Invalid(format!(
                    "closed inline imprint map rejects key `{name}`"
                )));
            }
        }
        Ok(())
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
    let closed =
        |id: &str, version: u32, required: Vec<ImprintField>, optional: Vec<ImprintField>| {
            ImprintDeclaration {
                id: id.into(),
                version,
                required,
                optional,
                open: false,
                default_sensitivity: ImprintSensitivity::Internal,
            }
        };
    let string = || ImprintType::StrBound { max_bytes: 16_384 };
    let field = |name: &str, ty: ImprintType| ImprintField {
        name: name.into(),
        ty,
        sensitivity: None,
    };
    let list = |item: ImprintType, max_items| ImprintType::List {
        item: Box::new(item),
        max_items,
    };
    let map = |required: Vec<ImprintField>, optional: Vec<ImprintField>| ImprintType::Map {
        required,
        optional,
        open: false,
    };
    let slot = map(
        vec![
            field("name", string()),
            field("imprint", string()),
            field("required", ImprintType::Bool),
        ],
        vec![field(
            "sensitivity",
            ImprintType::StrEnum {
                values: ["public", "internal", "pii", "secret"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            },
        )],
    );
    let effect_list = || {
        list(
            ImprintType::StrEnum {
                values: ["pure", "read", "write", "external"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            },
            4,
        )
    };
    let cost = map(
        vec![
            field("llm_calls", ImprintType::Int),
            field("tokens", ImprintType::Int),
            field("wall_ms", ImprintType::Int),
            field("depth_reached", ImprintType::Int),
            field("retries", ImprintType::Int),
        ],
        vec![],
    );
    let selection_choice = map(
        vec![
            field("id", string()),
            field("score", ImprintType::Number),
            field("role", string()),
        ],
        vec![],
    );
    let runner_up = map(
        vec![field("id", string()), field("score", ImprintType::Number)],
        vec![],
    );
    let composition_node = map(
        vec![field("nid", string()), field("artifact", string())],
        vec![],
    );
    let seam_source = ImprintType::OneOf {
        variants: vec![
            map(
                vec![
                    field(
                        "kind",
                        ImprintType::StrEnum {
                            values: vec!["parent_input".into()],
                        },
                    ),
                    field("slot", string()),
                ],
                vec![],
            ),
            map(
                vec![
                    field(
                        "kind",
                        ImprintType::StrEnum {
                            values: vec!["node_output".into()],
                        },
                    ),
                    field("node", string()),
                ],
                vec![],
            ),
        ],
    };
    let seam_sink = ImprintType::OneOf {
        variants: vec![
            map(
                vec![
                    field(
                        "kind",
                        ImprintType::StrEnum {
                            values: vec!["node_input".into()],
                        },
                    ),
                    field("node", string()),
                    field("slot", string()),
                ],
                vec![],
            ),
            map(
                vec![field(
                    "kind",
                    ImprintType::StrEnum {
                        values: vec!["parent_output".into()],
                    },
                )],
                vec![],
            ),
        ],
    };
    let composition_seam = map(
        vec![field("from", seam_source), field("to", seam_sink)],
        vec![],
    );
    vec![
        open_map("aelio.imprint.declaration"),
        open_map("aelio.turn.input"),
        open_map("aelio.turn.output"),
        open_map("aelio.build_spec"),
        open_map("aelio.build_result"),
        open_map("aelio.selection_result"),
        open_map("aelio.compose_result"),
        open_map("aelio.decompose_result"),
        closed(
            "aelio.build_spec",
            2,
            vec![
                field("name", ImprintType::StrBound { max_bytes: 192 }),
                field("description", ImprintType::StrBound { max_bytes: 500 }),
                field("inputs", list(slot.clone(), 256)),
                field("output", string()),
                field(
                    "budget",
                    map(
                        vec![
                            field("max_depth", ImprintType::IntRange { min: 1, max: 32 }),
                            field("max_children", ImprintType::IntRange { min: 1, max: 64 }),
                            field(
                                "max_llm_calls",
                                ImprintType::IntRange { min: 1, max: 1_000 },
                            ),
                            field(
                                "max_tokens",
                                ImprintType::IntRange {
                                    min: 1,
                                    max: 100_000_000,
                                },
                            ),
                            field(
                                "max_reactions",
                                ImprintType::IntRange {
                                    min: 1,
                                    max: 100_000,
                                },
                            ),
                            field(
                                "max_wall_ms",
                                ImprintType::IntRange {
                                    min: 1,
                                    max: 86_400_000,
                                },
                            ),
                        ],
                        vec![],
                    ),
                ),
                field(
                    "scope",
                    map(
                        vec![
                            field("tenant", string()),
                            field("registries", list(string(), 128)),
                        ],
                        vec![],
                    ),
                ),
                field(
                    "policy",
                    map(
                        vec![
                            field("principal_grants", effect_list()),
                            field("allowed_effects", effect_list()),
                            field("denied_effects", effect_list()),
                        ],
                        vec![],
                    ),
                ),
                field(
                    "examples",
                    list(
                        map(
                            vec![
                                field("inputs", ImprintType::Any),
                                field("output", ImprintType::Any),
                                field("negative", ImprintType::Bool),
                                field("fixtures", list(ImprintType::Any, 256)),
                            ],
                            vec![],
                        ),
                        256,
                    ),
                ),
                field("spec_hash", ImprintType::StrBound { max_bytes: 64 }),
            ],
            vec![],
        ),
        closed(
            "aelio.build_result",
            2,
            vec![
                field(
                    "status",
                    ImprintType::StrEnum {
                        values: ["built", "reused", "failed"]
                            .into_iter()
                            .map(str::to_owned)
                            .collect(),
                    },
                ),
                field("spec_hash", ImprintType::StrBound { max_bytes: 64 }),
                field("cost", cost),
                field("ledger", list(string(), 4_096)),
            ],
            vec![
                field("artifact", ImprintType::Any),
                field("artifact_pin", string()),
                field("failure", ImprintType::Any),
            ],
        ),
        closed(
            "aelio.selection_result",
            2,
            vec![
                field("selected", list(selection_choice, 25)),
                field("runners_up", list(runner_up, 25)),
                field("unmet", list(string(), 64)),
                field("undeterminable", ImprintType::Bool),
            ],
            vec![],
        ),
        closed(
            "aelio.compose_result",
            2,
            vec![
                field("tree", list(composition_node, 64)),
                field("seams", list(composition_seam.clone(), 1_024)),
                field("undeterminable", ImprintType::Bool),
            ],
            vec![],
        ),
        closed(
            "aelio.decompose_result",
            2,
            vec![
                field(
                    "verdict",
                    ImprintType::StrEnum {
                        values: ["children", "atomic", "cannot"]
                            .into_iter()
                            .map(str::to_owned)
                            .collect(),
                    },
                ),
                field(
                    "parent_complexity",
                    ImprintType::IntRange {
                        min: 0,
                        max: i64::MAX,
                    },
                ),
                field("children", list(ImprintType::Any, 64)),
                field("seams", list(composition_seam, 1_024)),
                field("child_complexities", ImprintType::Any),
                field("detail", ImprintType::StrBound { max_bytes: 4_096 }),
                field("undeterminable", ImprintType::Bool),
            ],
            vec![],
        ),
    ]
}
