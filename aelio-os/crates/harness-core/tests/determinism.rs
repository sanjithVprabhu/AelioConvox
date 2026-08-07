//! M1 exit criterion — pinned reference hashes (§2.9, A8).
//!
//! Provenance: workspace `blake3 = 1`, x86_64, and aelio-sol canonical §4.3.
//! A change to either hash is a `SystemVersion.serialiser` bump, never a fixture edit.

use aelio_sol::{canonical_string, value_hash, SolValue};
use harness_core::agg::{float_sum_reference_hash, mean_skip_null_int, AggOutcome};
use harness_core::numeric::Int;

#[test]
fn reexports_aelio_sol_canonical_hash() {
    let v = SolValue::Int(42);
    assert!(!value_hash(&v).is_empty());
}

#[test]
fn canonical_form_reference_hash() {
    let value = SolValue::Map(
        [
            ("b".to_owned(), SolValue::Int(2)),
            ("a".to_owned(), SolValue::List(vec![
                SolValue::Float(0.1 + 0.2),
                SolValue::Str("xy".into()),
            ])),
        ]
        .into_iter()
        .collect(),
    );
    let hash = value_hash(&value);
    assert_eq!(
        hash,
        "a66bcd6a407d14189842ba22dc2627c7fdc023acccd7836a8e9d3fe40f4f2f80"
    );
    assert_eq!(canonical_string(&value).contains("\"a\""), true);
}

#[test]
fn float_sum_reference_hash_pinned() {
    let hash = float_sum_reference_hash();
    assert_eq!(
        hash,
        "0140b77a15543ec6dd2a95205d3c3e2982e7cc673ae12245440c4c61561e1d4d"
    );
}

#[test]
fn serialiser_edge_sweep_a8() {
    // -0.0 / 0.0 equal after normalization
    let zero = SolValue::float(0.0).unwrap();
    let neg_zero = SolValue::float(-0.0).unwrap();
    assert_eq!(value_hash(&zero), value_hash(&neg_zero));

    // 0.1+0.2 vs 0.3 differ
    let approx = SolValue::Float(0.1 + 0.2);
    let exact = SolValue::Float(0.3);
    assert_ne!(value_hash(&approx), value_hash(&exact));

    // ["ab","c"] vs ["a","bc"] differ
    let ab_c = SolValue::list([
        SolValue::Str("ab".into()),
        SolValue::Str("c".into()),
    ]);
    let a_bc = SolValue::list([
        SolValue::Str("a".into()),
        SolValue::Str("bc".into()),
    ]);
    assert_ne!(value_hash(&ab_c), value_hash(&a_bc));

    // Int(1) vs Decimal-looking float differ — int/float distinction
    assert_ne!(value_hash(&SolValue::Int(1)), value_hash(&SolValue::Float(1.0)));

    // nested map key order invariant
    let map_a = SolValue::map([(
        "outer".to_string(),
        SolValue::map([
            ("z".to_string(), SolValue::Int(1)),
            ("a".to_string(), SolValue::Int(2)),
        ]),
    )]);
    let map_b = SolValue::map([(
        "outer".to_string(),
        SolValue::map([
            ("a".to_string(), SolValue::Int(2)),
            ("z".to_string(), SolValue::Int(1)),
        ]),
    )]);
    assert_eq!(value_hash(&map_a), value_hash(&map_b));
}

#[test]
fn repeated_evaluation_is_bit_identical() {
    let input = vec![Some(Int(10)), None, Some(Int(20)), Some(Int(30))];
    let first = mean_skip_null_int(&input).expect("mean");
    for _ in 0..100 {
        assert_eq!(mean_skip_null_int(&input).expect("mean"), first);
    }
    assert!(matches!(first, AggOutcome::Value(_)));
}

#[test]
fn mean_reference_is_stable() {
    let input = vec![Some(Int(10)), None, Some(Int(20)), Some(Int(30))];
    assert_eq!(
        mean_skip_null_int(&input).expect("mean"),
        AggOutcome::Value(harness_core::numeric::Decimal {
            mantissa: 20_000_000,
            scale: 6,
        })
    );
}

#[test]
fn map_hashing_is_stable_across_construction_paths() {
    let forward = SolValue::Map(
        (0..500)
            .map(|index| (format!("key-{index:03}"), SolValue::Int(index)))
            .collect(),
    );
    let reverse = SolValue::Map(
        (0..500)
            .rev()
            .map(|index| (format!("key-{index:03}"), SolValue::Int(index)))
            .collect(),
    );
    assert_eq!(value_hash(&forward), value_hash(&reverse));
}
