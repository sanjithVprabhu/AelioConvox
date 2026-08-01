//! CAS-backed builder coordination: exclusive name leases and immutable verification results.

use super::{
    decode, encode, ensure_repository_schema, now_ms, store_error, validate_hash, validate_id,
    validate_tenant, ArtifactError,
};
use aelio_store::{PutIfAbsent, Store, StoreError};
use serde::{Deserialize, Serialize};

pub const NAME_LEASE_TABLE: &str = "name_leases";
pub const VERIFICATION_CACHE_TABLE: &str = "verification_cache";
pub const COORDINATION_SCHEMA_VERSION: u32 = 1;
const MAX_LEASE_MS: i64 = 86_400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseStatus {
    Active,
    Released,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseRecord {
    pub name: String,
    pub interface_hash: String,
    pub spec_hash: String,
    pub build_id: String,
    pub acquired_at_ms: i64,
    pub expires_at_ms: i64,
    pub status: LeaseStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseRequest<'a> {
    pub name: &'a str,
    pub interface_hash: &'a str,
    pub spec_hash: &'a str,
    pub build_id: &'a str,
    pub ttl_ms: i64,
}

#[derive(Clone)]
pub struct NameLeaseRepository<S: Store + Clone> {
    store: S,
}

impl<S: Store + Clone> NameLeaseRepository<S> {
    pub fn open(mut store: S) -> Result<Self, ArtifactError> {
        ensure_repository_schema(&mut store, NAME_LEASE_TABLE, COORDINATION_SCHEMA_VERSION)?;
        Ok(Self { store })
    }

    pub fn acquire(
        &mut self,
        tenant: &str,
        request: LeaseRequest<'_>,
    ) -> Result<LeaseRecord, ArtifactError> {
        self.acquire_at(tenant, request, now_ms()?)
    }

    pub fn acquire_at(
        &mut self,
        tenant: &str,
        request: LeaseRequest<'_>,
        at_ms: i64,
    ) -> Result<LeaseRecord, ArtifactError> {
        validate_tenant(tenant)?;
        validate_id(request.name)?;
        validate_hash("interface_hash", request.interface_hash)?;
        validate_hash("spec_hash", request.spec_hash)?;
        validate_build_id(request.build_id)?;
        if !(1..=MAX_LEASE_MS).contains(&request.ttl_ms) {
            return Err(ArtifactError::Invalid(format!(
                "lease ttl_ms must be within 1..={MAX_LEASE_MS}"
            )));
        }
        let expires_at_ms = at_ms
            .checked_add(request.ttl_ms)
            .ok_or_else(|| ArtifactError::Invalid("lease expiration overflow".into()))?;
        let record = LeaseRecord {
            name: request.name.into(),
            interface_hash: request.interface_hash.into(),
            spec_hash: request.spec_hash.into(),
            build_id: request.build_id.into(),
            acquired_at_ms: at_ms,
            expires_at_ms,
            status: LeaseStatus::Active,
        };
        let key = lease_key(request.name, request.interface_hash);
        match self
            .store
            .put_if_absent(tenant, NAME_LEASE_TABLE, &key, encode(&record)?)
            .map_err(store_error)?
        {
            PutIfAbsent::Inserted { .. } => Ok(record),
            PutIfAbsent::Existing(existing) => {
                let current: LeaseRecord = decode(&existing.value)?;
                if current.status == LeaseStatus::Active
                    && current.expires_at_ms > at_ms
                    && current.spec_hash == request.spec_hash
                    && current.build_id == request.build_id
                {
                    return Ok(current);
                }
                if current.status == LeaseStatus::Active && current.expires_at_ms > at_ms {
                    return Err(ArtifactError::Conflict(format!(
                        "name `{}` is leased by build `{}`",
                        request.name, current.build_id
                    )));
                }
                self.store
                    .cas(
                        tenant,
                        NAME_LEASE_TABLE,
                        &key,
                        existing.version,
                        encode(&record)?,
                    )
                    .map_err(cas_error)?;
                Ok(record)
            }
        }
    }

    pub fn release(
        &mut self,
        tenant: &str,
        name: &str,
        interface_hash: &str,
        build_id: &str,
    ) -> Result<LeaseRecord, ArtifactError> {
        validate_tenant(tenant)?;
        validate_id(name)?;
        validate_hash("interface_hash", interface_hash)?;
        validate_build_id(build_id)?;
        let key = lease_key(name, interface_hash);
        let existing = self
            .store
            .get(tenant, NAME_LEASE_TABLE, &key)
            .map_err(store_error)?
            .ok_or_else(|| ArtifactError::NotFound(key.clone()))?;
        let mut record: LeaseRecord = decode(&existing.value)?;
        if record.build_id != build_id {
            return Err(ArtifactError::Conflict(
                "only the owning build may release a name lease".into(),
            ));
        }
        if record.status == LeaseStatus::Released {
            return Ok(record);
        }
        record.status = LeaseStatus::Released;
        self.store
            .cas(
                tenant,
                NAME_LEASE_TABLE,
                &key,
                existing.version,
                encode(&record)?,
            )
            .map_err(cas_error)?;
        Ok(record)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationVerdict {
    Pass,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationCacheEntry {
    pub candidate_hash: String,
    pub examples_hash: String,
    pub pin_context_hash: String,
    pub verdict: VerificationVerdict,
    pub ledger_ref: String,
}

#[derive(Clone)]
pub struct VerificationCacheRepository<S: Store + Clone> {
    store: S,
}

impl<S: Store + Clone> VerificationCacheRepository<S> {
    pub fn open(mut store: S) -> Result<Self, ArtifactError> {
        ensure_repository_schema(
            &mut store,
            VERIFICATION_CACHE_TABLE,
            COORDINATION_SCHEMA_VERSION,
        )?;
        Ok(Self { store })
    }

    pub fn put(
        &mut self,
        tenant: &str,
        entry: VerificationCacheEntry,
    ) -> Result<VerificationCacheEntry, ArtifactError> {
        validate_tenant(tenant)?;
        validate_cache_entry(&entry)?;
        let key = cache_key(&entry);
        match self
            .store
            .put_if_absent(tenant, VERIFICATION_CACHE_TABLE, &key, encode(&entry)?)
            .map_err(store_error)?
        {
            PutIfAbsent::Inserted { .. } => Ok(entry),
            PutIfAbsent::Existing(existing) => {
                let current: VerificationCacheEntry = decode(&existing.value)?;
                if current == entry {
                    Ok(current)
                } else {
                    Err(ArtifactError::Conflict(
                        "verification cache key already has a different immutable verdict".into(),
                    ))
                }
            }
        }
    }

    pub fn get(
        &self,
        tenant: &str,
        candidate_hash: &str,
        examples_hash: &str,
        pin_context_hash: &str,
    ) -> Result<Option<VerificationCacheEntry>, ArtifactError> {
        validate_tenant(tenant)?;
        let probe = VerificationCacheEntry {
            candidate_hash: candidate_hash.into(),
            examples_hash: examples_hash.into(),
            pin_context_hash: pin_context_hash.into(),
            verdict: VerificationVerdict::Fail,
            ledger_ref: "probe".into(),
        };
        validate_cache_entry(&probe)?;
        self.store
            .get(tenant, VERIFICATION_CACHE_TABLE, &cache_key(&probe))
            .map_err(store_error)?
            .map(|row| decode(&row.value))
            .transpose()
    }
}

fn validate_cache_entry(entry: &VerificationCacheEntry) -> Result<(), ArtifactError> {
    validate_hash("candidate_hash", &entry.candidate_hash)?;
    validate_hash("examples_hash", &entry.examples_hash)?;
    validate_hash("pin_context_hash", &entry.pin_context_hash)?;
    if entry.ledger_ref.is_empty() || entry.ledger_ref.len() > 512 {
        return Err(ArtifactError::Invalid("ledger_ref is invalid".into()));
    }
    Ok(())
}

fn lease_key(name: &str, interface_hash: &str) -> String {
    format!("{name}|{interface_hash}")
}

fn cache_key(entry: &VerificationCacheEntry) -> String {
    format!(
        "{}|{}|{}",
        entry.candidate_hash, entry.examples_hash, entry.pin_context_hash
    )
}

fn validate_build_id(build_id: &str) -> Result<(), ArtifactError> {
    if build_id.is_empty()
        || build_id.len() > 192
        || !build_id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'_' || byte == b'-'
        })
    {
        Err(ArtifactError::Invalid("build_id is invalid".into()))
    } else {
        Ok(())
    }
}

fn cas_error(error: StoreError) -> ArtifactError {
    match error {
        StoreError::Conflict => ArtifactError::Conflict("compare-and-swap lost".into()),
        other => store_error(other),
    }
}
