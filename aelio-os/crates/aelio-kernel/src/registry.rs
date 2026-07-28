//! Call bridge & registry (§10). Targets are registered Compute/I/O/Model/Tool endpoints; the
//! kernel never grows domain nouns (§7). For P0 targets are in-process closures with a declared
//! effect class (§10.1); P1 replaces the Tool class with the SDK reverse channel (§31).

use crate::error::ErrV1;
use aelio_sol::SolValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectClass {
    Pure,
    Read,
    Write,
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetClass {
    Compute,
    Io,
    Model,
    Tool,
    Flow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Boundedness {
    CostEnvelope { max_units: u64 },
    DeadlineCompliant { max_ms: u64 },
    RegisteredFlow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Tenant,
    Vendor,
}

#[derive(Debug, Clone)]
pub struct Declaration {
    pub id: String,
    pub class: TargetClass,
    pub input_imprint: String,
    pub output_imprint: String,
    pub boundedness: Boundedness,
    pub effect_class: EffectClass,
    pub policy_tags: Vec<String>,
    pub tenant: String,
    pub origin: Origin,
}

impl Declaration {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() || !self.id.contains('@') {
            return Err("registry target id must be version-pinned (`id@version`)".into());
        }
        if self.input_imprint.trim().is_empty() || self.output_imprint.trim().is_empty() {
            return Err("registry Call boundaries require declared input/output imprints".into());
        }
        if self.tenant.trim().is_empty() {
            return Err("registry declaration requires tenant scope".into());
        }
        match self.boundedness {
            Boundedness::CostEnvelope { max_units: 0 }
            | Boundedness::DeadlineCompliant { max_ms: 0 } => {
                return Err("registry boundedness must be positive".into())
            }
            _ => {}
        }
        if self.effect_class.is_effectful() {
            if self.policy_tags.is_empty()
                || self.policy_tags.iter().any(|tag| tag.trim().is_empty())
            {
                return Err("write/external targets require non-empty policy tags (§10.2)".into());
            }
            if !matches!(self.boundedness, Boundedness::DeadlineCompliant { .. }) {
                return Err("write/external targets must be deadline-compliant (§8.4)".into());
            }
        }
        if self.class == TargetClass::Flow && self.boundedness != Boundedness::RegisteredFlow {
            return Err("flow targets require registered-flow boundedness".into());
        }
        Ok(())
    }
}

impl EffectClass {
    /// write/external Calls get intent→result accounting (§12.4); read/model are result-only.
    pub fn is_effectful(self) -> bool {
        matches!(self, EffectClass::Write | EffectClass::External)
    }
}

/// A registered target: `args` in (least-privilege projection), Sol out.
pub type TargetFn = Box<dyn FnMut(&SolValue) -> Result<Invocation, ErrV1>>;

#[derive(Debug, Clone, PartialEq)]
pub struct Invocation {
    pub output: SolValue,
    /// Input + output tokens reported by a Model provider; zero for non-Model targets.
    pub usage_tokens: u64,
}

pub struct Entry {
    pub effect_class: EffectClass,
    pub declaration: Option<Declaration>,
    pub func: TargetFn,
}

#[derive(Default)]
pub struct Registry {
    entries: std::collections::HashMap<String, Entry>,
    flow_edges: std::collections::HashMap<String, Vec<String>>,
}

impl Registry {
    pub fn register(
        &mut self,
        id: impl Into<String>,
        effect_class: EffectClass,
        mut func: impl FnMut(&SolValue) -> Result<SolValue, ErrV1> + 'static,
    ) {
        self.entries.insert(
            id.into(),
            Entry {
                effect_class,
                declaration: None,
                func: Box::new(move |args| {
                    func(args).map(|output| Invocation {
                        output,
                        usage_tokens: 0,
                    })
                }),
            },
        );
    }

    pub fn register_declared(
        &mut self,
        declaration: Declaration,
        mut func: impl FnMut(&SolValue) -> Result<SolValue, ErrV1> + 'static,
    ) -> Result<(), String> {
        declaration.validate()?;
        let id = declaration.id.clone();
        let is_flow = declaration.class == TargetClass::Flow;
        if self.entries.contains_key(&id) {
            return Err(format!("registry target `{id}` already exists"));
        }
        self.entries.insert(
            id.clone(),
            Entry {
                effect_class: declaration.effect_class,
                declaration: Some(declaration),
                func: Box::new(move |args| {
                    func(args).map(|output| Invocation {
                        output,
                        usage_tokens: 0,
                    })
                }),
            },
        );
        if is_flow {
            self.flow_edges.insert(id, Vec::new());
        }
        Ok(())
    }

    pub fn register_model_declared(
        &mut self,
        declaration: Declaration,
        func: impl FnMut(&SolValue) -> Result<Invocation, ErrV1> + 'static,
    ) -> Result<(), String> {
        if declaration.class != TargetClass::Model {
            return Err("model registration requires TargetClass::Model".into());
        }
        declaration.validate()?;
        let id = declaration.id.clone();
        if self.entries.contains_key(&id) {
            return Err(format!("registry target `{id}` already exists"));
        }
        self.entries.insert(
            id,
            Entry {
                effect_class: declaration.effect_class,
                declaration: Some(declaration),
                func: Box::new(func),
            },
        );
        Ok(())
    }

    /// Declares the pinned flow-to-flow call edges used by the termination proof. All endpoints
    /// must be registered Flow targets; cycles and cross-tenant edges are refused by
    /// [`Self::validate_call_graph`].
    pub fn set_flow_calls(&mut self, flow_id: &str, calls: Vec<String>) -> Result<(), String> {
        let declaration = self
            .declaration(flow_id)
            .ok_or_else(|| format!("unknown flow `{flow_id}`"))?;
        if declaration.class != TargetClass::Flow {
            return Err(format!("`{flow_id}` is not a Flow target"));
        }
        let mut distinct = std::collections::BTreeSet::new();
        for target in &calls {
            let target_decl = self
                .declaration(target)
                .ok_or_else(|| format!("flow edge targets unknown `{target}`"))?;
            if target_decl.class != TargetClass::Flow {
                return Err(format!("flow edge target `{target}` is not a Flow"));
            }
            if !distinct.insert(target.clone()) {
                return Err(format!("duplicate flow edge `{flow_id}` → `{target}`"));
            }
        }
        self.flow_edges.insert(flow_id.to_owned(), calls);
        Ok(())
    }

    pub fn validate_call_graph(&self, tenant: &str) -> Result<(), String> {
        for (source, targets) in &self.flow_edges {
            let source_decl = self
                .declaration(source)
                .ok_or_else(|| format!("flow graph source `{source}` is unregistered"))?;
            if source_decl.origin == Origin::Tenant && source_decl.tenant != tenant {
                continue;
            }
            for target in targets {
                let target_decl = self
                    .declaration(target)
                    .ok_or_else(|| format!("flow graph target `{target}` is unregistered"))?;
                if target_decl.origin == Origin::Tenant && target_decl.tenant != tenant {
                    return Err(format!(
                        "flow edge `{source}` → `{target}` crosses tenant boundary"
                    ));
                }
            }
        }

        fn visit(
            id: &str,
            edges: &std::collections::HashMap<String, Vec<String>>,
            visiting: &mut std::collections::HashSet<String>,
            visited: &mut std::collections::HashSet<String>,
        ) -> Result<(), String> {
            if visited.contains(id) {
                return Ok(());
            }
            if !visiting.insert(id.to_owned()) {
                return Err(format!("recursive flow cycle reaches `{id}`"));
            }
            if let Some(targets) = edges.get(id) {
                for target in targets {
                    visit(target, edges, visiting, visited)?;
                }
            }
            visiting.remove(id);
            visited.insert(id.to_owned());
            Ok(())
        }

        let mut visiting = std::collections::HashSet::new();
        let mut visited = std::collections::HashSet::new();
        for id in self.flow_edges.keys() {
            visit(id, &self.flow_edges, &mut visiting, &mut visited)?;
        }
        Ok(())
    }

    pub fn effect_of(&self, id: &str) -> Option<EffectClass> {
        self.entries.get(id).map(|e| e.effect_class)
    }

    pub fn class_of(&self, id: &str) -> Option<TargetClass> {
        self.declaration(id).map(|declaration| declaration.class)
    }

    pub fn call(&mut self, id: &str, args: &SolValue) -> Option<Result<SolValue, ErrV1>> {
        self.entries
            .get_mut(id)
            .map(|e| (e.func)(args).map(|result| result.output))
    }

    pub fn invoke(&mut self, id: &str, args: &SolValue) -> Option<Result<Invocation, ErrV1>> {
        self.entries.get_mut(id).map(|entry| (entry.func)(args))
    }

    pub fn contains(&self, id: &str) -> bool {
        self.entries.contains_key(id)
    }

    pub fn declaration(&self, id: &str) -> Option<&Declaration> {
        self.entries
            .get(id)
            .and_then(|entry| entry.declaration.as_ref())
    }
}
