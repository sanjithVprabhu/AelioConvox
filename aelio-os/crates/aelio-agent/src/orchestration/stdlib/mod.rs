//! First-party standard tools (`std.*`).
//!
//! Pure tools execute locally via `ops::pure` — no SDK round-trip.
//! Catalog: `docs/development/AELIO_STD_TOOL_CATALOG.md`

mod helpers;
mod invoke;
mod specs;

use crate::abilities::registry::Registry;
use crate::types::{AelioResult, Value};
use indexmap::IndexMap;

pub use specs::standard_tool_specs;

pub const STANDARD_PROVIDER: &str = "aelio-standard";

/// Register vendor standard tools into the tenant registry at boot.
pub fn register_standard_tools(registry: &mut Registry) {
    for tool in standard_tool_specs() {
        registry
            .register_tool(tool)
            .expect("first-party standard tools carry required ToolContracts");
    }
}

/// Returns true when `tool_id` is a registered standard tool.
pub fn is_standard_tool(tool_id: &str) -> bool {
    tool_id.starts_with("std.")
}

/// Execute a standard tool locally. Returns `None` if `tool_id` is not standard.
pub fn invoke_standard_tool(
    tool_id: &str,
    args: &IndexMap<String, Value>,
) -> Option<AelioResult<Value>> {
    if !is_standard_tool(tool_id) {
        return None;
    }
    Some(invoke::invoke_standard_tool_inner(tool_id, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sum_and_average_tools_work() {
        let values = Value::List(vec![
            Value::Int(10),
            Value::Int(20),
            Value::Int(30),
        ]);
        let mut args = IndexMap::new();
        args.insert("values".into(), values);
        let sum = invoke_standard_tool("std.math.sum", &args)
            .expect("std tool")
            .expect("ok");
        if let Value::Map(map) = sum {
            assert_eq!(map.get("total"), Some(&Value::Int(60)));
        } else {
            panic!("expected map");
        }
        let avg = invoke_standard_tool("std.math.average", &args)
            .expect("std tool")
            .expect("ok");
        if let Value::Map(map) = avg {
            match map.get("average") {
                Some(Value::Int(20)) => {}
                Some(Value::Float(f)) if (*f - 20.0).abs() < f64::EPSILON => {}
                other => panic!("unexpected average: {other:?}"),
            }
        } else {
            panic!("expected map");
        }
    }

    #[test]
    fn registry_includes_full_catalog() {
        let specs = standard_tool_specs();
        assert!(specs.iter().any(|t| t.id == "std.text.join"));
        assert!(specs.iter().any(|t| t.id == "std.data.filter_equals"));
        assert!(specs.iter().any(|t| t.id == "std.control.coalesce"));
    }
}
