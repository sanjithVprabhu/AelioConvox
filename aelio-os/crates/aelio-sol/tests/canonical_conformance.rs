//! Canonical-hash conformance (§4.3, §27) — the P0 step-1 exit criteria as executable vectors.
//! Every assertion cites the section whose normative claim it pins.

use aelio_sol::{
    canonical_string, structural_imprint, value_hash, Limits, Path, Segment, SolError, SolValue,
};
use std::collections::BTreeMap;

// ── §4.3: int `2` ≢ float `2.0` — the distinction JSON doesn't make ───────────────────────────
#[test]
fn int_two_is_not_float_two() {
    let int2 = SolValue::Int(2);
    let float2 = SolValue::float(2.0).unwrap();
    assert_eq!(canonical_string(&int2), "2");
    assert_eq!(canonical_string(&float2), "2.0");
    assert_ne!(
        value_hash(&int2),
        value_hash(&float2),
        "§4.3 int/float distinction"
    );
}

#[test]
fn floats_always_carry_a_decimal_point() {
    // §4.3: shortest round-trip, always a decimal point.
    assert_eq!(canonical_string(&SolValue::float(0.0).unwrap()), "0.0");
    assert_eq!(canonical_string(&SolValue::float(1.0).unwrap()), "1.0");
    assert_eq!(canonical_string(&SolValue::float(1.5).unwrap()), "1.5");
    // -0.0 normalized to 0.0 at construction.
    assert_eq!(canonical_string(&SolValue::float(-0.0).unwrap()), "0.0");
}

// ── §4.3: NaN / ±∞ are forbidden in Sol data — unconstructible ────────────────────────────────
#[test]
fn nan_and_infinities_are_rejected_at_construction() {
    assert_eq!(SolValue::float(f64::NAN), Err(SolError::NonFinite));
    assert_eq!(SolValue::float(f64::INFINITY), Err(SolError::NonFinite));
    assert_eq!(SolValue::float(f64::NEG_INFINITY), Err(SolError::NonFinite));
}

// ── §4.3: canonical hashing stable + key-order independent ────────────────────────────────────
#[test]
fn map_hash_is_key_order_independent() {
    // Two logically-equal maps built in different insertion orders hash identically (BTreeMap sorts).
    let a = SolValue::map([
        ("b", SolValue::Int(2)),
        ("a", SolValue::Int(1)),
        ("c", SolValue::Int(3)),
    ]);
    let mut raw = BTreeMap::new();
    raw.insert("c".to_string(), SolValue::Int(3));
    raw.insert("a".to_string(), SolValue::Int(1));
    raw.insert("b".to_string(), SolValue::Int(2));
    let b = SolValue::Map(raw);
    assert_eq!(
        canonical_string(&a),
        r#"{"a":1,"b":2,"c":3}"#,
        "§4.3 sorted keys, no whitespace"
    );
    assert_eq!(
        value_hash(&a),
        value_hash(&b),
        "§4.3 canonical hash stable across build order"
    );
}

#[test]
fn canonical_hash_round_trips_identically() {
    // §27 replay-determinism seed: hashing the same value twice yields the same digest.
    let v = SolValue::map([
        ("name", SolValue::str("Acme")),
        (
            "scores",
            SolValue::list([
                SolValue::float(0.94).unwrap(),
                SolValue::float(0.88).unwrap(),
            ]),
        ),
        ("active", SolValue::Bool(true)),
        ("note", SolValue::Null),
    ]);
    assert_eq!(value_hash(&v), value_hash(&v.clone()));
}

// ── §4.3: strings — no Unicode normalization, minimal escapes ─────────────────────────────────
#[test]
fn strings_are_not_unicode_normalized() {
    // U+00E9 (é precomposed) vs U+0065 U+0301 (e + combining acute) are different byte strings and
    // must hash differently — normalizing would silently alter user data (§4.3).
    let precomposed = SolValue::Str("\u{00E9}".into());
    let decomposed = SolValue::Str("e\u{0301}".into());
    assert_ne!(
        value_hash(&precomposed),
        value_hash(&decomposed),
        "§4.3 no normalization"
    );
}

#[test]
fn string_minimal_escapes() {
    assert_eq!(
        canonical_string(&SolValue::str("a\"b\\c\n")),
        r#""a\"b\\c\n""#
    );
}

// ── §4.1.3: structural imprint = shape hash, content-independent ──────────────────────────────
#[test]
fn structural_imprint_is_shape_not_content() {
    let one = SolValue::map([("id", SolValue::str("acme")), ("n", SolValue::Int(1))]);
    let two = SolValue::map([("id", SolValue::str("globex")), ("n", SolValue::Int(999))]);
    // Same shape, different content → same structural imprint.
    assert_eq!(
        structural_imprint(&one),
        structural_imprint(&two),
        "§4.1.3 shape imprint"
    );
    // Different shape → different imprint.
    let three = SolValue::map([
        ("id", SolValue::str("x")),
        ("n", SolValue::float(1.0).unwrap()),
    ]);
    assert_ne!(
        structural_imprint(&one),
        structural_imprint(&three),
        "int≢float in shape too"
    );
    // Imprints are written `~<hash>` (§4.1.1).
    assert!(structural_imprint(&one).starts_with('~'));
}

#[test]
fn homogeneous_vs_mixed_lists_have_distinct_shapes() {
    let homo = SolValue::list([SolValue::Int(1), SolValue::Int(2)]);
    let mixed = SolValue::list([SolValue::Int(1), SolValue::str("two")]);
    assert_eq!(aelio_sol::shape_signature(&homo), "list<int>");
    assert_eq!(aelio_sol::shape_signature(&mixed), "list<mixed>");
    assert_eq!(
        aelio_sol::shape_signature(&SolValue::List(vec![])),
        "list<never>"
    );
}

// ── §4.4: limits ──────────────────────────────────────────────────────────────────────────────
#[test]
fn limits_reject_oversized_structures() {
    let limits = Limits::default();
    // A list at the cap is fine; over the cap is rejected.
    let ok = SolValue::List(vec![SolValue::Int(0); 10_000]);
    assert!(limits.check(&ok).is_ok());
    let too_long = SolValue::List(vec![SolValue::Int(0); 10_001]);
    assert!(matches!(limits.check(&too_long), Err(SolError::Limit(_))));
    // Depth over 32 rejected.
    let mut deep = SolValue::Int(0);
    for _ in 0..40 {
        deep = SolValue::list([deep]);
    }
    assert!(matches!(limits.check(&deep), Err(SolError::Limit(_))));
}

// ── §6.1: path grammar ────────────────────────────────────────────────────────────────────────
#[test]
fn path_grammar_positive_examples() {
    // ≥10 positive cases incl. quoted-bracket keys and index forms (F2 completion check seed).
    for p in [
        "a",
        "a.b",
        "a.b.c",
        "a.b[0]",
        "a[0].b",
        "a[0][1]",
        r#"["user-id"]"#,
        r#"["user-id"].x"#,
        r#"a.["weird key"].y"#,
        "_x9.y_2",
        r#"root.["a.b.c"]"#,
    ] {
        assert!(Path::parse(p).is_ok(), "should parse: {p}");
    }
}

#[test]
fn path_grammar_negative_examples() {
    // ≥10 negative cases: empty, computed-ish, root index, bad tokens, unterminated.
    for p in [
        "",          // empty
        "[0]",       // first element cannot be an index (root is a map body)
        "a..b",      // empty segment
        "a.",        // trailing dot
        "1abc",      // identifier can't start with a digit
        "a[b]",      // non-integer index
        "a[-1]",     // negative index
        r#"a["k"]"#, // quoted key must follow '.'
        r#"a.["k"#,  // unterminated quoted key
        "a b",       // space is not a valid token
        "a[]",       // empty index
    ] {
        assert!(Path::parse(p).is_err(), "should reject: {p}");
    }
}

#[test]
fn path_get_and_exists() {
    let bag = SolValue::map([
        ("user", SolValue::map([("phone", SolValue::str("+91..."))])),
        (
            "items",
            SolValue::list([SolValue::Int(10), SolValue::Int(20)]),
        ),
        (r#"weird key"#, SolValue::Bool(true)),
    ]);
    assert_eq!(
        Path::parse("user.phone").unwrap().get(&bag),
        Some(&SolValue::str("+91..."))
    );
    assert_eq!(
        Path::parse("items[1]").unwrap().get(&bag),
        Some(&SolValue::Int(20))
    );
    assert!(Path::parse("items[9]").unwrap().get(&bag).is_none()); // out of range → None (§6.4)
    assert!(!Path::parse("user.missing").unwrap().exists(&bag));
    assert_eq!(
        Path::parse(r#"["weird key"]"#).unwrap().get(&bag),
        Some(&SolValue::Bool(true))
    );
    // Segment structure sanity.
    assert_eq!(
        Path::parse("a[0]").unwrap().segments(),
        &[Segment::Key("a".into()), Segment::Index(0)]
    );
}

// ── §4.3 depth-32 path boundary (F2 completion) ───────────────────────────────────────────────
#[test]
fn path_depth_32_boundary() {
    let at_limit = std::iter::repeat("a")
        .take(32)
        .collect::<Vec<_>>()
        .join(".");
    assert!(Path::parse(&at_limit).is_ok(), "32 segments allowed");
    let over_limit = std::iter::repeat("a")
        .take(33)
        .collect::<Vec<_>>()
        .join(".");
    assert!(
        Path::parse(&over_limit).is_err(),
        "33 segments rejected (§4.4 depth 32)"
    );
}
