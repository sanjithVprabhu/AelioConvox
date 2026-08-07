//! HKv4 versioning — `SystemVersion`, op `impl_hash`, Merkle catalog root (§17.1–17.3).

use std::collections::{BTreeMap, BTreeSet};

/// Content hash — BLAKE3-256.
pub type Hash = [u8; 32];

/// Every mutable input to a harness/skeleton hash (§17.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemVersion {
    pub op_catalog: Hash,
    pub dialect: Hash,
    pub renderer: Hash,
    pub tzdb: Hash,
    pub serialiser: Hash,
    pub intent_schema: Hash,
    pub extractor_model: Hash,
    pub embedding_model: Hash,
    pub triage_model: Hash,
}

impl SystemVersion {
    /// Hash all fields — exhaustive destructuring fails to compile if a field is added
    /// without being included here (§17.1).
    pub fn hash(&self) -> Hash {
        let SystemVersion {
            op_catalog,
            dialect,
            renderer,
            tzdb,
            serialiser,
            intent_schema,
            extractor_model,
            embedding_model,
            triage_model,
        } = self;

        let mut hasher = blake3::Hasher::new();
        for field in [
            op_catalog,
            dialect,
            renderer,
            tzdb,
            serialiser,
            intent_schema,
            extractor_model,
            embedding_model,
            triage_model,
        ] {
            hasher.update(field);
        }
        *hasher.finalize().as_bytes()
    }
}

/// Per-op implementation hash over source (§17.3).
pub fn impl_hash(source: &str) -> Hash {
    *blake3::hash(source.as_bytes()).as_bytes()
}

/// Merkle root over per-op `impl_hash` values, keyed by op name (§17.3).
pub fn op_catalog_merkle_root(ops: &BTreeMap<&str, Hash>) -> Hash {
    let mut hasher = blake3::Hasher::new();
    for (name, hash) in ops {
        // Delimit names so catalogs such as {ab, c} and {a, bc} cannot share
        // the same concatenated byte stream.
        hasher.update(&(name.len() as u64).to_be_bytes());
        hasher.update(name.as_bytes());
        hasher.update(hash);
    }
    *hasher.finalize().as_bytes()
}

/// Per-operation semantic revisions. A behavioral change must bump its entry;
/// the catalog Merkle root then invalidates all cached harnesses (§17.3).
///
/// These revisions intentionally describe operations, not Rust files: moving a
/// function is not semantic drift, while changing its behavior is.
const HARNESS_OP_IMPL_REVS: &[(&str, &str)] = &[
    ("agg.count_rows", "1"),
    ("agg.count_values", "1"),
    ("agg.div_int_or_empty", "1"),
    ("agg.mean_skip_null_decimal", "1"),
    ("agg.mean_skip_null_float", "1"),
    ("agg.mean_skip_null_int", "1"),
    ("agg.mean_strict_decimal", "1"),
    ("agg.mean_strict_float", "1"),
    ("agg.mean_strict_int", "1"),
    ("agg.mean_zero_null_decimal", "1"),
    ("agg.mean_zero_null_float", "1"),
    ("agg.mean_zero_null_int", "1"),
    ("agg.sum_decimal", "1"),
    ("agg.sum_float", "1"),
    ("agg.sum_int", "1"),
    ("agg.sum_num", "1"),
    ("exec.execute_effect", "1"),
    ("exec.verify_replay", "1"),
    ("numeric.add", "1"),
    ("numeric.div", "1"),
    ("numeric.division_scale", "1"),
    ("numeric.int.div_floor", "1"),
    ("numeric.mul", "1"),
    ("numeric.sub", "1"),
    ("taint.check_prohibited", "1"),
    ("taint.declassify_count", "1"),
    ("taint.join_taint", "1"),
    ("taint.propagate_taint", "1"),
    ("time.calendar_bucket", "1"),
    ("time.temporal_binding.resolve", "1"),
];

/// The in-process harness op catalog, deterministically keyed by operation
/// name. The BTreeMap provides a canonical Merkle leaf order.
pub fn harness_op_catalog() -> BTreeMap<&'static str, Hash> {
    HARNESS_OP_IMPL_REVS
        .iter()
        .map(|(name, revision)| (*name, impl_hash(revision)))
        .collect()
}

/// Build a `SystemVersion` from component hashes plus an op catalog map.
pub fn system_version_from_ops(
    ops: &BTreeMap<&str, &str>,
    dialect: Hash,
    renderer: Hash,
    tzdb: Hash,
    serialiser: Hash,
    intent_schema: Hash,
    extractor_model: Hash,
    embedding_model: Hash,
    triage_model: Hash,
) -> SystemVersion {
    let impl_hashes = ops
        .iter()
        .map(|(name, source)| (*name, impl_hash(source)))
        .collect::<BTreeMap<_, _>>();
    let op_catalog = op_catalog_merkle_root(
        &impl_hashes
            .iter()
            .map(|(name, hash)| (*name, *hash))
            .collect(),
    );
    SystemVersion {
        op_catalog,
        dialect,
        renderer,
        tzdb,
        serialiser,
        intent_schema,
        extractor_model,
        embedding_model,
        triage_model,
    }
}

/// Build a SystemVersion using the complete native harness op catalog and the
/// exact shipped tzdb dependency.
pub fn system_version_with_harness_catalog(
    dialect: Hash,
    renderer: Hash,
    serialiser: Hash,
    intent_schema: Hash,
    extractor_model: Hash,
    embedding_model: Hash,
    triage_model: Hash,
) -> SystemVersion {
    let op_catalog = op_catalog_merkle_root(&harness_op_catalog());
    SystemVersion {
        op_catalog,
        dialect,
        renderer,
        tzdb: crate::time::tzdb_hash(),
        serialiser,
        intent_schema,
        extractor_model,
        embedding_model,
        triage_model,
    }
}

/// Authority wrapper — constructible only after hash verification (§17.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified<T> {
    inner: T,
    verified_hash: Hash,
}

impl<T> Verified<T> {
    pub fn load(expected: Hash, value: T, actual: Hash) -> Result<Self, VerifyError> {
        if expected != actual {
            return Err(VerifyError { expected, actual });
        }
        Ok(Self {
            inner: value,
            verified_hash: actual,
        })
    }

    pub fn inner(&self) -> &T {
        &self.inner
    }

    pub fn verified_hash(&self) -> Hash {
        self.verified_hash
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyError {
    pub expected: Hash,
    pub actual: Hash,
}

/// The promoted, content-addressed set of tools an AST may directly reference.
///
/// It is intentionally a sorted set: insertion order is not execution
/// semantics and must not affect its content address.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EffectSet {
    effects: BTreeSet<String>,
}

impl EffectSet {
    pub fn from_effects<I, S>(effects: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            effects: effects.into_iter().map(Into::into).collect(),
        }
    }

    pub fn contains(&self, effect: &str) -> bool {
        self.effects.contains(effect)
    }

    pub fn hash(&self) -> Hash {
        let mut hasher = blake3::Hasher::new();
        for effect in &self.effects {
            hasher.update(&(effect.len() as u64).to_be_bytes());
            hasher.update(effect.as_bytes());
        }
        *hasher.finalize().as_bytes()
    }

    /// Load a promoted effect set only when its own content hash matches.
    pub fn verify(self, expected: Hash) -> Result<Verified<Self>, VerifyError> {
        let actual = self.hash();
        Verified::load(expected, self, actual)
    }
}

/// Failure when a requested effect is absent from the verified grant set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationError {
    pub effect: String,
}

/// Authorize an effect only through a content-verified grant set (§17.2).
///
/// Taking `Verified<EffectSet>` is intentional structural closure: callers
/// cannot accidentally treat a parsed or stale raw effect set as authority.
pub fn authorize(grants: &Verified<EffectSet>, effect: &str) -> Result<(), AuthorizationError> {
    if grants.inner().contains(effect) {
        Ok(())
    } else {
        Err(AuthorizationError {
            effect: effect.to_owned(),
        })
    }
}

/// Failure when the AST reaches an effect absent from its verified cache entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AstExceedsEffectSet {
    pub missing: BTreeSet<String>,
}

/// Validate direct AST references against the verified promoted effect set.
///
/// The cache hash alone proves byte integrity. This second check catches a
/// valid but stale/incomplete cached set before authorization or dispatch.
pub fn assert_effect_set_bounds<I, S>(
    allowed: &Verified<EffectSet>,
    direct_effects: I,
) -> Result<(), AstExceedsEffectSet>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let missing: BTreeSet<String> = direct_effects
        .into_iter()
        .map(|effect| effect.as_ref().to_owned())
        .filter(|effect| !allowed.inner().contains(effect))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(AstExceedsEffectSet { missing })
    }
}
