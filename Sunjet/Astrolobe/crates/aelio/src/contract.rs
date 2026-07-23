//! Uniform ability contract at every rung L0→L4.
//!
//! A caller cannot tell whether it invokes a primitive or a 100-step flow.

use crate::types::{CostClass, ReasonCode, Sensitivity, Substrate, TypeTag};
use serde::{Deserialize, Serialize};

/// Closed runtime schema used to type-check compositions before execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypeSchema {
    Any,
    Scalar {
        tag: TypeTag,
    },
    Optional {
        inner: Box<TypeSchema>,
    },
    List {
        items: Box<TypeSchema>,
        max_items: usize,
    },
    Record {
        fields: indexmap::IndexMap<String, FieldSchema>,
        allow_additional: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldSchema {
    pub schema: TypeSchema,
    pub required: bool,
}

impl TypeSchema {
    pub fn accepts_output_of(&self, producer: &TypeSchema) -> bool {
        match (self, producer) {
            // An unconstrained consumer can accept every producer. The inverse is unsafe:
            // an `Any` producer makes no promise that it can satisfy a typed consumer.
            (Self::Any, _) => true,
            (_, Self::Any) => false,
            (Self::Scalar { tag: expected }, Self::Scalar { tag: actual }) => expected == actual,
            (Self::Optional { inner }, other) => inner.accepts_output_of(other),
            (
                Self::List {
                    items: expected, ..
                },
                Self::List { items: actual, .. },
            ) => expected.accepts_output_of(actual),
            (
                Self::Record {
                    fields: expected, ..
                },
                Self::Record { fields: actual, .. },
            ) => expected.iter().all(|(name, field)| {
                !field.required
                    || actual
                        .get(name)
                        .is_some_and(|value| field.schema.accepts_output_of(&value.schema))
            }),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untyped_output_cannot_satisfy_typed_input() {
        let typed = TypeSchema::Scalar { tag: TypeTag::Str };
        assert!(!typed.accepts_output_of(&TypeSchema::Any));
        assert!(TypeSchema::Any.accepts_output_of(&typed));
    }
}

/// Predicate / invariant as a closed expression tree (see policy module for evaluation).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Predicate {
    True,
    False,
    Eq {
        path: String,
        value: serde_json::Value,
    },
    Ne {
        path: String,
        value: serde_json::Value,
    },
    In {
        path: String,
        values: Vec<serde_json::Value>,
    },
    Gt {
        path: String,
        value: serde_json::Value,
    },
    Gte {
        path: String,
        value: serde_json::Value,
    },
    Lt {
        path: String,
        value: serde_json::Value,
    },
    Lte {
        path: String,
        value: serde_json::Value,
    },
    Present {
        path: String,
    },
    Absent {
        path: String,
    },
    And {
        of: Vec<Predicate>,
    },
    Or {
        of: Vec<Predicate>,
    },
    Not {
        of: Box<Predicate>,
    },
}

impl Predicate {
    pub fn always() -> Self {
        Predicate::True
    }

    pub fn state_eq(state_id: impl Into<String>) -> Self {
        Predicate::Eq {
            path: "state".into(),
            value: serde_json::Value::String(state_id.into()),
        }
    }
}

/// The uniform contract shared by pure ops, abilities, blocks, procedures, and flows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbilityContract {
    pub id: String,
    pub version: String,
    pub input: TypeSchema,
    pub output: TypeSchema,
    pub substrate: Substrate,
    pub cost_class: CostClass,
    pub idempotent: bool,
    pub effectful: bool,
    pub suspendable: bool,
    pub preconditions: Vec<Predicate>,
    pub postconditions: Vec<Predicate>,
    pub tool_deps: Vec<String>,
    pub prompt_hash: Option<String>,
    pub sensitivity: Sensitivity,
    /// Closed set of failure reason codes this ability may return.
    pub fail: Vec<ReasonCode>,
    /// Optional semantic retrieval handle for learned procedures.
    pub situation_key: Option<Vec<f32>>,
}

impl AbilityContract {
    pub fn pure(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            version: "1".into(),
            input: TypeSchema::Any,
            output: TypeSchema::Any,
            substrate: Substrate::Pure,
            cost_class: CostClass::Free,
            idempotent: true,
            effectful: false,
            suspendable: false,
            preconditions: vec![],
            postconditions: vec![],
            tool_deps: vec![],
            prompt_hash: None,
            sensitivity: Sensitivity::None,
            fail: vec![],
            situation_key: None,
        }
    }

    pub fn effect(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            version: "1".into(),
            input: TypeSchema::Any,
            output: TypeSchema::Any,
            substrate: Substrate::Effect,
            cost_class: CostClass::Moderate,
            idempotent: false,
            effectful: true,
            suspendable: false,
            preconditions: vec![],
            postconditions: vec![],
            tool_deps: vec![],
            prompt_hash: None,
            sensitivity: Sensitivity::None,
            fail: vec![ReasonCode::ToolError],
            situation_key: None,
        }
    }

    pub fn with_tool_deps(mut self, deps: Vec<String>) -> Self {
        self.tool_deps = deps;
        self
    }

    pub fn with_postconditions(mut self, posts: Vec<Predicate>) -> Self {
        self.postconditions = posts;
        self
    }

    pub fn with_preconditions(mut self, pres: Vec<Predicate>) -> Self {
        self.preconditions = pres;
        self
    }

    pub fn suspendable(mut self) -> Self {
        self.suspendable = true;
        self
    }

    pub fn idempotent(mut self) -> Self {
        self.idempotent = true;
        self
    }

    pub fn typed(mut self, input: TypeSchema, output: TypeSchema) -> Self {
        self.input = input;
        self.output = output;
        self
    }
}

/// Ordered path over declared abilities — the only thing Learn.ProposePath may emit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbilityPath {
    pub steps: Vec<PathStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathStep {
    pub ability_id: String,
    pub args: IndexMapLite,
}

/// Thin ordered map for path args without pulling IndexMap into every call site.
pub type IndexMapLite = indexmap::IndexMap<String, serde_json::Value>;

impl AbilityPath {
    pub fn seq(ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            steps: ids
                .into_iter()
                .map(|id| PathStep {
                    ability_id: id.into(),
                    args: indexmap::IndexMap::new(),
                })
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}
