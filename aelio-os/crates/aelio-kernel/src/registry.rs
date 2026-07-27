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

impl EffectClass {
    /// write/external Calls get intent→result accounting (§12.4); read/model are result-only.
    pub fn is_effectful(self) -> bool {
        matches!(self, EffectClass::Write | EffectClass::External)
    }
}

/// A registered target: `args` in (least-privilege projection), Sol out.
pub type TargetFn = Box<dyn FnMut(&SolValue) -> Result<SolValue, ErrV1>>;

pub struct Entry {
    pub effect_class: EffectClass,
    pub func: TargetFn,
}

#[derive(Default)]
pub struct Registry {
    entries: std::collections::HashMap<String, Entry>,
}

impl Registry {
    pub fn register(
        &mut self,
        id: impl Into<String>,
        effect_class: EffectClass,
        func: impl FnMut(&SolValue) -> Result<SolValue, ErrV1> + 'static,
    ) {
        self.entries.insert(
            id.into(),
            Entry {
                effect_class,
                func: Box::new(func),
            },
        );
    }

    pub fn effect_of(&self, id: &str) -> Option<EffectClass> {
        self.entries.get(id).map(|e| e.effect_class)
    }

    pub fn call(&mut self, id: &str, args: &SolValue) -> Option<Result<SolValue, ErrV1>> {
        self.entries.get_mut(id).map(|e| (e.func)(args))
    }

    pub fn contains(&self, id: &str) -> bool {
        self.entries.contains_key(id)
    }
}
