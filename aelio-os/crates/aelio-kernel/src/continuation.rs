//! Durable, tamper-evident Park continuation envelope (Appendix I).

use crate::driver::Parked;
use crate::error::ErrV1;
use crate::exec::Frame;
use crate::instr::Until;
use aelio_sol::{structural_imprint, value_hash, SolValue};
use std::collections::{BTreeMap, VecDeque};

pub const CONT_FORMAT: i64 = 1;
pub const KERNEL_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SOL_VERSION: &str = "1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationPins {
    pub kernel_version: String,
    pub sol_version: String,
    pub flow_id: String,
    pub flow_rev: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationIdentity {
    pub tenant: String,
    pub flow_instance_id: String,
}

#[derive(Debug)]
pub struct DecodedContinuation {
    pub parked: Parked,
    pub pins: ContinuationPins,
    pub identity: ContinuationIdentity,
    pub event_key: Option<String>,
}

/// Mints an unguessable callback token. Identifiers provide domain separation; the deployment
/// secret provides authorization entropy and must never be logged or stored in the envelope.
pub fn mint_event_key(
    secret: &[u8; 32],
    tenant: &str,
    flow_instance_id: &str,
    park_nid: &str,
) -> String {
    let material = SolValue::list([
        SolValue::str("aelio.event.v1"),
        SolValue::str(tenant),
        SolValue::str(flow_instance_id),
        SolValue::str(park_nid),
    ]);
    blake3::keyed_hash(secret, &aelio_sol::canonical_bytes(&material))
        .to_hex()
        .to_string()
}

impl Parked {
    pub fn to_continuation(
        &self,
        pins: &ContinuationPins,
        identity: &ContinuationIdentity,
    ) -> Result<SolValue, String> {
        if self.frames.len() != self.frame_nids.len() {
            return Err("continuation frame/nid length mismatch".into());
        }
        validate_metadata(pins, identity)?;
        let frames = self
            .frames
            .iter()
            .zip(&self.frame_nids)
            .map(|(frame, nid)| frame_to_sol(nid, frame))
            .collect::<Result<Vec<_>, _>>()?;
        let mut park = vec![
            ("park_nid", SolValue::str(self.park_nid.clone())),
            ("until", until_to_sol(&self.until)?),
        ];
        if let Some(key) = self.event_key.as_deref() {
            if key.is_empty() {
                return Err("event_key must not be empty".into());
            }
            park.push(("event_key", SolValue::str(key)));
        }
        let unsigned = SolValue::map([
            ("cont_format", SolValue::Int(CONT_FORMAT)),
            (
                "pins",
                SolValue::map([
                    ("kernel_version", SolValue::str(pins.kernel_version.clone())),
                    ("sol_version", SolValue::str(pins.sol_version.clone())),
                    ("flow_id", SolValue::str(pins.flow_id.clone())),
                    ("flow_rev", SolValue::str(pins.flow_rev.clone())),
                ]),
            ),
            (
                "identity",
                SolValue::map([
                    ("tenant", SolValue::str(identity.tenant.clone())),
                    (
                        "flow_instance_id",
                        SolValue::str(identity.flow_instance_id.clone()),
                    ),
                ]),
            ),
            ("park", SolValue::map(park)),
            ("frames", SolValue::List(frames)),
            (
                "bag",
                SolValue::map([
                    ("imprint", SolValue::str(structural_imprint(&self.bag))),
                    ("body", self.bag.clone()),
                ]),
            ),
        ]);
        let hash = value_hash(&unsigned);
        let mut envelope = unsigned
            .as_map()
            .cloned()
            .ok_or("continuation envelope must be a map")?;
        envelope.insert("continuation_hash".into(), SolValue::str(hash));
        Ok(SolValue::Map(envelope))
    }

    pub fn from_continuation(
        value: &SolValue,
        expected_kernel_version: &str,
    ) -> Result<DecodedContinuation, String> {
        let envelope = map(value, "continuation")?;
        exact(
            envelope,
            &[
                "cont_format",
                "pins",
                "identity",
                "park",
                "frames",
                "bag",
                "continuation_hash",
            ],
            "continuation",
        )?;
        let claimed_hash = string(envelope, "continuation_hash")?;
        let mut unsigned = envelope.clone();
        unsigned.remove("continuation_hash");
        if value_hash(&SolValue::Map(unsigned)) != claimed_hash {
            return Err("continuation hash mismatch".into());
        }
        if integer(envelope, "cont_format")? != CONT_FORMAT {
            return Err("unsupported continuation format".into());
        }

        let pins_map = nested(envelope, "pins")?;
        exact(
            pins_map,
            &["kernel_version", "sol_version", "flow_id", "flow_rev"],
            "continuation.pins",
        )?;
        let pins = ContinuationPins {
            kernel_version: string(pins_map, "kernel_version")?,
            sol_version: string(pins_map, "sol_version")?,
            flow_id: string(pins_map, "flow_id")?,
            flow_rev: string(pins_map, "flow_rev")?,
        };
        if pins.kernel_version != expected_kernel_version {
            return Err(format!(
                "continuation kernel version {} does not match {}",
                pins.kernel_version, expected_kernel_version
            ));
        }
        if pins.sol_version != SOL_VERSION {
            return Err(format!(
                "unsupported continuation sol version {}",
                pins.sol_version
            ));
        }

        let identity_map = nested(envelope, "identity")?;
        exact(
            identity_map,
            &["tenant", "flow_instance_id"],
            "continuation.identity",
        )?;
        let identity = ContinuationIdentity {
            tenant: string(identity_map, "tenant")?,
            flow_instance_id: string(identity_map, "flow_instance_id")?,
        };
        validate_metadata(&pins, &identity)?;

        let park_map = nested(envelope, "park")?;
        exact_optional(
            park_map,
            &["park_nid", "until"],
            &["event_key"],
            "continuation.park",
        )?;
        let park_nid = string(park_map, "park_nid")?;
        let until = until_from_sol(
            park_map
                .get("until")
                .ok_or("continuation.park.until missing")?,
        )?;
        let event_key = optional_string(park_map, "event_key")?;

        let bag_map = nested(envelope, "bag")?;
        exact(bag_map, &["imprint", "body"], "continuation.bag")?;
        let bag = bag_map
            .get("body")
            .cloned()
            .ok_or("continuation.bag.body missing")?;
        if string(bag_map, "imprint")? != structural_imprint(&bag) {
            return Err("continuation structural imprint mismatch".into());
        }

        let frame_values = list(envelope, "frames")?;
        let mut frames = VecDeque::with_capacity(frame_values.len());
        let mut frame_nids = VecDeque::with_capacity(frame_values.len());
        for value in frame_values {
            let (nid, frame) = frame_from_sol(value)?;
            frame_nids.push_back(nid);
            frames.push_back(frame);
        }
        Ok(DecodedContinuation {
            parked: Parked {
                bag,
                frames,
                frame_nids,
                park_nid,
                until,
                event_key: event_key.clone(),
            },
            pins,
            identity,
            event_key,
        })
    }
}

fn frame_to_sol(nid: &str, frame: &Frame) -> Result<SolValue, String> {
    let mut fields = vec![("nid", SolValue::str(nid))];
    match frame {
        Frame::Seq(index) => {
            fields.push(("frame", SolValue::str("Seq")));
            fields.push(("step_index", uint(*index as u64)?));
        }
        Frame::Branch(arm) => {
            fields.push(("frame", SolValue::str("Branch")));
            fields.push(("arm", SolValue::Bool(*arm)));
        }
        Frame::Body => fields.push(("frame", SolValue::str("Body"))),
        Frame::Let(saved) => {
            fields.push(("frame", SolValue::str("Let")));
            fields.push((
                "shadowed",
                SolValue::List(
                    saved
                        .iter()
                        .map(|(key, value)| {
                            SolValue::map([
                                ("key", SolValue::str(key)),
                                ("present", SolValue::Bool(value.is_some())),
                                ("value", value.clone().unwrap_or(SolValue::Null)),
                            ])
                        })
                        .collect(),
                ),
            ));
        }
        Frame::Loop(iter) => {
            fields.push(("frame", SolValue::str("Loop")));
            fields.push(("iter_count", uint(*iter)?));
        }
        Frame::TryBody => {
            fields.push(("frame", SolValue::str("Try")));
            fields.push(("phase", SolValue::str("body")));
        }
        Frame::TryHandler(index) => {
            fields.push(("frame", SolValue::str("Try")));
            fields.push(("phase", SolValue::str("handler")));
            fields.push(("handler_index", uint(*index as u64)?));
        }
        Frame::TryFinally(error) => {
            fields.push(("frame", SolValue::str("Try")));
            fields.push(("phase", SolValue::str("finally")));
            fields.push((
                "pending_error",
                error.as_ref().map(ErrV1::to_sol).unwrap_or(SolValue::Null),
            ));
        }
        Frame::Fallback {
            step_index,
            prior_errors,
        } => {
            fields.push(("frame", SolValue::str("Fallback")));
            fields.push(("step_index", uint(*step_index as u64)?));
            fields.push((
                "prior_errs",
                SolValue::List(prior_errors.iter().map(ErrV1::to_sol).collect()),
            ));
        }
        Frame::Budget {
            used_calls,
            used_tokens,
            used_ms,
        } => {
            fields.push(("frame", SolValue::str("Budget")));
            fields.push(("used_calls", uint(*used_calls)?));
            fields.push(("used_tokens", uint(*used_tokens)?));
            fields.push(("used_ms", uint(*used_ms)?));
        }
        Frame::Timeout { used_ms } => {
            fields.push(("frame", SolValue::str("Timeout")));
            fields.push(("used_ms", uint(*used_ms)?));
        }
        Frame::GuardHandler => {
            fields.push(("frame", SolValue::str("Guard")));
            fields.push(("phase", SolValue::str("handler")));
        }
        Frame::Map {
            element_index,
            collected,
            parent_bag,
            parent_scope,
        } => {
            fields.push(("frame", SolValue::str("Map")));
            fields.push(("element_index", uint(*element_index as u64)?));
            fields.push(("collected", SolValue::List(collected.clone())));
            fields.push(("parent_bag", parent_bag.clone()));
            fields.push(("parent_scope", parent_scope.clone()));
        }
    }
    Ok(SolValue::map(fields))
}

fn frame_from_sol(value: &SolValue) -> Result<(String, Frame), String> {
    let map = map(value, "frame")?;
    let nid = string(map, "nid")?;
    let kind = string(map, "frame")?;
    let frame = match kind.as_str() {
        "Seq" => Frame::Seq(usize_value(map, "step_index")?),
        "Branch" => Frame::Branch(bool_value(map, "arm")?),
        "Body" => Frame::Body,
        "Let" => {
            let values = list(map, "shadowed")?;
            let mut saved = Vec::with_capacity(values.len());
            for value in values {
                let item = self_map(value, "Let.shadowed")?;
                exact(item, &["key", "present", "value"], "Let.shadowed")?;
                let present = bool_value(item, "present")?;
                saved.push((
                    string(item, "key")?,
                    present.then(|| item.get("value").cloned().unwrap_or(SolValue::Null)),
                ));
            }
            Frame::Let(saved)
        }
        "Loop" => Frame::Loop(u64_value(map, "iter_count")?),
        "Try" => match string(map, "phase")?.as_str() {
            "body" => Frame::TryBody,
            "handler" => Frame::TryHandler(usize_value(map, "handler_index")?),
            "finally" => {
                let error = match map.get("pending_error") {
                    Some(SolValue::Null) => None,
                    Some(value) => Some(ErrV1::from_sol(value).ok_or("invalid pending err.v1")?),
                    None => return Err("Try.finally pending_error missing".into()),
                };
                Frame::TryFinally(error)
            }
            _ => return Err("invalid Try phase".into()),
        },
        "Fallback" => Frame::Fallback {
            step_index: usize_value(map, "step_index")?,
            prior_errors: list(map, "prior_errs")?
                .iter()
                .map(|value| ErrV1::from_sol(value).ok_or("invalid prior err.v1".into()))
                .collect::<Result<Vec<_>, String>>()?,
        },
        "Budget" => Frame::Budget {
            used_calls: u64_value(map, "used_calls")?,
            used_tokens: u64_value(map, "used_tokens")?,
            used_ms: u64_value(map, "used_ms")?,
        },
        "Timeout" => Frame::Timeout {
            used_ms: u64_value(map, "used_ms")?,
        },
        "Guard" if string(map, "phase")? == "handler" => Frame::GuardHandler,
        "Map" => Frame::Map {
            element_index: usize_value(map, "element_index")?,
            collected: list(map, "collected")?.to_vec(),
            parent_bag: map.get("parent_bag").cloned().ok_or("Map.parent_bag")?,
            parent_scope: map.get("parent_scope").cloned().ok_or("Map.parent_scope")?,
        },
        _ => return Err(format!("unknown continuation frame `{kind}`")),
    };
    Ok((nid, frame))
}

fn until_to_sol(until: &Until) -> Result<SolValue, String> {
    Ok(match until {
        Until::Event => SolValue::map([("kind", SolValue::str("event"))]),
        Until::Ttl { ms } => SolValue::map([("kind", SolValue::str("ttl")), ("ms", uint(*ms)?)]),
        Until::Instant { at } => SolValue::map([
            ("kind", SolValue::str("instant")),
            ("at", SolValue::str(at)),
        ]),
    })
}

fn until_from_sol(value: &SolValue) -> Result<Until, String> {
    let map = self_map(value, "until")?;
    match string(map, "kind")?.as_str() {
        "event" => Ok(Until::Event),
        "ttl" => Ok(Until::Ttl {
            ms: u64_value(map, "ms")?,
        }),
        "instant" => Ok(Until::Instant {
            at: string(map, "at")?,
        }),
        _ => Err("unknown Park.until kind".into()),
    }
}

fn validate_metadata(
    pins: &ContinuationPins,
    identity: &ContinuationIdentity,
) -> Result<(), String> {
    if [
        pins.kernel_version.as_str(),
        pins.sol_version.as_str(),
        pins.flow_id.as_str(),
        pins.flow_rev.as_str(),
        identity.tenant.as_str(),
        identity.flow_instance_id.as_str(),
    ]
    .contains(&"")
    {
        return Err("continuation pins and identity must be non-empty".into());
    }
    Ok(())
}

fn uint(value: u64) -> Result<SolValue, String> {
    i64::try_from(value)
        .map(SolValue::Int)
        .map_err(|_| "continuation integer exceeds i64".into())
}

fn map<'a>(value: &'a SolValue, label: &str) -> Result<&'a BTreeMap<String, SolValue>, String> {
    value
        .as_map()
        .ok_or_else(|| format!("{label} must be a map"))
}

fn self_map<'a>(
    value: &'a SolValue,
    label: &str,
) -> Result<&'a BTreeMap<String, SolValue>, String> {
    map(value, label)
}

fn nested<'a>(
    map: &'a BTreeMap<String, SolValue>,
    key: &str,
) -> Result<&'a BTreeMap<String, SolValue>, String> {
    self_map(
        map.get(key)
            .ok_or_else(|| format!("continuation.{key} missing"))?,
        key,
    )
}

fn exact(map: &BTreeMap<String, SolValue>, required: &[&str], label: &str) -> Result<(), String> {
    exact_optional(map, required, &[], label)
}

fn exact_optional(
    map: &BTreeMap<String, SolValue>,
    required: &[&str],
    optional: &[&str],
    label: &str,
) -> Result<(), String> {
    if let Some(key) = required.iter().find(|key| !map.contains_key(**key)) {
        return Err(format!("{label}.{key} missing"));
    }
    if let Some(key) = map
        .keys()
        .find(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(format!("unknown {label} field `{key}`"));
    }
    Ok(())
}

fn string(map: &BTreeMap<String, SolValue>, key: &str) -> Result<String, String> {
    match map.get(key) {
        Some(SolValue::Str(value)) if !value.is_empty() => Ok(value.clone()),
        _ => Err(format!("`{key}` must be a non-empty string")),
    }
}

fn optional_string(map: &BTreeMap<String, SolValue>, key: &str) -> Result<Option<String>, String> {
    map.get(key).map(|_| string(map, key)).transpose()
}

fn integer(map: &BTreeMap<String, SolValue>, key: &str) -> Result<i64, String> {
    match map.get(key) {
        Some(SolValue::Int(value)) => Ok(*value),
        _ => Err(format!("`{key}` must be an int")),
    }
}

fn u64_value(map: &BTreeMap<String, SolValue>, key: &str) -> Result<u64, String> {
    u64::try_from(integer(map, key)?).map_err(|_| format!("`{key}` must be non-negative"))
}

fn usize_value(map: &BTreeMap<String, SolValue>, key: &str) -> Result<usize, String> {
    usize::try_from(u64_value(map, key)?).map_err(|_| format!("`{key}` exceeds usize"))
}

fn bool_value(map: &BTreeMap<String, SolValue>, key: &str) -> Result<bool, String> {
    match map.get(key) {
        Some(SolValue::Bool(value)) => Ok(*value),
        _ => Err(format!("`{key}` must be bool")),
    }
}

fn list<'a>(map: &'a BTreeMap<String, SolValue>, key: &str) -> Result<&'a [SolValue], String> {
    match map.get(key) {
        Some(SolValue::List(values)) => Ok(values),
        _ => Err(format!("`{key}` must be a list")),
    }
}
