//! Namespaced pure Call targets for the P0 standard library slice.
//!
//! These are thin, declared wrappers over [`crate::compute::apply`]. They give
//! harness programs stable `math.*` / `collection.*` / `logic.*` Call ids that
//! pin in contracts (plan §5.3–5.5) without expanding the kernel opcode set.

use crate::compute;
use crate::error::{ErrV1, ReasonCode};
use crate::registry::{Boundedness, Declaration, EffectClass, Origin, Registry, TargetClass};
use aelio_sol::SolValue;

fn pure_decl(id: &str, input: &str, output: &str) -> Declaration {
    Declaration {
        id: id.into(),
        class: TargetClass::Compute,
        input_imprint: input.into(),
        output_imprint: output.into(),
        boundedness: Boundedness::CostEnvelope { max_units: 1 },
        effect_class: EffectClass::Pure,
        policy_tags: vec![],
        tenant: "vendor".into(),
        origin: Origin::Vendor,
    }
}

fn map_field<'a>(args: &'a SolValue, key: &str) -> Result<&'a SolValue, ErrV1> {
    args.as_map()
        .and_then(|m| m.get(key))
        .ok_or_else(|| {
            ErrV1::new(
                ReasonCode::Type,
                key,
                format!("expected map field `{key}`"),
            )
        })
}

fn compute_err(target: &str, err: compute::ComputeErr) -> ErrV1 {
    ErrV1::new(err.code, target, err.detail)
}

/// Register P0 pure stdlib Call targets used by `workflow.average` and related seeds.
pub fn register_p0_pure_stdlib(registry: &mut Registry) -> Result<(), String> {
    // collection.count@1 — { list: [...] } → int
    registry.register_declared(
        pure_decl(
            "collection.count@1",
            "aelio.collection.list@1",
            "aelio.math.int@1",
        ),
        |args| {
            let list = map_field(args, "list")?;
            compute::apply("count", std::slice::from_ref(list))
                .map_err(|e| compute_err("collection.count@1", e))
        },
    )?;

    // math.sum@1 — { list: [numbers] } → number
    registry.register_declared(
        pure_decl("math.sum@1", "aelio.math.number_list@1", "aelio.math.number@1"),
        |args| {
            let list = map_field(args, "list")?;
            compute::apply("sum", std::slice::from_ref(list))
                .map_err(|e| compute_err("math.sum@1", e))
        },
    )?;

    // math.divide@1 — { a, b } → number
    registry.register_declared(
        pure_decl("math.divide@1", "aelio.math.number_pair@1", "aelio.math.number@1"),
        |args| {
            let a = map_field(args, "a")?.clone();
            let b = map_field(args, "b")?.clone();
            compute::apply("div", &[a, b]).map_err(|e| compute_err("math.divide@1", e))
        },
    )?;

    // math.add@1 — { a, b } → number
    registry.register_declared(
        pure_decl("math.add@1", "aelio.math.number_pair@1", "aelio.math.number@1"),
        |args| {
            let a = map_field(args, "a")?.clone();
            let b = map_field(args, "b")?.clone();
            compute::apply("add", &[a, b]).map_err(|e| compute_err("math.add@1", e))
        },
    )?;

    // logic.gt@1 — { a, b } → bool (for Guard-like call sites if needed)
    registry.register_declared(
        pure_decl("logic.gt@1", "aelio.math.number_pair@1", "aelio.logic.bool@1"),
        |args| {
            let a = map_field(args, "a")?.clone();
            let b = map_field(args, "b")?.clone();
            compute::apply("gt", &[a, b]).map_err(|e| compute_err("logic.gt@1", e))
        },
    )?;

    // logic.eq@1
    registry.register_declared(
        pure_decl("logic.eq@1", "aelio.any.pair@1", "aelio.logic.bool@1"),
        |args| {
            let a = map_field(args, "a")?.clone();
            let b = map_field(args, "b")?.clone();
            compute::apply("eq", &[a, b]).map_err(|e| compute_err("logic.eq@1", e))
        },
    )?;

    // math.multiply@1 / math.add already above; add subtract + abs
    registry.register_declared(
        pure_decl(
            "math.multiply@1",
            "aelio.math.number_pair@1",
            "aelio.math.number@1",
        ),
        |args| {
            let a = map_field(args, "a")?.clone();
            let b = map_field(args, "b")?.clone();
            compute::apply("mul", &[a, b]).map_err(|e| compute_err("math.multiply@1", e))
        },
    )?;
    registry.register_declared(
        pure_decl(
            "math.subtract@1",
            "aelio.math.number_pair@1",
            "aelio.math.number@1",
        ),
        |args| {
            let a = map_field(args, "a")?.clone();
            let b = map_field(args, "b")?.clone();
            compute::apply("sub", &[a, b]).map_err(|e| compute_err("math.subtract@1", e))
        },
    )?;
    registry.register_declared(
        pure_decl("math.abs@1", "aelio.math.number@1", "aelio.math.number@1"),
        |args| {
            let a = map_field(args, "value")
                .or_else(|_| map_field(args, "a"))?
                .clone();
            compute::apply("abs", &[a]).map_err(|e| compute_err("math.abs@1", e))
        },
    )?;

    // text.length@1 / text.concat@1
    registry.register_declared(
        pure_decl("text.length@1", "aelio.text.string@1", "aelio.math.int@1"),
        |args| {
            let s = map_field(args, "text")
                .or_else(|_| map_field(args, "value"))?
                .clone();
            compute::apply("length", &[s]).map_err(|e| compute_err("text.length@1", e))
        },
    )?;
    registry.register_declared(
        pure_decl("text.concat@1", "aelio.text.string_list@1", "aelio.text.string@1"),
        |args| {
            let list = map_field(args, "parts")?;
            match list {
                SolValue::List(parts) => {
                    compute::apply("concat", parts).map_err(|e| compute_err("text.concat@1", e))
                }
                _ => Err(ErrV1::new(
                    ReasonCode::Type,
                    "text.concat@1",
                    "parts must be a list of strings",
                )),
            }
        },
    )?;

    register_p0_binary_pure(registry)?;

    Ok(())
}

/// Remaining compute-backed pure targets (plan §5.3–5.4 slice completion).
///
/// Every entry here is a thin binary wrapper over an existing [`compute::apply`] fn, so the
/// kernel opcode set is untouched. Nondeterministic families (`time.now`, `id.uuid`) are
/// deliberately absent: they are ledgered nondet and must not be declared `Pure`.
fn register_p0_binary_pure(registry: &mut Registry) -> Result<(), String> {
    // (call id, compute fn, input imprint, output imprint)
    const BINARY_NUMERIC: &[(&str, &str)] = &[
        ("math.modulo@1", "mod"),
        ("math.min@1", "min"),
        ("math.max@1", "max"),
    ];
    for (id, op) in BINARY_NUMERIC {
        let op = *op;
        let id_owned: &'static str = id;
        registry.register_declared(
            pure_decl(id, "aelio.math.number_pair@1", "aelio.math.number@1"),
            move |args| {
                let a = map_field(args, "a")?;
                let b = map_field(args, "b")?;
                compute::apply(op, &[a.clone(), b.clone()])
                    .map_err(|e| compute_err(id_owned, e))
            },
        )?;
    }

    const BINARY_PREDICATE: &[(&str, &str)] = &[
        ("logic.not_equals@1", "ne"),
        ("logic.lt@1", "lt"),
        ("logic.lte@1", "le"),
        ("logic.gte@1", "ge"),
        ("logic.and@1", "and"),
        ("logic.or@1", "or"),
    ];
    for (id, op) in BINARY_PREDICATE {
        let op = *op;
        let id_owned: &'static str = id;
        registry.register_declared(
            pure_decl(id, "aelio.logic.pair@1", "aelio.logic.bool@1"),
            move |args| {
                let a = map_field(args, "a")?;
                let b = map_field(args, "b")?;
                compute::apply(op, &[a.clone(), b.clone()])
                    .map_err(|e| compute_err(id_owned, e))
            },
        )?;
    }

    // logic.not@1 — unary
    registry.register_declared(
        pure_decl("logic.not@1", "aelio.logic.bool@1", "aelio.logic.bool@1"),
        |args| {
            let v = map_field(args, "value")?;
            compute::apply("not", std::slice::from_ref(v))
                .map_err(|e| compute_err("logic.not@1", e))
        },
    )?;

    Ok(())
}

/// Leaf demo registry + P0 pure stdlib targets.
pub fn registry_with_p0_stdlib() -> Registry {
    let mut r = crate::sol_harness_lib::leaf_call_registry();
    register_p0_pure_stdlib(&mut r).expect("P0 pure stdlib declarations must validate");
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use aelio_sol::SolValue;

    #[test]
    fn sum_count_divide_via_stdlib_calls() {
        let mut r = registry_with_p0_stdlib();
        let list = SolValue::map([(
            "list",
            SolValue::List(vec![SolValue::Int(2), SolValue::Int(4), SolValue::Int(6)]),
        )]);
        let sum = r
            .call("math.sum@1", &list)
            .expect("registered")
            .expect("ok");
        assert_eq!(sum, SolValue::Int(12));
        let count = r
            .call("collection.count@1", &list)
            .expect("registered")
            .expect("ok");
        assert_eq!(count, SolValue::Int(3));
        let avg_args = SolValue::map([("a", sum), ("b", count)]);
        let avg = r
            .call("math.divide@1", &avg_args)
            .expect("registered")
            .expect("ok");
        assert_eq!(avg, SolValue::Int(4));
    }

    #[test]
    fn binary_pure_slice_is_registered_and_evaluates() {
        let mut r = registry_with_p0_stdlib();
        let pair = |a: i64, b: i64| SolValue::map([("a", SolValue::Int(a)), ("b", SolValue::Int(b))]);

        assert_eq!(
            r.call("math.modulo@1", &pair(7, 4)).expect("registered").expect("ok"),
            SolValue::Int(3)
        );
        assert_eq!(
            r.call("math.min@1", &pair(7, 4)).expect("registered").expect("ok"),
            SolValue::Int(4)
        );
        assert_eq!(
            r.call("math.max@1", &pair(7, 4)).expect("registered").expect("ok"),
            SolValue::Int(7)
        );
        assert_eq!(
            r.call("logic.lt@1", &pair(4, 7)).expect("registered").expect("ok"),
            SolValue::Bool(true)
        );
        assert_eq!(
            r.call("logic.gte@1", &pair(4, 7)).expect("registered").expect("ok"),
            SolValue::Bool(false)
        );
        assert_eq!(
            r.call("logic.not_equals@1", &pair(4, 7)).expect("registered").expect("ok"),
            SolValue::Bool(true)
        );

        let not_args = SolValue::map([("value", SolValue::Bool(false))]);
        assert_eq!(
            r.call("logic.not@1", &not_args).expect("registered").expect("ok"),
            SolValue::Bool(true)
        );
    }

    #[test]
    fn nondeterministic_families_are_not_declared_pure() {
        let r = registry_with_p0_stdlib();
        for id in ["time.now@1", "id.uuid@1", "math.random_bounded@1"] {
            assert!(
                !r.contains(id),
                "{id} is ledgered nondet and must not ship as a Pure stdlib target"
            );
        }
    }
}
