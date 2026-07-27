//! Turn driver: the live turn loop and the replay pass, both over the one [`crate::exec::Executor`]
//! walker (§12.3). This is where the ledger (§12.2, App G) is produced (live) and consumed
//! (replay), and where park/resume across turns is orchestrated.

use crate::error::{ErrV1, ReasonCode};
use crate::exec::{Backend, Executor, Frame, OnceClaim, Outcome, Suspend};
use crate::instr::Node;
use crate::ledger::{Category, Ledger};
use crate::registry::Registry;
use aelio_sol::{value_hash, SolValue};
use aelio_store::{once_begin, once_complete, MemoryStore, OnceState, Store, StoreError};
use std::collections::VecDeque;

/// A parked instance: what the driver holds between turns.
pub struct Parked {
    pub bag: SolValue,
    pub frames: VecDeque<Frame>,
    pub park_nid: String,
}

/// Result of driving one turn.
pub enum TurnOutcome {
    Completed { bag: SolValue, bag_hash: String },
    Parked(Parked),
}

/// Drives a single flow instance across turns, owning its ledger + registry + Once store.
pub struct Instance<'r> {
    turn_id_seq: u64,
    ledger: Ledger,
    registry: &'r mut Registry,
    program: Node,
    store: Box<dyn Store>,
    tenant: String,
    instance_id: String,
}

impl<'r> Instance<'r> {
    pub fn new(program: Node, registry: &'r mut Registry) -> Self {
        Instance {
            turn_id_seq: 0,
            ledger: Ledger::default(),
            registry,
            program,
            store: Box::new(MemoryStore::new()),
            tenant: "default".into(),
            instance_id: "inst0".into(),
        }
    }

    /// Construct an instance over an injected durable store. Production callers must use this
    /// constructor; [`Self::new`] intentionally remains the deterministic test/demo convenience.
    pub fn with_store(
        program: Node,
        registry: &'r mut Registry,
        store: Box<dyn Store>,
        tenant: impl Into<String>,
        instance_id: impl Into<String>,
    ) -> Result<Self, ErrV1> {
        let tenant = tenant.into();
        let instance_id = instance_id.into();
        if tenant.is_empty() || instance_id.is_empty() {
            return Err(ErrV1::new(
                ReasonCode::Shape,
                "instance",
                "tenant and flow_instance_id must be non-empty",
            ));
        }
        Ok(Instance {
            turn_id_seq: 0,
            ledger: Ledger::default(),
            registry,
            program,
            store,
            tenant,
            instance_id,
        })
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    fn next_turn_id(&mut self) -> String {
        let id = format!("t{}", self.turn_id_seq);
        self.turn_id_seq += 1;
        id
    }

    /// First turn: run from the top until completion or the first park.
    pub fn start(&mut self, initial_bag: SolValue) -> Result<TurnOutcome, ErrV1> {
        let turn_id = self.next_turn_id();
        self.ledger.append(
            &turn_id,
            None,
            "turn_start",
            Category::Info,
            SolValue::map([("trigger", SolValue::str("message"))]),
        );
        let (outcome, error_bag) = {
            let mut backend = LiveBackend {
                registry: self.registry,
                ledger: &mut self.ledger,
                turn_id: turn_id.clone(),
                pending_wake: None,
                store: self.store.as_mut(),
                tenant: self.tenant.clone(),
                instance_id: self.instance_id.clone(),
                nondet_seq: 0,
            };
            let mut exec = Executor::new(initial_bag, &mut backend);
            let outcome = exec.run(&self.program);
            let error_bag = exec.bag().clone();
            (outcome, error_bag)
        };
        match outcome {
            Ok(outcome) => self.settle(outcome, &turn_id),
            Err(error) => self.settle_error(error, error_bag, &turn_id),
        }
    }

    /// Resume a parked instance with a wake payload (§23 — routed here by the flow gate / event key).
    pub fn resume(&mut self, parked: Parked, wake: SolValue) -> Result<TurnOutcome, ErrV1> {
        let turn_id = self.next_turn_id();
        self.ledger.append(
            &turn_id,
            None,
            "turn_start",
            Category::Info,
            SolValue::map([("trigger", SolValue::str("wake"))]),
        );
        // §8.4 resume sequence is enforced inside the Guard/park handling; the wake is INJECT-ledgered.
        self.ledger.append(
            &turn_id,
            Some(&parked.park_nid),
            "resume",
            Category::Inject,
            SolValue::map([("wake", wake.clone())]),
        );
        let (outcome, error_bag) = {
            let mut backend = LiveBackend {
                registry: self.registry,
                ledger: &mut self.ledger,
                turn_id: turn_id.clone(),
                pending_wake: Some(wake),
                store: self.store.as_mut(),
                tenant: self.tenant.clone(),
                instance_id: self.instance_id.clone(),
                nondet_seq: 0,
            };
            let mut exec = Executor::new(parked.bag, &mut backend);
            let outcome = exec.resume(&self.program, parked.frames);
            let error_bag = exec.bag().clone();
            (outcome, error_bag)
        };
        match outcome {
            Ok(outcome) => self.settle(outcome, &turn_id),
            Err(error) => self.settle_error(error, error_bag, &turn_id),
        }
    }

    fn settle(&mut self, outcome: Outcome, turn_id: &str) -> Result<TurnOutcome, ErrV1> {
        match outcome {
            Outcome::Completed { bag } => {
                let bag_hash = value_hash(&bag);
                self.ledger.append(
                    turn_id,
                    None,
                    "turn_end",
                    Category::Verify,
                    SolValue::map([
                        ("outcome", SolValue::str("completed")),
                        ("bag_hash", SolValue::str(bag_hash.clone())),
                    ]),
                );
                Ok(TurnOutcome::Completed { bag, bag_hash })
            }
            Outcome::Parked { suspension, bag } => {
                let Suspend {
                    park_nid, frames, ..
                } = suspension;
                let cont_hash = value_hash(&bag);
                self.ledger.append(
                    turn_id,
                    Some(&park_nid),
                    "park",
                    Category::Verify,
                    SolValue::map([
                        ("park_nid", SolValue::str(park_nid.clone())),
                        ("continuation_hash", SolValue::str(cont_hash)),
                    ]),
                );
                self.ledger.append(
                    turn_id,
                    None,
                    "turn_end",
                    Category::Info,
                    SolValue::map([("outcome", SolValue::str("parked"))]),
                );
                Ok(TurnOutcome::Parked(Parked {
                    bag,
                    frames,
                    park_nid,
                }))
            }
        }
    }

    fn settle_error(
        &mut self,
        error: ErrV1,
        bag: SolValue,
        turn_id: &str,
    ) -> Result<TurnOutcome, ErrV1> {
        self.ledger.append(
            turn_id,
            None,
            "turn_end",
            Category::Verify,
            SolValue::map([
                ("outcome", SolValue::str("erred")),
                ("final_err", error.to_sol()),
                ("bag_hash", SolValue::str(value_hash(&bag))),
            ]),
        );
        Err(error)
    }
}

/// Live backend: performs effects and appends ledger entries (§12.4).
struct LiveBackend<'a> {
    registry: &'a mut Registry,
    ledger: &'a mut Ledger,
    turn_id: String,
    /// Set on a resume turn; consumed by the first park reached with an empty cursor.
    pending_wake: Option<SolValue>,
    store: &'a mut dyn Store,
    tenant: String,
    instance_id: String,
    /// Monotonic per-turn counter mixed into generated nondet values (uuid uniqueness).
    nondet_seq: u64,
}

impl Backend for LiveBackend<'_> {
    fn call(&mut self, nid: &str, id: &str, args: SolValue) -> Result<SolValue, ErrV1> {
        let effect = self.registry.effect_of(id).ok_or_else(|| {
            ErrV1::new(
                ReasonCode::Shape,
                nid,
                format!("unregistered Call target `{id}`"),
            )
        })?;
        // §12.4: write/external Calls are intent-ledgered before dispatch.
        if effect.is_effectful() {
            self.ledger.append(
                &self.turn_id,
                Some(nid),
                "call_intent",
                Category::Verify,
                SolValue::map([
                    ("target", SolValue::str(id)),
                    (
                        "effect_class",
                        SolValue::str(match effect {
                            crate::registry::EffectClass::Write => "write",
                            crate::registry::EffectClass::External => "external",
                            _ => unreachable!(),
                        }),
                    ),
                    ("args_hash", SolValue::str(value_hash(&args))),
                ]),
            );
            self.ledger.append(
                &self.turn_id,
                Some(nid),
                "call_dispatch",
                Category::Verify,
                SolValue::map([("corr", SolValue::str(format!("{}:{nid}", self.turn_id)))]),
            );
        }
        let result = self
            .registry
            .call(id, &args)
            .ok_or_else(|| ErrV1::new(ReasonCode::Internal, nid, "target vanished"))?;
        let kind = if effect == crate::registry::EffectClass::Read {
            "read_result"
        } else {
            "call_result"
        };
        match result {
            Ok(output) => {
                self.ledger.append(
                    &self.turn_id,
                    Some(nid),
                    kind,
                    Category::Inject,
                    SolValue::map([
                        ("target", SolValue::str(id)),
                        ("args_hash", SolValue::str(value_hash(&args))),
                        ("outcome", SolValue::str("ok")),
                        ("output", output.clone()),
                    ]),
                );
                Ok(output)
            }
            Err(error) => {
                self.ledger.append(
                    &self.turn_id,
                    Some(nid),
                    kind,
                    Category::Inject,
                    SolValue::map([
                        ("target", SolValue::str(id)),
                        ("args_hash", SolValue::str(value_hash(&args))),
                        ("outcome", SolValue::str("err")),
                        ("err", error.to_sol()),
                    ]),
                );
                Err(error)
            }
        }
    }

    fn at_park(&mut self, _nid: &str) -> Result<Option<SolValue>, ErrV1> {
        // Live: wake if the driver injected one for this resume; else suspend the turn.
        Ok(self.pending_wake.take())
    }

    fn once_claim(&mut self, nid: &str, idem_key: &str) -> Result<OnceClaim, ErrV1> {
        let full = format!("{}|{}|{}", self.tenant, self.instance_id, idem_key);
        match once_begin(self.store, &self.tenant, &full) {
            Ok(OnceState::Execute) => {
                self.ledger.append(
                    &self.turn_id,
                    Some(nid),
                    "once_intent",
                    Category::Verify,
                    SolValue::map([("idem_key", SolValue::str(full))]),
                );
                Ok(OnceClaim::Run)
            }
            Ok(OnceState::Replay(_)) => Ok(OnceClaim::Skip),
            Err(StoreError::UnknownOutcome) => Err(ErrV1::new(
                ReasonCode::Internal,
                nid,
                "Once intent-without-result — fail-loud, never re-execute (§8.4)",
            )),
            Err(e) => Err(ErrV1::new(
                ReasonCode::Internal,
                nid,
                format!("once store: {e:?}"),
            )),
        }
    }

    fn once_complete(&mut self, nid: &str, idem_key: &str) -> Result<(), ErrV1> {
        let full = format!("{}|{}|{}", self.tenant, self.instance_id, idem_key);
        once_complete(
            self.store,
            &self.tenant,
            &full,
            SolValue::map([("ok", SolValue::Bool(true))]),
        )
        .map_err(|e| ErrV1::new(ReasonCode::Internal, nid, format!("once complete: {e:?}")))?;
        self.ledger.append(
            &self.turn_id,
            Some(nid),
            "once_result",
            Category::Inject,
            SolValue::map([
                ("idem_key", SolValue::str(full)),
                ("ok", SolValue::Bool(true)),
            ]),
        );
        Ok(())
    }

    fn nondet(&mut self, nid: &str, source: &str) -> Result<SolValue, ErrV1> {
        // §9 L0-C: generate the value once, live, and record it as an INJECT entry (§12.2). Replay
        // reads it back verbatim (§12.3) so the turn is bit-identical despite the nondeterminism.
        let value = match source {
            "now" => SolValue::Int(now_millis()),
            "uuid" => SolValue::str(gen_uuid(&self.turn_id, self.nondet_seq)),
            "random" => SolValue::Int(gen_random(self.nondet_seq)),
            other => {
                return Err(ErrV1::new(
                    ReasonCode::Shape,
                    nid,
                    format!("unknown nondet source `{other}` (§9)"),
                ))
            }
        };
        self.nondet_seq += 1;
        self.ledger.append(
            &self.turn_id,
            Some(nid),
            "nondet_value",
            Category::Inject,
            SolValue::map([("source", SolValue::str(source)), ("value", value.clone())]),
        );
        Ok(value)
    }

    fn validate(&mut self, nid: &str, validator_id: &str, value: &SolValue) -> Result<bool, ErrV1> {
        // §5.4 validators are registered targets returning bool. Deterministic, but ledgered so
        // replay does not need the registry (replay stays a pure function of the ledger, §12.3).
        let out = self
            .registry
            .call(validator_id, &SolValue::map([("value", value.clone())]))
            .ok_or_else(|| {
                ErrV1::new(
                    ReasonCode::Shape,
                    nid,
                    format!("unregistered validator `{validator_id}` (§5.4)"),
                )
            })??;
        let result = match out {
            SolValue::Bool(b) => b,
            _ => {
                return Err(ErrV1::new(
                    ReasonCode::Type,
                    nid,
                    "validator must return bool (§5.4)",
                ))
            }
        };
        self.ledger.append(
            &self.turn_id,
            Some(nid),
            "validate_result",
            Category::Inject,
            SolValue::map([
                ("validator", SolValue::str(validator_id)),
                ("result", SolValue::Bool(result)),
            ]),
        );
        Ok(result)
    }

    fn report(&mut self, nid: &str, kind: &str, payload: SolValue) -> Result<(), ErrV1> {
        self.ledger
            .append(&self.turn_id, Some(nid), kind, Category::Inject, payload);
        Ok(())
    }

    fn time_exceeded(
        &mut self,
        nid: &str,
        kind: &str,
        meter: &str,
        limit: u64,
        observed: u64,
    ) -> Result<bool, ErrV1> {
        if observed <= limit {
            return Ok(false);
        }
        self.ledger.append(
            &self.turn_id,
            Some(nid),
            kind,
            Category::Inject,
            SolValue::map([
                ("scope_nid", SolValue::str(nid)),
                ("meter", SolValue::str(meter)),
                ("limit", SolValue::Int(limit as i64)),
                ("observed", SolValue::Int(observed as i64)),
            ]),
        );
        Ok(true)
    }
}

fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A version-4-shaped identifier derived from turn + counter + clock (uniqueness only; the value is
/// ledgered, so replay reproduces it exactly regardless of how it was minted).
fn gen_uuid(turn: &str, seq: u64) -> String {
    let h = value_hash(&SolValue::str(format!("{turn}:{seq}:{}", now_millis())));
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn gen_random(seq: u64) -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    ((nanos ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15)) & (i64::MAX as u64)) as i64
}

// ── replay (§12.3): re-run the program as a pure function of the ledger ────────────────────────

/// Replay a completed instance's ledger against its program and return the recomputed final
/// `bag_hash`, hard-refusing on any divergence (§12.3). Bit-identity ⇔ the returned hash equals the
/// ledger's `turn_end.bag_hash` (checked by the caller / here).
pub fn replay(program: &Node, ledger: &Ledger, initial_bag: SolValue) -> Result<String, ErrV1> {
    ledger.verify_chain().map_err(|seq| {
        ErrV1::new(
            ReasonCode::Internal,
            "replay",
            format!("ledger hash-chain divergence at seq {seq}"),
        )
    })?;
    // Collect the recorded effect/park/resume entries in order.
    let mut effects: VecDeque<&crate::ledger::Entry> = ledger
        .entries()
        .iter()
        .filter(|e| {
            matches!(
                e.kind.as_str(),
                "call_result"
                    | "read_result"
                    | "call_intent"
                    | "call_dispatch"
                    | "park"
                    | "resume"
                    | "nondet_value"
                    | "validate_result"
                    | "side_failed"
                    | "finally_failed"
                    | "budget_trip"
                    | "timeout_trip"
            )
        })
        .collect();
    let expected_final = ledger
        .entries()
        .iter()
        .rev()
        .find(|e| e.kind == "turn_end")
        .and_then(|e| e.payload.as_map().and_then(|m| m.get("bag_hash")))
        .and_then(|h| match h {
            SolValue::Str(s) => Some(s.clone()),
            _ => None,
        });

    let mut backend = ReplayBackend {
        entries: &mut effects,
    };
    let mut exec = Executor::new(initial_bag, &mut backend);
    // Replay never suspends: at each park it injects the recorded resume wake and continues.
    match exec.run(program)? {
        Outcome::Completed { bag } => {
            let recomputed = value_hash(&bag);
            drop(exec);
            if !backend.entries.is_empty() {
                return Err(ErrV1::new(
                    ReasonCode::Internal,
                    "replay",
                    "ledger has unconsumed nondeterministic entries",
                ));
            }
            match expected_final {
                Some(expected) if expected != recomputed => Err(ErrV1::new(
                    ReasonCode::Internal,
                    "replay",
                    "bag_hash divergence — hard refuse (§12.3)",
                )),
                _ => Ok(recomputed),
            }
        }
        Outcome::Parked { .. } => Err(ErrV1::new(
            ReasonCode::Internal,
            "replay",
            "replay parked — ledger incomplete",
        )),
    }
}

struct ReplayBackend<'a> {
    entries: &'a mut VecDeque<&'a crate::ledger::Entry>,
}

impl ReplayBackend<'_> {
    fn pop(&mut self, expect_kind: &[&str], nid: &str) -> Result<&crate::ledger::Entry, ErrV1> {
        let e = self
            .entries
            .pop_front()
            .ok_or_else(|| ErrV1::new(ReasonCode::Internal, nid, "replay ran past the ledger"))?;
        if !expect_kind.contains(&e.kind.as_str()) {
            // §12.3: kind/nid mismatch ⇒ hard refuse.
            return Err(ErrV1::new(
                ReasonCode::Internal,
                nid,
                format!(
                    "replay divergence: expected {expect_kind:?}, got {}",
                    e.kind
                ),
            ));
        }
        if let Some(n) = &e.nid {
            if n != nid {
                return Err(ErrV1::new(
                    ReasonCode::Internal,
                    nid,
                    format!("replay nid mismatch: {n} ≠ {nid}"),
                ));
            }
        }
        Ok(e)
    }
}

impl Backend for ReplayBackend<'_> {
    fn call(&mut self, nid: &str, id: &str, args: SolValue) -> Result<SolValue, ErrV1> {
        if self
            .entries
            .front()
            .is_some_and(|entry| entry.kind == "call_intent")
        {
            let intent = self.pop(&["call_intent"], nid)?;
            verify_call_identity(intent, nid, id, &args)?;
            self.pop(&["call_dispatch"], nid)?;
        }
        let e = self.pop(&["call_result", "read_result"], nid)?;
        verify_call_identity(e, nid, id, &args)?;
        let payload = e
            .payload
            .as_map()
            .ok_or_else(|| ErrV1::new(ReasonCode::Internal, nid, "replay: bad result payload"))?;
        match payload.get("outcome") {
            Some(SolValue::Str(outcome)) if outcome == "ok" => {
                payload.get("output").cloned().ok_or_else(|| {
                    ErrV1::new(
                        ReasonCode::Internal,
                        nid,
                        "replay: result entry missing output",
                    )
                })
            }
            Some(SolValue::Str(outcome)) if outcome == "err" => {
                let error = payload
                    .get("err")
                    .and_then(ErrV1::from_sol)
                    .ok_or_else(|| {
                        ErrV1::new(
                            ReasonCode::Internal,
                            nid,
                            "replay: result entry missing err.v1",
                        )
                    })?;
                Err(error)
            }
            _ => Err(ErrV1::new(
                ReasonCode::Internal,
                nid,
                "replay: result entry missing outcome",
            )),
        }
    }

    fn at_park(&mut self, nid: &str) -> Result<Option<SolValue>, ErrV1> {
        // The live ledger recorded `park` then (on the resuming turn) `resume`. Consume both here.
        let _park = self.pop(&["park"], nid)?;
        let resume = self.pop(&["resume"], nid)?;
        let wake = resume
            .payload
            .as_map()
            .and_then(|m| m.get("wake"))
            .cloned()
            .ok_or_else(|| {
                ErrV1::new(
                    ReasonCode::Internal,
                    nid,
                    "replay: resume entry missing wake",
                )
            })?;
        Ok(Some(wake))
    }

    fn nondet(&mut self, nid: &str, _source: &str) -> Result<SolValue, ErrV1> {
        let e = self.pop(&["nondet_value"], nid)?;
        e.payload
            .as_map()
            .and_then(|m| m.get("value"))
            .cloned()
            .ok_or_else(|| {
                ErrV1::new(
                    ReasonCode::Internal,
                    nid,
                    "replay: nondet_value entry missing value",
                )
            })
    }

    fn validate(
        &mut self,
        nid: &str,
        _validator_id: &str,
        _value: &SolValue,
    ) -> Result<bool, ErrV1> {
        let e = self.pop(&["validate_result"], nid)?;
        match e.payload.as_map().and_then(|m| m.get("result")) {
            Some(SolValue::Bool(b)) => Ok(*b),
            _ => Err(ErrV1::new(
                ReasonCode::Internal,
                nid,
                "replay: validate_result entry missing bool result",
            )),
        }
    }

    fn report(&mut self, nid: &str, kind: &str, payload: SolValue) -> Result<(), ErrV1> {
        let entry = self.pop(&[kind], nid)?;
        if entry.payload != payload {
            return Err(ErrV1::new(
                ReasonCode::Internal,
                nid,
                "replay report payload divergence",
            ));
        }
        Ok(())
    }

    fn time_exceeded(
        &mut self,
        nid: &str,
        kind: &str,
        _meter: &str,
        _limit: u64,
        _observed: u64,
    ) -> Result<bool, ErrV1> {
        if self
            .entries
            .front()
            .is_some_and(|entry| entry.kind == kind && entry.nid.as_deref() == Some(nid))
        {
            self.pop(&[kind], nid)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

fn verify_call_identity(
    entry: &crate::ledger::Entry,
    nid: &str,
    id: &str,
    args: &SolValue,
) -> Result<(), ErrV1> {
    let payload = entry
        .payload
        .as_map()
        .ok_or_else(|| ErrV1::new(ReasonCode::Internal, nid, "replay: bad call payload"))?;
    if payload.get("target") != Some(&SolValue::str(id)) {
        return Err(ErrV1::new(
            ReasonCode::Internal,
            nid,
            "replay call target divergence",
        ));
    }
    if payload.get("args_hash") != Some(&SolValue::str(value_hash(args))) {
        return Err(ErrV1::new(
            ReasonCode::Internal,
            nid,
            "replay call args_hash divergence",
        ));
    }
    Ok(())
}
