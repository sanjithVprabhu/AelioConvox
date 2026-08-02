//! Closed, versioned App H wire frames. The transport supplies WebSocket binary messages or gRPC
//! byte streams; this crate owns the canonical payload and length prefix.

use aelio_sol::{Limits, SolValue};
use serde_json::Value as Json;
use std::collections::BTreeMap;

pub const PROTOCOL_MAJOR: u64 = 1;
pub const PROTOCOL_MINOR: u64 = 0;
pub const MAX_FRAME_BYTES: usize = 1_048_576;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Protocol {
    pub major: u64,
    pub minor: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClientFrame {
    Hello {
        protocol: Protocol,
        sdk_version: String,
        deployer_id: String,
        auth: String,
        last_corr_acked: Option<String>,
    },
    ToolResult {
        corr: String,
        outcome: ToolOutcome,
        duration_ms: u64,
    },
    Ack {
        corr: String,
    },
    Ping {
        t: u64,
    },
    Pong {
        t: u64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToolOutcome {
    Ok(SolValue),
    Err(SolValue),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectClass {
    Pure,
    Read,
    Write,
    External,
}

impl EffectClass {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "pure" => Some(Self::Pure),
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            "external" => Some(Self::External),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerFrame {
    HelloAck {
        protocol: Protocol,
        conn_id: String,
        heartbeat_ms: u64,
        pending_tool_calls: u64,
    },
    HelloReject {
        reason: String,
        supported_majors: Vec<u64>,
    },
    ToolCall {
        corr: String,
        target: String,
        effect_class: String,
        args: SolValue,
        deadline_ms: u64,
        idem_key: Option<String>,
        turn_ref: String,
    },
    Ack {
        corr: String,
    },
    Ping {
        t: u64,
    },
    Pong {
        t: u64,
    },
}

pub fn parse_client_payload(bytes: &[u8]) -> Result<ClientFrame, String> {
    if bytes.len() > MAX_FRAME_BYTES {
        return Err("wire frame exceeds 1 MiB".into());
    }
    let json: Json = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let object = json.as_object().ok_or("wire frame must be an object")?;
    let frame = string(object, "frame")?;
    match frame.as_str() {
        "hello" => {
            closed(
                object,
                &[
                    "frame",
                    "protocol",
                    "sdk_version",
                    "deployer_id",
                    "auth",
                    "resume",
                ],
            )?;
            let protocol = parse_protocol(object.get("protocol").ok_or("hello.protocol")?)?;
            let last_corr_acked = match object.get("resume") {
                None => None,
                Some(resume) => {
                    let resume = resume.as_object().ok_or("hello.resume must be an object")?;
                    closed(resume, &["last_corr_acked"])?;
                    Some(string(resume, "last_corr_acked")?)
                }
            };
            Ok(ClientFrame::Hello {
                protocol,
                sdk_version: nonempty(string(object, "sdk_version")?, "sdk_version")?,
                deployer_id: nonempty(string(object, "deployer_id")?, "deployer_id")?,
                auth: nonempty(string(object, "auth")?, "auth")?,
                last_corr_acked,
            })
        }
        "tool_result" => {
            closed(
                object,
                &["frame", "corr", "outcome", "output", "err", "duration_ms"],
            )?;
            let outcome = match string(object, "outcome")?.as_str() {
                "ok" => {
                    if object.contains_key("err") {
                        return Err("successful tool_result cannot carry err".into());
                    }
                    ToolOutcome::Ok(sol(object.get("output").ok_or("tool_result.output")?)?)
                }
                "err" => {
                    if object.contains_key("output") {
                        return Err("failed tool_result cannot carry output".into());
                    }
                    ToolOutcome::Err(sol(object.get("err").ok_or("tool_result.err")?)?)
                }
                _ => return Err("tool_result.outcome must be ok|err".into()),
            };
            Ok(ClientFrame::ToolResult {
                corr: nonempty(string(object, "corr")?, "corr")?,
                outcome,
                duration_ms: positive_u64(object, "duration_ms")?,
            })
        }
        "ack" => {
            closed(object, &["frame", "corr"])?;
            Ok(ClientFrame::Ack {
                corr: nonempty(string(object, "corr")?, "corr")?,
            })
        }
        "ping" | "pong" => {
            closed(object, &["frame", "t"])?;
            let t = object.get("t").and_then(Json::as_u64).ok_or("frame.t")?;
            if frame == "ping" {
                Ok(ClientFrame::Ping { t })
            } else {
                Ok(ClientFrame::Pong { t })
            }
        }
        _ => Err(format!("unknown client frame `{frame}`")),
    }
}

pub fn negotiate(hello: &ClientFrame, conn_id: impl Into<String>) -> ServerFrame {
    let ClientFrame::Hello { protocol, .. } = hello else {
        return ServerFrame::HelloReject {
            reason: "first frame must be hello".into(),
            supported_majors: vec![PROTOCOL_MAJOR],
        };
    };
    if protocol.major != PROTOCOL_MAJOR {
        return ServerFrame::HelloReject {
            reason: format!("unsupported protocol major {}", protocol.major),
            supported_majors: vec![PROTOCOL_MAJOR],
        };
    }
    ServerFrame::HelloAck {
        protocol: negotiate_version(
            *protocol,
            Protocol {
                major: PROTOCOL_MAJOR,
                minor: PROTOCOL_MINOR,
            },
        )
        .expect("major version was checked above"),
        conn_id: conn_id.into(),
        heartbeat_ms: 15_000,
        pending_tool_calls: 0,
    }
}

/// Negotiates the greatest minor version understood by both peers. Major versions are never
/// silently downgraded.
pub fn negotiate_version(offered: Protocol, supported: Protocol) -> Option<Protocol> {
    (offered.major == supported.major).then_some(Protocol {
        major: supported.major,
        minor: offered.minor.min(supported.minor),
    })
}

pub fn encode_server_frame(frame: &ServerFrame) -> Result<Vec<u8>, String> {
    let value = server_to_sol(frame)?;
    Limits::default()
        .check(&value)
        .map_err(|error| error.to_string())?;
    let payload = aelio_sol::canonical_bytes(&value);
    if payload.len() > MAX_FRAME_BYTES {
        return Err("wire frame exceeds 1 MiB".into());
    }
    let length = u32::try_from(payload.len()).map_err(|_| "wire frame too large")?;
    let mut encoded = Vec::with_capacity(payload.len() + 4);
    encoded.extend_from_slice(&length.to_be_bytes());
    encoded.extend_from_slice(&payload);
    Ok(encoded)
}

pub fn decode_length_prefixed(bytes: &[u8]) -> Result<&[u8], String> {
    let prefix: [u8; 4] = bytes
        .get(..4)
        .ok_or("wire frame missing length prefix")?
        .try_into()
        .map_err(|_| "bad length prefix")?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > MAX_FRAME_BYTES || bytes.len() != length + 4 {
        return Err("wire frame length mismatch or limit exceeded".into());
    }
    Ok(&bytes[4..])
}

fn server_to_sol(frame: &ServerFrame) -> Result<SolValue, String> {
    Ok(match frame {
        ServerFrame::HelloAck {
            protocol,
            conn_id,
            heartbeat_ms,
            pending_tool_calls,
        } => SolValue::map([
            ("frame", SolValue::str("hello_ack")),
            ("protocol", protocol_sol(*protocol)),
            ("conn_id", SolValue::str(conn_id)),
            ("heartbeat_ms", int(*heartbeat_ms)?),
            ("pending_tool_calls", int(*pending_tool_calls)?),
        ]),
        ServerFrame::HelloReject {
            reason,
            supported_majors,
        } => SolValue::map([
            ("frame", SolValue::str("hello_reject")),
            ("reason", SolValue::str(reason)),
            (
                "supported_majors",
                SolValue::list(
                    supported_majors
                        .iter()
                        .copied()
                        .map(int)
                        .collect::<Result<Vec<_>, _>>()?,
                ),
            ),
        ]),
        ServerFrame::ToolCall {
            corr,
            target,
            effect_class,
            args,
            deadline_ms,
            idem_key,
            turn_ref,
        } => {
            if !target.contains('@') || EffectClass::parse(effect_class).is_none() {
                return Err("tool_call requires a pinned target and valid effect_class".into());
            }
            if *deadline_ms == 0 {
                return Err("tool_call deadline_ms must be positive".into());
            }
            let mut fields = vec![
                ("frame", SolValue::str("tool_call")),
                ("corr", SolValue::str(corr)),
                ("target", SolValue::str(target)),
                ("effect_class", SolValue::str(effect_class)),
                ("args", args.clone()),
                ("deadline_ms", int(*deadline_ms)?),
                ("turn_ref", SolValue::str(turn_ref)),
            ];
            if let Some(key) = idem_key {
                fields.push(("idem_key", SolValue::str(key)));
            }
            SolValue::map(fields)
        }
        ServerFrame::Ack { corr } => SolValue::map([
            ("frame", SolValue::str("ack")),
            ("corr", SolValue::str(corr)),
        ]),
        ServerFrame::Ping { t } => {
            SolValue::map([("frame", SolValue::str("ping")), ("t", int(*t)?)])
        }
        ServerFrame::Pong { t } => {
            SolValue::map([("frame", SolValue::str("pong")), ("t", int(*t)?)])
        }
    })
}

fn protocol_sol(protocol: Protocol) -> SolValue {
    SolValue::map([
        ("major", SolValue::Int(protocol.major as i64)),
        ("minor", SolValue::Int(protocol.minor as i64)),
    ])
}

fn parse_protocol(json: &Json) -> Result<Protocol, String> {
    let object = json.as_object().ok_or("protocol must be an object")?;
    closed(object, &["major", "minor"])?;
    Ok(Protocol {
        major: object
            .get("major")
            .and_then(Json::as_u64)
            .ok_or("protocol.major")?,
        minor: object
            .get("minor")
            .and_then(Json::as_u64)
            .ok_or("protocol.minor")?,
    })
}

fn closed(object: &serde_json::Map<String, Json>, allowed: &[&str]) -> Result<(), String> {
    if let Some(unknown) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("unknown wire field `{unknown}`"));
    }
    Ok(())
}

fn string(object: &serde_json::Map<String, Json>, key: &str) -> Result<String, String> {
    object
        .get(key)
        .and_then(Json::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("wire `{key}` must be a string"))
}

fn nonempty(value: String, field: &str) -> Result<String, String> {
    if value.is_empty() {
        Err(format!("wire `{field}` must not be empty"))
    } else {
        Ok(value)
    }
}

fn positive_u64(object: &serde_json::Map<String, Json>, key: &str) -> Result<u64, String> {
    object
        .get(key)
        .and_then(Json::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("wire `{key}` must be positive"))
}

/// Server-side delivery state for the reverse tool channel. This is deliberately independent of a
/// socket implementation: the caller persists transitions alongside the kernel ledger, and only
/// calls `mark_dispatched` after the frame has actually left the server.
#[derive(Debug, Default)]
pub struct DeliveryBook {
    calls: BTreeMap<String, DeliveryRecord>,
}

#[derive(Debug, Clone)]
struct DeliveryRecord {
    frame: ServerFrame,
    effect: EffectClass,
    deadline_at_ms: u64,
    state: DeliveryState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryState {
    Prepared,
    Dispatched,
    Acknowledged,
    Resolved,
    ExpiredRetryable,
    ExpiredUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpiryDisposition {
    ToolTransient,
    InternalUnknownOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultDisposition {
    Accepted,
    Duplicate,
    Late,
    UnknownCorrelation,
}

impl DeliveryBook {
    /// Registers an unresolved call before attempting socket delivery. Correlation identifiers are
    /// immutable and unique for the life of the book.
    pub fn prepare(&mut self, frame: ServerFrame, deadline_at_ms: u64) -> Result<(), String> {
        let ServerFrame::ToolCall {
            corr,
            effect_class,
            deadline_ms,
            ..
        } = &frame
        else {
            return Err("only tool_call frames can enter delivery state".into());
        };
        if corr.is_empty() || *deadline_ms == 0 || deadline_at_ms == 0 {
            return Err("delivery requires corr and positive deadlines".into());
        }
        let effect =
            EffectClass::parse(effect_class).ok_or("delivery requires a valid effect_class")?;
        if self.calls.contains_key(corr) {
            return Err(format!("duplicate delivery corr `{corr}`"));
        }
        self.calls.insert(
            corr.clone(),
            DeliveryRecord {
                frame,
                effect,
                deadline_at_ms,
                state: DeliveryState::Prepared,
            },
        );
        Ok(())
    }

    /// Marks the write boundary. The kernel must durably append `call_dispatch` at this boundary.
    pub fn mark_dispatched(&mut self, corr: &str) -> Result<(), String> {
        let record = self.calls.get_mut(corr).ok_or("unknown delivery corr")?;
        match record.state {
            DeliveryState::Prepared => {
                record.state = DeliveryState::Dispatched;
                Ok(())
            }
            DeliveryState::Dispatched | DeliveryState::Acknowledged => Ok(()),
            _ => Err("delivery is no longer dispatchable".into()),
        }
    }

    pub fn acknowledge(&mut self, corr: &str) -> Result<(), String> {
        let record = self.calls.get_mut(corr).ok_or("unknown delivery corr")?;
        match record.state {
            DeliveryState::Dispatched | DeliveryState::Acknowledged => {
                record.state = DeliveryState::Acknowledged;
                Ok(())
            }
            DeliveryState::Prepared => Err("cannot acknowledge an undispatched call".into()),
            _ => Ok(()),
        }
    }

    /// Returns unresolved frames that must be re-sent after reconnect. Re-sending does not create
    /// a new dispatch transition or correlation identifier.
    pub fn unresolved(&self, now_ms: u64) -> Vec<&ServerFrame> {
        self.calls
            .values()
            .filter(|record| {
                now_ms < record.deadline_at_ms
                    && matches!(
                        record.state,
                        DeliveryState::Prepared
                            | DeliveryState::Dispatched
                            | DeliveryState::Acknowledged
                    )
            })
            .map(|record| &record.frame)
            .collect()
    }

    /// Applies authoritative server-side expiry and reports the exact error class the kernel must
    /// raise. A dispatched write/external call is never labelled retryable.
    pub fn expire(&mut self, corr: &str, now_ms: u64) -> Result<Option<ExpiryDisposition>, String> {
        let record = self.calls.get_mut(corr).ok_or("unknown delivery corr")?;
        if now_ms < record.deadline_at_ms {
            return Ok(None);
        }
        let disposition = match record.state {
            DeliveryState::Prepared => ExpiryDisposition::ToolTransient,
            DeliveryState::Dispatched | DeliveryState::Acknowledged => match record.effect {
                EffectClass::Write | EffectClass::External => {
                    ExpiryDisposition::InternalUnknownOutcome
                }
                EffectClass::Pure | EffectClass::Read => ExpiryDisposition::ToolTransient,
            },
            DeliveryState::Resolved => return Ok(None),
            DeliveryState::ExpiredRetryable => return Ok(Some(ExpiryDisposition::ToolTransient)),
            DeliveryState::ExpiredUnknown => {
                return Ok(Some(ExpiryDisposition::InternalUnknownOutcome));
            }
        };
        record.state = match disposition {
            ExpiryDisposition::ToolTransient => DeliveryState::ExpiredRetryable,
            ExpiryDisposition::InternalUnknownOutcome => DeliveryState::ExpiredUnknown,
        };
        Ok(Some(disposition))
    }

    /// Deduplicates results by correlation id. Late results are observable but can never resolve or
    /// re-enter a flow that has already failed.
    pub fn accept_result(&mut self, corr: &str) -> ResultDisposition {
        let Some(record) = self.calls.get_mut(corr) else {
            return ResultDisposition::UnknownCorrelation;
        };
        match record.state {
            DeliveryState::Prepared | DeliveryState::Dispatched | DeliveryState::Acknowledged => {
                record.state = DeliveryState::Resolved;
                ResultDisposition::Accepted
            }
            DeliveryState::Resolved => ResultDisposition::Duplicate,
            DeliveryState::ExpiredRetryable | DeliveryState::ExpiredUnknown => {
                ResultDisposition::Late
            }
        }
    }

    /// Cancel a call that was prepared but provably never crossed the dispatch boundary. This is
    /// used when the server-side outbound queue rejects the frame; dispatched calls can never be
    /// removed through this surface.
    pub fn cancel_prepared(&mut self, corr: &str) -> Result<(), String> {
        let Some(record) = self.calls.get(corr) else {
            return Err("unknown delivery corr".into());
        };
        if record.state != DeliveryState::Prepared {
            return Err("only an undispatched prepared call can be cancelled".into());
        }
        self.calls.remove(corr);
        Ok(())
    }

    /// Remove a terminal correlation tombstone after the caller's bounded retention window. The
    /// book refuses to prune unresolved work so memory-pressure policy cannot create a duplicate
    /// effect window.
    pub fn remove_terminal(&mut self, corr: &str) -> Result<(), String> {
        let Some(record) = self.calls.get(corr) else {
            return Ok(());
        };
        if !matches!(
            record.state,
            DeliveryState::Resolved
                | DeliveryState::ExpiredRetryable
                | DeliveryState::ExpiredUnknown
        ) {
            return Err("unresolved delivery correlation cannot be pruned".into());
        }
        self.calls.remove(corr);
        Ok(())
    }
}

fn int(value: u64) -> Result<SolValue, String> {
    i64::try_from(value)
        .map(SolValue::Int)
        .map_err(|_| "wire integer exceeds i64".into())
}

fn sol(json: &Json) -> Result<SolValue, String> {
    let value = match json {
        Json::Null => SolValue::Null,
        Json::Bool(value) => SolValue::Bool(*value),
        Json::Number(value) => {
            if let Some(value) = value.as_i64() {
                SolValue::Int(value)
            } else {
                SolValue::float(value.as_f64().ok_or("number out of range")?)
                    .map_err(|error| error.to_string())?
            }
        }
        Json::String(value) => SolValue::str(value),
        Json::Array(values) => {
            SolValue::list(values.iter().map(sol).collect::<Result<Vec<_>, _>>()?)
        }
        Json::Object(values) => SolValue::map(
            values
                .iter()
                .map(|(key, value)| Ok((key.clone(), sol(value)?)))
                .collect::<Result<Vec<_>, String>>()?,
        ),
    };
    Limits::default()
        .check(&value)
        .map_err(|error| error.to_string())?;
    Ok(value)
}
