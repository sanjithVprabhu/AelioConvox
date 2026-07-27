//! Turn driver: the live turn loop and the replay pass, both over the one [`crate::exec::Executor`]
//! walker (§12.3). This is where the ledger (§12.2, App G) is produced (live) and consumed
//! (replay), and where park/resume across turns is orchestrated.

use crate::error::{ErrV1, ReasonCode};
use crate::exec::{Backend, Executor, Frame, OnceClaim, Outcome, Suspend};
use crate::instr::Node;
use crate::ledger::{Category, Ledger};
use crate::registry::Registry;
use aelio_sol::{value_hash, SolValue};
use aelio_store::{once_begin, once_complete, MemoryStore, OnceState, StoreError};
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
    /// In-memory Once/CAS double for P0 (F-002). Production swaps Sunjet via the store trait.
    store: MemoryStore,
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
            store: MemoryStore::new(),
            tenant: "default".into(),
            instance_id: "inst0".into(),
        }
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
        self.ledger.append(&turn_id, None, "turn_start", Category::Info, SolValue::map([("trigger", SolValue::str("message"))]));
        let mut backend = LiveBackend {
            registry: self.registry,
            ledger: &mut self.ledger,
            turn_id: turn_id.clone(),
            pending_wake: None,
            store: &mut self.store,
            tenant: self.tenant.clone(),
            instance_id: self.instance_id.clone(),
        };
        let mut exec = Executor::new(initial_bag, &mut backend);
        let outcome = exec.run(&self.program)?;
        self.settle(outcome, &turn_id)
    }

    /// Resume a parked instance with a wake payload (§23 — routed here by the flow gate / event key).
    pub fn resume(&mut self, parked: Parked, wake: SolValue) -> Result<TurnOutcome, ErrV1> {
        let turn_id = self.next_turn_id();
        self.ledger.append(&turn_id, None, "turn_start", Category::Info, SolValue::map([("trigger", SolValue::str("wake"))]));
        // §8.4 resume sequence is enforced inside the Guard/park handling; the wake is INJECT-ledgered.
        self.ledger.append(&turn_id, Some(&parked.park_nid), "resume", Category::Inject, SolValue::map([("wake", wake.clone())]));
        let mut backend = LiveBackend {
            registry: self.registry,
            ledger: &mut self.ledger,
            turn_id: turn_id.clone(),
            pending_wake: Some(wake),
            store: &mut self.store,
            tenant: self.tenant.clone(),
            instance_id: self.instance_id.clone(),
        };
        let mut exec = Executor::new(parked.bag, &mut backend);
        let outcome = exec.resume(&self.program, parked.frames)?;
        self.settle(outcome, &turn_id)
    }

    fn settle(&mut self, outcome: Outcome, turn_id: &str) -> Result<TurnOutcome, ErrV1> {
        match outcome {
            Outcome::Completed { bag } => {
                let bag_hash = value_hash(&bag);
                self.ledger.append(turn_id, None, "turn_end", Category::Verify, SolValue::map([
                    ("outcome", SolValue::str("completed")),
                    ("bag_hash", SolValue::str(bag_hash.clone())),
                ]));
                Ok(TurnOutcome::Completed { bag, bag_hash })
            }
            Outcome::Parked { suspension, bag } => {
                let Suspend { park_nid, frames, .. } = suspension;
                let cont_hash = value_hash(&bag);
                self.ledger.append(turn_id, Some(&park_nid), "park", Category::Verify, SolValue::map([
                    ("park_nid", SolValue::str(park_nid.clone())),
                    ("continuation_hash", SolValue::str(cont_hash)),
                ]));
                self.ledger.append(turn_id, None, "turn_end", Category::Info, SolValue::map([("outcome", SolValue::str("parked"))]));
                Ok(TurnOutcome::Parked(Parked { bag, frames, park_nid }))
            }
        }
    }
}

/// Live backend: performs effects and appends ledger entries (§12.4).
struct LiveBackend<'a> {
    registry: &'a mut Registry,
    ledger: &'a mut Ledger,
    turn_id: String,
    /// Set on a resume turn; consumed by the first park reached with an empty cursor.
    pending_wake: Option<SolValue>,
    store: &'a mut MemoryStore,
    tenant: String,
    instance_id: String,
}

impl Backend for LiveBackend<'_> {
    fn call(&mut self, nid: &str, id: &str, args: SolValue) -> Result<SolValue, ErrV1> {
        let effect = self
            .registry
            .effect_of(id)
            .ok_or_else(|| ErrV1::new(ReasonCode::Shape, nid, format!("unregistered Call target `{id}`")))?;
        // §12.4: write/external Calls are intent-ledgered before dispatch.
        if effect.is_effectful() {
            self.ledger.append(&self.turn_id, Some(nid), "call_intent", Category::Verify, SolValue::map([
                ("target", SolValue::str(id)),
                ("args_hash", SolValue::str(value_hash(&args))),
            ]));
        }
        let result = self
            .registry
            .call(id, &args)
            .ok_or_else(|| ErrV1::new(ReasonCode::Internal, nid, "target vanished"))??;
        let kind = if effect == crate::registry::EffectClass::Read { "read_result" } else { "call_result" };
        self.ledger.append(&self.turn_id, Some(nid), kind, Category::Inject, SolValue::map([
            ("target", SolValue::str(id)),
            ("output", result.clone()),
        ]));
        Ok(result)
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
            Err(e) => Err(ErrV1::new(ReasonCode::Internal, nid, format!("once store: {e:?}"))),
        }
    }

    fn once_complete(&mut self, nid: &str, idem_key: &str) -> Result<(), ErrV1> {
        let full = format!("{}|{}|{}", self.tenant, self.instance_id, idem_key);
        once_complete(self.store, &self.tenant, &full, SolValue::map([("ok", SolValue::Bool(true))]))
            .map_err(|e| ErrV1::new(ReasonCode::Internal, nid, format!("once complete: {e:?}")))?;
        self.ledger.append(
            &self.turn_id,
            Some(nid),
            "once_result",
            Category::Inject,
            SolValue::map([("idem_key", SolValue::str(full)), ("ok", SolValue::Bool(true))]),
        );
        Ok(())
    }
}

// ── replay (§12.3): re-run the program as a pure function of the ledger ────────────────────────

/// Replay a completed instance's ledger against its program and return the recomputed final
/// `bag_hash`, hard-refusing on any divergence (§12.3). Bit-identity ⇔ the returned hash equals the
/// ledger's `turn_end.bag_hash` (checked by the caller / here).
pub fn replay(program: &Node, ledger: &Ledger, initial_bag: SolValue) -> Result<String, ErrV1> {
    // Collect the recorded effect/park/resume entries in order.
    let mut effects: VecDeque<&crate::ledger::Entry> = ledger
        .entries()
        .iter()
        .filter(|e| matches!(e.kind.as_str(), "call_result" | "read_result" | "park" | "resume"))
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

    let mut backend = ReplayBackend { entries: &mut effects };
    let mut exec = Executor::new(initial_bag, &mut backend);
    // Replay never suspends: at each park it injects the recorded resume wake and continues.
    match exec.run(program)? {
        Outcome::Completed { bag } => {
            let recomputed = value_hash(&bag);
            match expected_final {
                Some(expected) if expected != recomputed => Err(ErrV1::new(
                    ReasonCode::Internal,
                    "replay",
                    "bag_hash divergence — hard refuse (§12.3)",
                )),
                _ => Ok(recomputed),
            }
        }
        Outcome::Parked { .. } => Err(ErrV1::new(ReasonCode::Internal, "replay", "replay parked — ledger incomplete")),
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
            return Err(ErrV1::new(ReasonCode::Internal, nid, format!("replay divergence: expected {expect_kind:?}, got {}", e.kind)));
        }
        if let Some(n) = &e.nid {
            if n != nid {
                return Err(ErrV1::new(ReasonCode::Internal, nid, format!("replay nid mismatch: {n} ≠ {nid}")));
            }
        }
        Ok(e)
    }
}

impl Backend for ReplayBackend<'_> {
    fn call(&mut self, nid: &str, _id: &str, _args: SolValue) -> Result<SolValue, ErrV1> {
        let e = self.pop(&["call_result", "read_result"], nid)?;
        e.payload
            .as_map()
            .and_then(|m| m.get("output"))
            .cloned()
            .ok_or_else(|| ErrV1::new(ReasonCode::Internal, nid, "replay: result entry missing output"))
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
            .ok_or_else(|| ErrV1::new(ReasonCode::Internal, nid, "replay: resume entry missing wake"))?;
        Ok(Some(wake))
    }
}
