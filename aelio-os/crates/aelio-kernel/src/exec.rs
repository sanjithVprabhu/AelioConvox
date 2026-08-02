//! Executor (§12): strict depth-first walk over the checked plan. Effects and park/resume go
//! through a [`Backend`] so **live** and **replay** share one walker (§12.3 "replay = same executor,
//! ledger-fed"). Park/resume uses a frame-stack continuation (App I) so a resumed turn continues
//! from the park point without re-executing completed effects.

use crate::compute;
use crate::error::{ErrV1, ReasonCode};
use crate::instr::{Expr, GuardCheck, Kind, Node, Until};
use aelio_sol::{Path, SolValue};
use std::collections::VecDeque;

/// Result of walking a node.
enum Flow {
    Done,
    Suspend(Suspend),
}

/// A suspended continuation (App I frame stack, outermost-first).
pub struct Suspend {
    pub park_nid: String,
    pub until: Until,
    pub into: Option<Path>,
    pub frames: VecDeque<Frame>,
    /// Scope nids parallel to `frames`, retained for the durable App I representation.
    pub frame_nids: VecDeque<String>,
}

impl Suspend {
    fn push_frame(&mut self, nid: &str, frame: Frame) {
        self.frames.push_front(frame);
        self.frame_nids.push_front(nid.to_owned());
    }
}

/// Per-scope resume position (App I.1, minimal set covering the login flow).
#[derive(Debug, Clone)]
pub enum Frame {
    Seq(usize),
    Branch(bool),
    /// Generic single-body wrapper (Guard/Budget/Timeout/Tee).
    Body,
    /// Lexical values that must be restored when a resumed Let body exits.
    Let(Vec<(String, Option<SolValue>)>),
    /// Number of fully completed iterations before the currently suspended iteration.
    Loop(u64),
    TryBody,
    TryHandler {
        index: usize,
        prefix: String,
    },
    TryFinally(Option<ErrV1>),
    Fallback {
        step_index: usize,
        prior_errors: Vec<ErrV1>,
    },
    Budget {
        used_calls: u64,
        used_tokens: u64,
        used_ms: u64,
    },
    Timeout {
        used_ms: u64,
    },
    GuardHandler,
    /// Parent state retained while a Map child is the active continuation bag.
    Map {
        element_index: usize,
        collected: Vec<SolValue>,
        parent_bag: SolValue,
        parent_scope: SolValue,
    },
}

/// What the executor exposes to a driver.
pub enum Outcome {
    Completed { bag: SolValue },
    Parked { suspension: Suspend, bag: SolValue },
}

/// Result of claiming an `Once` region (§8.4).
#[derive(Debug, Clone, PartialEq)]
pub enum OnceClaim {
    /// Execute the body, then [`Backend::once_complete`].
    Run,
    /// Prior successful completion — skip body (bag already carries effects).
    Skip(SolValue),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallOutput {
    pub value: SolValue,
    pub usage_tokens: u64,
}

/// Effect + park behavior, differing live vs replay.
pub trait Backend {
    /// Invoke a Call target (live) or inject its recorded result (replay). `args` is the projected
    /// least-privilege input (§4.2.4). The backend owns the registry, so it classifies the effect
    /// (§10.1) and ledgers intent/result accordingly (§12.4).
    fn call(&mut self, nid: &str, id: &str, args: SolValue) -> Result<CallOutput, ErrV1>;

    /// At a park reached with an empty resume cursor: `Some(wake)` ⇒ inject + continue (replay, or
    /// the live-resume target); `None` ⇒ suspend the turn here (live, fresh park).
    fn at_park(&mut self, nid: &str) -> Result<Option<SolValue>, ErrV1>;

    /// Claim an at-most-once region. Default: always `Run` (tests without a store).
    fn once_claim(&mut self, _nid: &str, _idem_key: &str) -> Result<OnceClaim, ErrV1> {
        Ok(OnceClaim::Run)
    }

    /// Record successful completion of an Once body. Default: no-op.
    fn once_complete(
        &mut self,
        _nid: &str,
        _idem_key: &str,
        _result: SolValue,
    ) -> Result<(), ErrV1> {
        Ok(())
    }

    /// L0-C ledgered nondeterminism (§9): live generates + records a `nondet_value` INJECT entry;
    /// replay injects the recorded value (§12.3). `source` ∈ {now, uuid, random}. Default: unsupported.
    fn nondet(&mut self, nid: &str, source: &str) -> Result<SolValue, ErrV1> {
        Err(ErrV1::new(
            ReasonCode::Shape,
            nid,
            format!("nondeterministic `{source}` unsupported by this backend (§9)"),
        ))
    }

    /// Registered validator dispatch for `matches_format` (§5.4). Ledgered (`validate_result`) so
    /// replay stays a pure function of the ledger. Default: unsupported.
    fn validate(
        &mut self,
        nid: &str,
        validator_id: &str,
        _value: &SolValue,
    ) -> Result<bool, ErrV1> {
        Err(ErrV1::new(
            ReasonCode::Shape,
            nid,
            format!("validator `{validator_id}` unsupported by this backend (§5.4)"),
        ))
    }

    /// Append a non-control-flow report such as `side_failed` or `finally_failed`.
    fn report(&mut self, _nid: &str, _kind: &str, _payload: SolValue) -> Result<(), ErrV1> {
        Ok(())
    }

    /// Wall-clock trips are nondeterministic: live records the decision; replay consumes it.
    fn time_exceeded(
        &mut self,
        _nid: &str,
        _kind: &str,
        _meter: &str,
        limit: u64,
        observed: u64,
    ) -> Result<bool, ErrV1> {
        Ok(observed > limit)
    }
}

#[derive(Debug)]
struct ActiveBudget {
    nid: String,
    calls: Option<u64>,
    tokens: Option<u64>,
    used_calls: u64,
    used_tokens: u64,
    used_ms: u64,
}

#[derive(Debug, Clone)]
struct ActiveEachGuard {
    nid: String,
    invariant: Expr,
}

pub struct Executor<'b, B: Backend> {
    bag: SolValue,
    /// Read-only projections visible in the current Map child but never collected into its output.
    scope: SolValue,
    active_budgets: Vec<ActiveBudget>,
    active_each_guards: Vec<ActiveEachGuard>,
    backend: &'b mut B,
}

impl<'b, B: Backend> Executor<'b, B> {
    pub fn new(initial_bag: SolValue, backend: &'b mut B) -> Self {
        Executor {
            bag: initial_bag,
            scope: SolValue::map::<_, &str>([]),
            active_budgets: Vec::new(),
            active_each_guards: Vec::new(),
            backend,
        }
    }

    /// Run a fresh turn (empty resume cursor).
    pub fn run(&mut self, program: &Node) -> Result<Outcome, ErrV1> {
        let mut cursor: VecDeque<Frame> = VecDeque::new();
        self.finish(program, &mut cursor)
    }

    /// Resume a parked turn: descend via the saved frames, inject the wake at the target park.
    pub fn resume(&mut self, program: &Node, frames: VecDeque<Frame>) -> Result<Outcome, ErrV1> {
        let mut cursor = frames;
        self.finish(program, &mut cursor)
    }

    pub fn bag(&self) -> &SolValue {
        &self.bag
    }

    fn finish(&mut self, program: &Node, cursor: &mut VecDeque<Frame>) -> Result<Outcome, ErrV1> {
        match self.exec(program, cursor)? {
            Flow::Done => Ok(Outcome::Completed {
                bag: self.bag.clone(),
            }),
            Flow::Suspend(s) => Ok(Outcome::Parked {
                suspension: s,
                bag: self.bag.clone(),
            }),
        }
    }

    fn exec(&mut self, node: &Node, cursor: &mut VecDeque<Frame>) -> Result<Flow, ErrV1> {
        let outcome = self.exec_inner(node, cursor)?;
        if matches!(outcome, Flow::Done) {
            let guards = self.active_each_guards.clone();
            for guard in guards {
                let ok = self.eval_bool(&guard.nid, &guard.invariant)?;
                self.backend.report(
                    &guard.nid,
                    "guard_check",
                    SolValue::map([
                        ("guard_nid", SolValue::str(guard.nid.clone())),
                        ("at_nid", SolValue::str(node.nid.clone())),
                        ("ok", SolValue::Bool(ok)),
                    ]),
                )?;
                if !ok {
                    return Err(ErrV1::new(
                        ReasonCode::GuardViolation,
                        &guard.nid,
                        format!("guard invariant false after {}", node.nid),
                    ));
                }
            }
        }
        Ok(outcome)
    }

    fn exec_inner(&mut self, node: &Node, cursor: &mut VecDeque<Frame>) -> Result<Flow, ErrV1> {
        let nid = &node.nid;
        match &node.kind {
            Kind::Const(v) => {
                // Sole whole-bag writer (§4.2.3): Const replaces the root bag.
                self.bag = v.clone();
                self.enforce_limits(nid)?;
                Ok(Flow::Done)
            }
            Kind::Identity => Ok(Flow::Done),

            Kind::Seq(steps) => {
                let start = match cursor.pop_front() {
                    Some(Frame::Seq(i)) => i,
                    Some(other) => return Err(frame_mismatch(nid, other)),
                    None => 0,
                };
                for (i, step) in steps.iter().enumerate().skip(start) {
                    if let Flow::Suspend(mut s) = self.exec(step, cursor)? {
                        s.push_frame(nid, Frame::Seq(i));
                        return Ok(Flow::Suspend(s));
                    }
                }
                Ok(Flow::Done)
            }

            Kind::Let { bindings, body } => {
                // Lexical scope (§4.2.7): save shadowed keys, bind, run body, unwind.
                let (mut saved, resuming) = match cursor.front() {
                    Some(Frame::Let(_)) => match cursor.pop_front() {
                        Some(Frame::Let(saved)) => (saved, true),
                        _ => unreachable!(),
                    },
                    Some(other) => return Err(frame_mismatch(nid, other.clone())),
                    None => (Vec::new(), false),
                };
                if !resuming {
                    for (key, value_expr) in bindings {
                        // Declaration order is significant: later bindings see earlier ones.
                        let val = self.eval_fx(nid, value_expr)?;
                        let p =
                            Path::parse(key).map_err(|e| shape(nid, format!("Let key: {e}")))?;
                        saved.push((key.clone(), self.bag_get(&p).cloned()));
                        self.write(nid, &p, val)?;
                    }
                }
                let out = self.exec(body, cursor)?;
                // Unwind bindings on the *completion* path only (park keeps them in the snapshot).
                if let Flow::Suspend(mut s) = out {
                    s.push_frame(nid, Frame::Let(saved));
                    return Ok(Flow::Suspend(s));
                }
                for (key, prior) in saved.into_iter().rev() {
                    let p = Path::parse(&key).unwrap();
                    match prior {
                        Some(v) => self.write(nid, &p, v)?,
                        None => self.remove(nid, &p)?,
                    }
                }
                Ok(out)
            }

            Kind::Branch { pred, then, els } => {
                let arm = match cursor.pop_front() {
                    Some(Frame::Branch(a)) => a,
                    Some(other) => return Err(frame_mismatch(nid, other)),
                    None => self.eval_bool(nid, pred)?,
                };
                let branch = if arm {
                    Some(then.as_ref())
                } else {
                    els.as_deref()
                };
                let out = match branch {
                    Some(b) => self.exec(b, cursor)?,
                    None => Flow::Done, // absent else = Identity (§8.2)
                };
                if let Flow::Suspend(mut s) = out {
                    s.push_frame(nid, Frame::Branch(arm));
                    return Ok(Flow::Suspend(s));
                }
                Ok(out)
            }

            Kind::Guard {
                invariant,
                body,
                on_violation,
                check,
            } => {
                if matches!(cursor.front(), Some(Frame::GuardHandler)) {
                    cursor.pop_front();
                    return self.run_guard_handler(nid, on_violation.as_deref(), cursor);
                }
                let resuming = matches!(cursor.front(), Some(Frame::Body));
                if resuming {
                    cursor.pop_front();
                    // §8.4: all enclosing Guards re-evaluate on resume regardless of `check`.
                    if !self.guard_is_ok(nid, invariant)? {
                        return self.run_guard_handler(
                            nid,
                            on_violation.as_deref(),
                            &mut VecDeque::new(),
                        );
                    }
                } else if matches!(check, GuardCheck::Entry | GuardCheck::Both)
                    && !self.guard_is_ok(nid, invariant)?
                {
                    return self.run_guard_handler(
                        nid,
                        on_violation.as_deref(),
                        &mut VecDeque::new(),
                    );
                }
                if matches!(check, GuardCheck::Each) {
                    self.active_each_guards.push(ActiveEachGuard {
                        nid: nid.clone(),
                        invariant: invariant.clone(),
                    });
                }
                let out = self.exec(body, cursor);
                if matches!(check, GuardCheck::Each) {
                    self.active_each_guards.pop();
                }
                let out = match out {
                    Ok(out) => out,
                    Err(error)
                        if error.code == ReasonCode::GuardViolation && error.op_serial == *nid =>
                    {
                        return self.run_guard_handler(
                            nid,
                            on_violation.as_deref(),
                            &mut VecDeque::new(),
                        );
                    }
                    Err(error) => return Err(error),
                };
                if let Flow::Suspend(mut s) = out {
                    s.push_frame(nid, Frame::Body);
                    return Ok(Flow::Suspend(s));
                }
                if matches!(check, GuardCheck::Exit | GuardCheck::Both)
                    && !self.guard_is_ok(nid, invariant)?
                {
                    return self.run_guard_handler(
                        nid,
                        on_violation.as_deref(),
                        &mut VecDeque::new(),
                    );
                }
                Ok(out)
            }

            Kind::Budget {
                body,
                calls,
                tokens,
                ms,
            } => {
                let (used_calls, used_tokens, used_ms) = match cursor.front() {
                    Some(Frame::Budget { .. }) => match cursor.pop_front() {
                        Some(Frame::Budget {
                            used_calls,
                            used_tokens,
                            used_ms,
                        }) => (used_calls, used_tokens, used_ms),
                        _ => unreachable!(),
                    },
                    Some(other) => return Err(frame_mismatch(nid, other.clone())),
                    None => (0, 0, 0),
                };
                self.active_budgets.push(ActiveBudget {
                    nid: nid.clone(),
                    calls: *calls,
                    tokens: *tokens,
                    used_calls,
                    used_tokens,
                    used_ms,
                });
                let started = std::time::Instant::now();
                let out = self.exec(body, cursor);
                let elapsed = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                let mut state = self.active_budgets.pop().expect("budget stack balanced");
                state.used_ms = state.used_ms.saturating_add(elapsed);
                if let Some(limit) = ms {
                    if self.backend.time_exceeded(
                        nid,
                        "budget_trip",
                        "ms",
                        *limit,
                        state.used_ms,
                    )? {
                        return Err(ErrV1::new(
                            ReasonCode::BudgetMs,
                            nid,
                            format!("Budget.ms exceeded: {} > {limit}", state.used_ms),
                        ));
                    }
                }
                match out? {
                    Flow::Suspend(mut suspension) => {
                        suspension.push_frame(
                            nid,
                            Frame::Budget {
                                used_calls: state.used_calls,
                                used_tokens: state.used_tokens,
                                used_ms: state.used_ms,
                            },
                        );
                        Ok(Flow::Suspend(suspension))
                    }
                    Flow::Done => Ok(Flow::Done),
                }
            }
            Kind::Timeout { body, ms } => {
                let used_ms = match cursor.front() {
                    Some(Frame::Timeout { .. }) => match cursor.pop_front() {
                        Some(Frame::Timeout { used_ms }) => used_ms,
                        _ => unreachable!(),
                    },
                    Some(other) => return Err(frame_mismatch(nid, other.clone())),
                    None => 0,
                };
                let started = std::time::Instant::now();
                let out = self.exec(body, cursor);
                let observed = used_ms
                    .saturating_add(started.elapsed().as_millis().min(u64::MAX as u128) as u64);
                if self
                    .backend
                    .time_exceeded(nid, "timeout_trip", "ms", *ms, observed)?
                {
                    return Err(ErrV1::new(
                        ReasonCode::Timeout,
                        nid,
                        format!("Timeout exceeded: {observed} > {ms}"),
                    ));
                }
                match out? {
                    Flow::Suspend(mut suspension) => {
                        suspension.push_frame(nid, Frame::Timeout { used_ms: observed });
                        Ok(Flow::Suspend(suspension))
                    }
                    Flow::Done => Ok(Flow::Done),
                }
            }

            Kind::Once { body, idem_key } => {
                // §8.4: claim via store (intent→result). Unknown outcome ⇒ Internal, never re-exec.
                let resuming = matches!(cursor.front(), Some(Frame::Body));
                if resuming {
                    cursor.pop_front();
                }
                let key = self.once_key(nid, idem_key, body)?;
                // Claim only on first entry; resume continues the same claim. Complete after body
                // finishes (including post-park resume of a body that never completed).
                if !resuming {
                    match self.backend.once_claim(nid, &key)? {
                        OnceClaim::Skip(result) => {
                            self.apply_once_result(nid, result)?;
                            return Ok(Flow::Done);
                        }
                        OnceClaim::Run => {}
                    }
                }
                let out = self.exec(body, cursor)?;
                if let Flow::Suspend(mut s) = out {
                    s.push_frame(nid, Frame::Body);
                    return Ok(Flow::Suspend(s));
                }
                let result = self.capture_once_result(body);
                self.backend.once_complete(nid, &key, result)?;
                Ok(out)
            }

            Kind::Park { until, into } => {
                if cursor.is_empty() {
                    // At the park with no more frames: ask the backend whether to wake or suspend.
                    match self.backend.at_park(nid)? {
                        Some(wake) => {
                            if let Some(p) = into {
                                self.write(nid, p, wake)?;
                            }
                            Ok(Flow::Done)
                        }
                        None => Ok(Flow::Suspend(Suspend {
                            park_nid: nid.clone(),
                            until: until.clone(),
                            into: into.clone(),
                            frames: VecDeque::new(),
                            frame_nids: VecDeque::new(),
                        })),
                    }
                } else {
                    // Frames remain — this park is on the descent path but not the target; shouldn't
                    // happen for a well-formed continuation.
                    Err(ErrV1::new(
                        ReasonCode::Internal,
                        nid,
                        "unexpected frames at park",
                    ))
                }
            }

            Kind::Call { id, args, into } => {
                self.charge_call()?;
                let projected = self.project_args(nid, args)?;
                let output = self.backend.call(nid, id, projected)?;
                self.charge_tokens(output.usage_tokens)?;
                self.write(nid, into, output.value)?;
                Ok(Flow::Done)
            }

            // ── ops present for closure; simple v0 (no cross-turn resume through them in the golden
            //    flow). Each executes its body/elements sequentially. ──
            Kind::Try {
                body,
                catch,
                err_into,
                finally,
            } => {
                let phase = match cursor.front() {
                    Some(Frame::TryBody) => {
                        cursor.pop_front();
                        Some(Frame::TryBody)
                    }
                    Some(Frame::TryHandler { .. }) | Some(Frame::TryFinally(_)) => {
                        cursor.pop_front()
                    }
                    Some(other) => return Err(frame_mismatch(nid, other.clone())),
                    None => None,
                };

                let mut pending_error = None;
                let resuming_finally = matches!(phase, Some(Frame::TryFinally(_)));
                match phase {
                    Some(Frame::TryFinally(error)) => pending_error = error,
                    Some(Frame::TryHandler { index, prefix }) => {
                        let Some((declared_prefix, handler)) = catch.get(index) else {
                            return Err(ErrV1::new(
                                ReasonCode::Shape,
                                nid,
                                "continuation Try handler index is out of bounds",
                            ));
                        };
                        if declared_prefix != &prefix {
                            return Err(ErrV1::new(
                                ReasonCode::Shape,
                                nid,
                                "continuation Try handler prefix does not match pinned flow",
                            ));
                        }
                        match self.exec(handler, cursor) {
                            Ok(Flow::Suspend(mut suspension)) => {
                                suspension.push_frame(nid, Frame::TryHandler { index, prefix });
                                return Ok(Flow::Suspend(suspension));
                            }
                            Ok(Flow::Done) => {}
                            Err(error) => pending_error = Some(error),
                        }
                    }
                    None | Some(Frame::TryBody) => match self.exec(body, cursor) {
                        Ok(Flow::Suspend(mut suspension)) => {
                            suspension.push_frame(nid, Frame::TryBody);
                            return Ok(Flow::Suspend(suspension));
                        }
                        Ok(Flow::Done) => {}
                        Err(error) => {
                            if let Some(index) = best_catch_index(catch, &error.code) {
                                self.write(nid, err_into, error.to_sol())?;
                                match self.exec(&catch[index].1, &mut VecDeque::new()) {
                                    Ok(Flow::Suspend(mut suspension)) => {
                                        suspension.push_frame(
                                            nid,
                                            Frame::TryHandler {
                                                index,
                                                prefix: catch[index].0.clone(),
                                            },
                                        );
                                        return Ok(Flow::Suspend(suspension));
                                    }
                                    Ok(Flow::Done) => {}
                                    Err(handler_error) => pending_error = Some(handler_error),
                                }
                            } else {
                                pending_error = Some(error);
                            }
                        }
                    },
                    Some(other) => return Err(frame_mismatch(nid, other)),
                }

                if let Some(fin) = finally {
                    let final_cursor = if resuming_finally {
                        cursor
                    } else {
                        &mut VecDeque::new()
                    };
                    match self.exec(fin, final_cursor) {
                        Ok(Flow::Suspend(mut suspension)) => {
                            suspension.push_frame(nid, Frame::TryFinally(pending_error));
                            return Ok(Flow::Suspend(suspension));
                        }
                        Ok(Flow::Done) => {}
                        Err(finally_error) => {
                            if let Some(original) = pending_error {
                                self.backend.report(
                                    nid,
                                    "finally_failed",
                                    SolValue::map([
                                        ("scope_nid", SolValue::str(nid)),
                                        ("err", finally_error.to_sol()),
                                    ]),
                                )?;
                                return Err(original);
                            }
                            return Err(finally_error);
                        }
                    }
                }
                pending_error.map_or(Ok(Flow::Done), Err)
            }
            Kind::Fallback(steps) => {
                let (start, mut prior_errors, resuming) = match cursor.front() {
                    Some(Frame::Fallback { .. }) => match cursor.pop_front() {
                        Some(Frame::Fallback {
                            step_index,
                            prior_errors,
                        }) => (step_index, prior_errors, true),
                        _ => unreachable!(),
                    },
                    Some(other) => return Err(frame_mismatch(nid, other.clone())),
                    None => (0, Vec::new(), false),
                };
                for (index, step) in steps.iter().enumerate().skip(start) {
                    let step_cursor = if resuming && index == start {
                        &mut *cursor
                    } else {
                        &mut VecDeque::new()
                    };
                    match self.exec(step, step_cursor) {
                        Ok(Flow::Done) => return Ok(Flow::Done),
                        Ok(Flow::Suspend(mut suspension)) => {
                            suspension.push_frame(
                                nid,
                                Frame::Fallback {
                                    step_index: index,
                                    prior_errors,
                                },
                            );
                            return Ok(Flow::Suspend(suspension));
                        }
                        Err(error) => prior_errors.push(error),
                    }
                }
                let mut last = prior_errors
                    .pop()
                    .unwrap_or_else(|| ErrV1::new(ReasonCode::Internal, nid, "empty fallback"));
                while let Some(prior) = prior_errors.pop() {
                    last = last.with_cause(prior);
                }
                Err(last)
            }
            Kind::Loop {
                while_,
                body,
                max_iter,
            } => {
                let (mut completed, resuming) = match cursor.front() {
                    Some(Frame::Loop(_)) => match cursor.pop_front() {
                        Some(Frame::Loop(iter)) => (iter, true),
                        _ => unreachable!(),
                    },
                    Some(other) => return Err(frame_mismatch(nid, other.clone())),
                    None => (0, false),
                };

                // A Loop frame means the predicate for this iteration already passed. Resume the
                // suspended body first, then return to ordinary pre-test iteration.
                if resuming && completed < *max_iter {
                    if let Flow::Suspend(mut s) = self.exec(body, cursor)? {
                        s.push_frame(nid, Frame::Loop(completed));
                        return Ok(Flow::Suspend(s));
                    }
                    completed += 1;
                }

                while completed < *max_iter {
                    if !self.eval_bool(nid, while_)? {
                        return Ok(Flow::Done);
                    }
                    if let Flow::Suspend(mut s) = self.exec(body, &mut VecDeque::new())? {
                        s.push_frame(nid, Frame::Loop(completed));
                        return Ok(Flow::Suspend(s));
                    }
                    completed += 1;
                }
                // §8.3: exhaustion raises Budget.Iter, never a silent exit.
                if self.eval_bool(nid, while_)? {
                    return Err(ErrV1::new(
                        ReasonCode::BudgetIter,
                        nid,
                        "Loop max_iter exhausted (§8.3)",
                    ));
                }
                Ok(Flow::Done)
            }
            Kind::Map {
                over,
                imports,
                body,
                into,
                max_items,
            } => {
                let resume_state = match cursor.front() {
                    Some(Frame::Map { .. }) => match cursor.pop_front() {
                        Some(Frame::Map {
                            element_index,
                            collected,
                            parent_bag,
                            parent_scope,
                        }) => Some((element_index, collected, parent_bag, parent_scope)),
                        _ => unreachable!(),
                    },
                    Some(other) => return Err(frame_mismatch(nid, other.clone())),
                    None => None,
                };
                let (mut index, mut collected, parent_bag, parent_scope, resume_child) =
                    if let Some((index, collected, parent_bag, parent_scope)) = resume_state {
                        (index, collected, parent_bag, parent_scope, true)
                    } else {
                        (0, Vec::new(), self.bag.clone(), self.scope.clone(), false)
                    };
                let child_scope = build_import_scope(nid, &parent_bag, imports)?;
                let items = over
                    .get(&parent_bag)
                    .and_then(SolValue::as_list)
                    .map(<[_]>::to_vec)
                    .ok_or_else(|| {
                        ErrV1::new(
                            ReasonCode::Type,
                            nid,
                            "Map.over must resolve to a list (§6.3)",
                        )
                    })?;
                if items.len() as u64 > *max_items {
                    return Err(ErrV1::new(
                        ReasonCode::BudgetIter,
                        nid,
                        "Map exceeds max_items (§8.3)",
                    ));
                }

                // On resume, `self.bag` is the suspended child. Finish it before advancing.
                if resume_child {
                    self.scope = child_scope.clone();
                    match self.exec(body, cursor)? {
                        Flow::Done => {
                            collected.push(self.bag.clone());
                            index += 1;
                        }
                        Flow::Suspend(mut suspension) => {
                            suspension.push_frame(
                                nid,
                                Frame::Map {
                                    element_index: index,
                                    collected,
                                    parent_bag,
                                    parent_scope,
                                },
                            );
                            return Ok(Flow::Suspend(suspension));
                        }
                    }
                }

                while index < items.len() {
                    self.bag = items[index].clone();
                    for (alias, _) in imports {
                        let alias = Path::parse(alias)
                            .map_err(|e| shape(nid, format!("Map import alias: {e}")))?;
                        if alias.exists(&self.bag) {
                            return Err(ErrV1::new(
                                ReasonCode::Shape,
                                nid,
                                "Map import alias collides with an element path (§6.3)",
                            ));
                        }
                    }
                    self.scope = child_scope.clone();
                    match self.exec(body, &mut VecDeque::new())? {
                        Flow::Done => {
                            collected.push(self.bag.clone());
                            index += 1;
                        }
                        Flow::Suspend(mut suspension) => {
                            suspension.push_frame(
                                nid,
                                Frame::Map {
                                    element_index: index,
                                    collected,
                                    parent_bag,
                                    parent_scope,
                                },
                            );
                            return Ok(Flow::Suspend(suspension));
                        }
                    }
                }
                self.bag = parent_bag;
                self.scope = parent_scope;
                self.write(nid, into, SolValue::List(collected))?;
                Ok(Flow::Done)
            }
            Kind::Filter {
                over,
                pred,
                into,
                max_items,
            } => {
                let items = self
                    .bag_get(over)
                    .and_then(SolValue::as_list)
                    .map(<[_]>::to_vec)
                    .ok_or_else(|| {
                        ErrV1::new(
                            ReasonCode::Type,
                            nid,
                            "Filter.over must resolve to a list (§6.3)",
                        )
                    })?;
                if items.len() as u64 > *max_items {
                    return Err(ErrV1::new(
                        ReasonCode::BudgetIter,
                        nid,
                        "Filter exceeds max_items",
                    ));
                }
                let mut kept = Vec::new();
                for item in items {
                    let child = Executor::new(item.clone(), self.backend);
                    if child.eval_bool(nid, pred)? {
                        kept.push(item);
                    }
                }
                self.write(nid, into, SolValue::List(kept))?;
                Ok(Flow::Done)
            }
            Kind::Tee {
                body,
                side,
                side_root,
            } => {
                let out = self.exec(body, cursor)?;
                if let Flow::Suspend(mut s) = out {
                    s.push_frame(nid, Frame::Body);
                    return Ok(Flow::Suspend(s));
                }
                // Fire-and-record: side runs; failure is a report, main continues (§8.2).
                if let Err(error) = self.exec(side, &mut VecDeque::new()) {
                    self.backend.report(
                        nid,
                        "side_failed",
                        SolValue::map([("scope_nid", SolValue::str(nid)), ("err", error.to_sol())]),
                    )?;
                }
                let _ = side_root;
                Ok(out)
            }
        }
    }

    // ── expression evaluation ────────────────────────────────────────────────────────────────
    fn eval(&self, nid: &str, expr: &Expr) -> Result<SolValue, ErrV1> {
        match expr {
            Expr::Lit(v) => Ok(v.clone()),
            Expr::Pull(p) => self
                .bag_get(p)
                .cloned()
                .ok_or_else(|| ErrV1::new(ReasonCode::Missing, nid, "pull: path not present")),
            Expr::Fn { op, args } => {
                if op == "exists" {
                    // `exists` inspects presence without erroring on absence (§9 structure).
                    if let [Expr::Pull(p)] = args.as_slice() {
                        return Ok(SolValue::Bool(p.exists(self.bag_root())));
                    }
                    return Err(shape(nid, "exists expects a single pull(path) arg"));
                }
                let vals: Vec<SolValue> = args
                    .iter()
                    .map(|a| self.eval(nid, a))
                    .collect::<Result<_, _>>()?;
                compute::apply(op, &vals).map_err(|e| ErrV1::new(e.code, nid, e.detail))
            }
        }
    }

    /// Effectful expression eval for **value-producing** positions (Let bindings, Call args). Resolves
    /// the L0-C nondeterministic sources (`now`/`uuid`/`random`) and `matches_format` through the
    /// backend (both ledgered); all pure ops delegate to [`compute::apply`]. Determinism-sensitive
    /// positions (predicates, idem keys) use the pure [`Self::eval`], which rejects these ops.
    fn eval_fx(&mut self, nid: &str, expr: &Expr) -> Result<SolValue, ErrV1> {
        match expr {
            Expr::Lit(v) => Ok(v.clone()),
            Expr::Pull(p) => self
                .bag_get(p)
                .cloned()
                .ok_or_else(|| ErrV1::new(ReasonCode::Missing, nid, "pull: path not present")),
            Expr::Fn { op, args } => match op.as_str() {
                "now" | "uuid" | "random" if args.is_empty() => self.backend.nondet(nid, op),
                "now" | "uuid" | "random" => Err(shape(nid, format!("{op} takes no args (§9)"))),
                "exists" => {
                    if let [Expr::Pull(p)] = args.as_slice() {
                        Ok(SolValue::Bool(p.exists(self.bag_root())))
                    } else {
                        Err(shape(nid, "exists expects a single pull(path) arg"))
                    }
                }
                "matches_format" => {
                    let val_expr = args
                        .first()
                        .ok_or_else(|| shape(nid, "matches_format(value, validator_id)"))?;
                    let val = self.eval_fx(nid, val_expr)?;
                    let vid = match args.get(1) {
                        Some(Expr::Lit(SolValue::Str(s))) => s.clone(),
                        _ => {
                            return Err(shape(
                                nid,
                                "matches_format validator_id must be a string literal (§5.4)",
                            ))
                        }
                    };
                    Ok(SolValue::Bool(self.backend.validate(nid, &vid, &val)?))
                }
                _ => {
                    let mut vals = Vec::with_capacity(args.len());
                    for a in args {
                        vals.push(self.eval_fx(nid, a)?);
                    }
                    compute::apply(op, &vals).map_err(|e| ErrV1::new(e.code, nid, e.detail))
                }
            },
        }
    }

    fn eval_bool(&self, nid: &str, expr: &Expr) -> Result<bool, ErrV1> {
        match self.eval(nid, expr)? {
            SolValue::Bool(b) => Ok(b),
            _ => Err(ErrV1::new(
                ReasonCode::Type,
                nid,
                "predicate must be bool (§8.2)",
            )),
        }
    }

    fn guard_is_ok(&self, nid: &str, invariant: &Expr) -> Result<bool, ErrV1> {
        self.eval_bool(nid, invariant)
    }

    fn run_guard_handler(
        &mut self,
        nid: &str,
        handler: Option<&Node>,
        cursor: &mut VecDeque<Frame>,
    ) -> Result<Flow, ErrV1> {
        let Some(handler) = handler else {
            return Err(ErrV1::new(
                ReasonCode::GuardViolation,
                nid,
                "guard invariant false (§8.3)",
            ));
        };
        match self.exec(handler, cursor)? {
            Flow::Done => Ok(Flow::Done),
            Flow::Suspend(mut suspension) => {
                suspension.push_frame(nid, Frame::GuardHandler);
                Ok(Flow::Suspend(suspension))
            }
        }
    }

    /// Default idem_key = blake3(nid ‖ canonical(bag projection of template or empty)).
    /// Override template: each Expr evaluates to a Sol fragment folded into the key material.
    fn once_key(
        &self,
        nid: &str,
        template: &Option<Vec<Expr>>,
        body: &Node,
    ) -> Result<String, ErrV1> {
        let mut parts = vec![SolValue::str(nid)];
        if let Some(exprs) = template {
            for e in exprs {
                parts.push(self.eval(nid, e)?);
            }
        } else {
            // Default identity includes the body's statically-derived read-set projection. A
            // corrected input therefore receives a distinct claim while a duplicate submit
            // reuses the prior result (§8.4).
            let projection = crate::waves::rw_set(body)
                .reads
                .into_iter()
                .map(|path| {
                    (
                        path.to_string(),
                        path.get(&self.bag).cloned().unwrap_or(SolValue::Null),
                    )
                })
                .collect::<Vec<_>>();
            parts.push(SolValue::map(projection));
        }
        Ok(aelio_sol::value_hash(&SolValue::List(parts)))
    }

    /// §4.2.4 / §6.3: build the callee input from `args` only — least privilege. Uses the effectful
    /// evaluator so a Call arg may carry an L0-C source (`uuid()`) or `matches_format` (both ledgered).
    fn project_args(&mut self, nid: &str, args: &[(String, Expr)]) -> Result<SolValue, ErrV1> {
        let mut pairs = Vec::new();
        for (slot, expr) in args {
            pairs.push((slot.clone(), self.eval_fx(nid, expr)?));
        }
        Ok(SolValue::map(pairs))
    }

    fn bag_get(&self, path: &Path) -> Option<&SolValue> {
        path.get(&self.bag).or_else(|| path.get(&self.scope))
    }
    fn bag_root(&self) -> &SolValue {
        &self.bag
    }
    fn write(&mut self, nid: &str, path: &Path, value: SolValue) -> Result<(), ErrV1> {
        set_path(&mut self.bag, path, value).map_err(|e| ErrV1::new(ReasonCode::Shape, nid, e))?;
        self.enforce_limits(nid)
    }

    fn remove(&mut self, nid: &str, path: &Path) -> Result<(), ErrV1> {
        let mut bag = crate::bag::Bag::from_value(std::mem::replace(&mut self.bag, SolValue::Null));
        let result = bag
            .remove(path)
            .map_err(|e| ErrV1::new(ReasonCode::Shape, nid, e));
        self.bag = bag.value().clone();
        result?;
        self.enforce_limits(nid)
    }

    fn charge_call(&mut self) -> Result<(), ErrV1> {
        let violation = self.active_budgets.iter().find_map(|budget| {
            budget
                .calls
                .and_then(|limit| (budget.used_calls >= limit).then(|| (budget.nid.clone(), limit)))
        });
        if let Some((scope_nid, limit)) = violation {
            self.backend.report(
                &scope_nid,
                "budget_trip",
                SolValue::map([
                    ("scope_nid", SolValue::str(scope_nid.clone())),
                    ("meter", SolValue::str("calls")),
                    ("limit", SolValue::Int(limit as i64)),
                    ("observed", SolValue::Int((limit + 1) as i64)),
                ]),
            )?;
            return Err(ErrV1::new(
                ReasonCode::BudgetCalls,
                scope_nid,
                "Budget.calls exceeded",
            ));
        }
        for budget in &mut self.active_budgets {
            budget.used_calls = budget.used_calls.saturating_add(1);
        }
        Ok(())
    }

    fn charge_tokens(&mut self, tokens: u64) -> Result<(), ErrV1> {
        let violation = self.active_budgets.iter().find_map(|budget| {
            budget.tokens.and_then(|limit| {
                let observed = budget.used_tokens.saturating_add(tokens);
                (observed > limit).then(|| (budget.nid.clone(), limit, observed))
            })
        });
        if let Some((scope_nid, limit, observed)) = violation {
            self.backend.report(
                &scope_nid,
                "budget_trip",
                SolValue::map([
                    ("scope_nid", SolValue::str(scope_nid.clone())),
                    ("meter", SolValue::str("tokens")),
                    ("limit", SolValue::Int(limit as i64)),
                    ("observed", SolValue::Int(observed as i64)),
                ]),
            )?;
            return Err(ErrV1::new(
                ReasonCode::BudgetTokens,
                scope_nid,
                "Budget.tokens exceeded",
            ));
        }
        for budget in &mut self.active_budgets {
            budget.used_tokens = budget.used_tokens.saturating_add(tokens);
        }
        Ok(())
    }

    fn capture_once_result(&self, body: &Node) -> SolValue {
        let writes = crate::waves::rw_set(body).writes;
        SolValue::map([
            ("whole", SolValue::Bool(contains_const(body))),
            ("bag", self.bag.clone()),
            (
                "writes",
                SolValue::map(writes.into_iter().map(|path| {
                    (
                        path.to_string(),
                        path.get(&self.bag).cloned().unwrap_or(SolValue::Null),
                    )
                })),
            ),
        ])
    }

    fn apply_once_result(&mut self, nid: &str, result: SolValue) -> Result<(), ErrV1> {
        let map = result.as_map().ok_or_else(|| {
            ErrV1::new(ReasonCode::Internal, nid, "stored Once result is malformed")
        })?;
        if map.get("whole") == Some(&SolValue::Bool(true)) {
            self.bag = map.get("bag").cloned().ok_or_else(|| {
                ErrV1::new(ReasonCode::Internal, nid, "stored Once bag is missing")
            })?;
            return self.enforce_limits(nid);
        }
        let writes = map
            .get("writes")
            .and_then(SolValue::as_map)
            .ok_or_else(|| {
                ErrV1::new(ReasonCode::Internal, nid, "stored Once writes are missing")
            })?;
        for (path, value) in writes {
            let path = Path::parse(path)
                .map_err(|error| ErrV1::new(ReasonCode::Internal, nid, error.to_string()))?;
            self.write(nid, &path, value.clone())?;
        }
        Ok(())
    }

    /// §4.4: after every commit the bag must satisfy the structural caps (depth 32, 1024 keys/map,
    /// 10k list len, 1 MiB canonical). Violations map to `Budget.Size` (§11). Enforced on every
    /// write so the bag invariant holds at all boundaries (persistence/Call/Park, §4.1).
    fn enforce_limits(&self, nid: &str) -> Result<(), ErrV1> {
        aelio_sol::Limits::default().check(&self.bag).map_err(|e| {
            ErrV1::new(
                ReasonCode::BudgetSize,
                nid,
                format!("§4.4 limit exceeded: {e}"),
            )
        })
    }
}

fn set_path(root: &mut SolValue, path: &Path, value: SolValue) -> Result<(), &'static str> {
    let mut bag = crate::bag::Bag::from_value(std::mem::replace(root, SolValue::Null));
    let r = bag.set(path, value);
    *root = bag.value().clone();
    r
}

fn build_import_scope(
    nid: &str,
    parent: &SolValue,
    imports: &[(String, Path)],
) -> Result<SolValue, ErrV1> {
    let mut scope = SolValue::map::<_, &str>([]);
    for (alias, source) in imports {
        let value = source.get(parent).cloned().ok_or_else(|| {
            ErrV1::new(
                ReasonCode::Missing,
                nid,
                format!("Map import source `{source:?}` is missing (§6.3)"),
            )
        })?;
        let alias = Path::parse(alias).map_err(|e| shape(nid, format!("Map import alias: {e}")))?;
        set_path(&mut scope, &alias, value).map_err(|e| ErrV1::new(ReasonCode::Shape, nid, e))?;
    }
    Ok(scope)
}

fn contains_const(node: &Node) -> bool {
    match &node.kind {
        Kind::Const(_) => true,
        Kind::Seq(children) | Kind::Fallback(children) => children.iter().any(contains_const),
        Kind::Let { body, .. }
        | Kind::Loop { body, .. }
        | Kind::Guard { body, .. }
        | Kind::Budget { body, .. }
        | Kind::Timeout { body, .. }
        | Kind::Once { body, .. }
        | Kind::Map { body, .. } => contains_const(body),
        Kind::Branch { then, els, .. } => {
            contains_const(then) || els.as_deref().is_some_and(contains_const)
        }
        Kind::Try {
            body,
            catch,
            finally,
            ..
        } => {
            contains_const(body)
                || catch.iter().any(|(_, child)| contains_const(child))
                || finally.as_deref().is_some_and(contains_const)
        }
        Kind::Tee { body, side, .. } => contains_const(body) || contains_const(side),
        _ => false,
    }
}

fn best_catch_index(catch: &[(String, Node)], code: &ReasonCode) -> Option<usize> {
    let full = code.code();
    catch
        .iter()
        .enumerate()
        .filter(|(_, (prefix, _))| {
            full == prefix.as_str() || full.starts_with(&format!("{prefix}."))
        })
        .max_by_key(|(_, (prefix, _))| prefix.len())
        .map(|(index, _)| index)
}

fn shape(nid: &str, why: impl Into<String>) -> ErrV1 {
    ErrV1::new(ReasonCode::Shape, nid, why)
}
fn frame_mismatch(nid: &str, got: Frame) -> ErrV1 {
    ErrV1::new(
        ReasonCode::Internal,
        nid,
        format!("resume frame mismatch: {got:?}"),
    )
}
