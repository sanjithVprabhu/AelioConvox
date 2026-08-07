use harness_core::versioning::{
    assert_effect_set_bounds, authorize, harness_op_catalog, impl_hash, op_catalog_merkle_root,
    system_version_from_ops, system_version_with_harness_catalog, EffectSet, Hash, SystemVersion,
    Verified,
};
use std::collections::BTreeMap;

fn zero_hash() -> Hash {
    [0_u8; 32]
}

#[test]
fn system_version_hash_is_exhaustive() {
    let version = SystemVersion {
        op_catalog: [1; 32],
        dialect: [2; 32],
        renderer: [3; 32],
        tzdb: [4; 32],
        serialiser: [5; 32],
        intent_schema: [6; 32],
        extractor_model: [7; 32],
        embedding_model: [8; 32],
        triage_model: [9; 32],
    };
    let hash = version.hash();
    assert_ne!(hash, zero_hash());

    let mut perturbations = vec![
        ("op_catalog", version.clone()),
        ("dialect", version.clone()),
        ("renderer", version.clone()),
        ("tzdb", version.clone()),
        ("serialiser", version.clone()),
        ("intent_schema", version.clone()),
        ("extractor_model", version.clone()),
        ("embedding_model", version.clone()),
        ("triage_model", version.clone()),
    ];
    perturbations[0].1.op_catalog[0] ^= 0xff;
    perturbations[1].1.dialect[0] ^= 0xff;
    perturbations[2].1.renderer[0] ^= 0xff;
    perturbations[3].1.tzdb[0] ^= 0xff;
    perturbations[4].1.serialiser[0] ^= 0xff;
    perturbations[5].1.intent_schema[0] ^= 0xff;
    perturbations[6].1.extractor_model[0] ^= 0xff;
    perturbations[7].1.embedding_model[0] ^= 0xff;
    perturbations[8].1.triage_model[0] ^= 0xff;

    for (field, changed) in perturbations {
        assert_ne!(hash, changed.hash(), "{field} must affect SystemVersion");
    }
}

#[test]
fn impl_hash_and_merkle_root_change_with_source() {
    let mut ops = BTreeMap::new();
    ops.insert("sum", impl_hash("fn sum() {}"));
    ops.insert("mean_skip_null", impl_hash("fn mean_skip_null() {}"));
    let root_a = op_catalog_merkle_root(&ops);
    ops.insert("mean_skip_null", impl_hash("fn mean_skip_null() { fixed }"));
    let root_b = op_catalog_merkle_root(&ops);
    assert_ne!(root_a, root_b);
}

#[test]
fn verified_load_rejects_hash_mismatch() {
    let expected = impl_hash("effect-set-v1");
    let actual = impl_hash("effect-set-v2");
    assert!(Verified::load(expected, vec!["tool_a"], actual).is_err());
    let verified = Verified::load(expected, vec!["tool_a"], expected).unwrap();
    assert_eq!(verified.inner(), &vec!["tool_a"]);
}

#[test]
fn system_version_from_ops_builds_catalog_merkle() {
    let ops = BTreeMap::from([
        ("count_rows", "count rows impl"),
        ("count_values", "count values impl"),
    ]);
    let version = system_version_from_ops(
        &ops, [1; 32], [2; 32], [3; 32], [4; 32], [5; 32], [6; 32], [7; 32], [8; 32],
    );
    assert_ne!(version.op_catalog, zero_hash());
    assert_ne!(version.hash(), zero_hash());
}

#[test]
fn op_catalog_hash_is_order_independent_of_insertion() {
    let mut forward = BTreeMap::new();
    forward.insert("alpha", impl_hash("alpha implementation"));
    forward.insert("beta", impl_hash("beta implementation"));

    let mut reverse = BTreeMap::new();
    reverse.insert("beta", impl_hash("beta implementation"));
    reverse.insert("alpha", impl_hash("alpha implementation"));

    assert_eq!(
        op_catalog_merkle_root(&forward),
        op_catalog_merkle_root(&reverse)
    );
}

#[test]
fn ast_exceeding_the_verified_effect_set_is_rejected() {
    let allowed = EffectSet::from_effects(["tool.read_profile"]);
    let verified = allowed.clone().verify(allowed.hash()).unwrap();

    assert_effect_set_bounds(&verified, ["tool.read_profile"]).unwrap();
    let err =
        assert_effect_set_bounds(&verified, ["tool.read_profile", "tool.delete_user"]).unwrap_err();
    assert!(err.missing.contains("tool.delete_user"));

    let stale = EffectSet::from_effects(["tool.read_profile", "tool.delete_user"]);
    assert!(stale.verify(allowed.hash()).is_err());
}

#[test]
fn native_catalog_merkle_is_wired_into_system_version() {
    let catalog = harness_op_catalog();
    assert!(catalog.contains_key("numeric.add"));
    assert!(catalog.contains_key("agg.mean_skip_null_decimal"));
    assert!(catalog.contains_key("time.calendar_bucket"));

    let version = system_version_with_harness_catalog(
        [1; 32], [2; 32], [3; 32], [4; 32], [5; 32], [6; 32], [7; 32],
    );
    assert_eq!(version.op_catalog, op_catalog_merkle_root(&catalog));
    assert_ne!(version.tzdb, zero_hash());
}

#[test]
fn authorization_accepts_only_verified_effect_sets() {
    let grants = EffectSet::from_effects(["tool.read_profile"]);
    let verified = grants.clone().verify(grants.hash()).unwrap();
    authorize(&verified, "tool.read_profile").unwrap();
    assert_eq!(
        authorize(&verified, "tool.delete_user").unwrap_err().effect,
        "tool.delete_user"
    );
}
