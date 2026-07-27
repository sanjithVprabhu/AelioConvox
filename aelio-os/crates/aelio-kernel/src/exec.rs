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
}

/// Per-scope resume position (App I.1, minimal set covering the login flow).
#[derive(Debug, Clone)]
pub enum Frame {
    Seq(usize),
    Branch(bool),
    /// Generic single-body wrapper (Guard/Let/Budget/Timeout/Once/Try.body).
    Body,
}

/// What the executor exposes to a driver.
pub enum Outcome {
    Completed { bag: SolValue },
    Parked { suspension: Suspend, bag: SolValue },
}

/// Result of claiming an `Once` region (§8.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnceClaim {
    /// Execute the body, then [`Backend::once_complete`].
    Run,
    /// Prior successful completion — skip body (bag already carries effects).
    Skip,
}

/// Effect + park behavior, differing live vs replay.
pub trait Backend {
    /// Invoke a Call target (live) or inject its recorded result (replay). `args` is the projected
    /// least-privilege input (§4.2.4). The backend owns the registry, so it classifies the effect
    /// (§10.1) and ledgers intent/result accordingly (§12.4).
    fn call(&mut self, nid: &str, id: &str, args: SolValue) -> Result<SolValue, ErrV1>;

    /// At a park reached with an empty resume cursor: `Some(wake)` ⇒ inject + continue (replay, or
    /// the live-resume target); `None` ⇒ suspend the turn here (live, fresh park).
    fn at_park(&mut self, nid: &str) -> Result<Option<SolValue>, ErrV1>;

    /// Claim an at-most-once region. Default: always `Run` (tests without a store).
    fn once_claim(&mut self, _nid: &str, _idem_key: &str) -> Result<OnceClaim, ErrV1> {
        Ok(OnceClaim::Run)
    }

    /// Record successful completion of an Once body. Default: no-op.
    fn once_complete(&mut self, _nid: &str, _idem_key: &str) -> Result<(), ErrV1> {
        Ok(())
    }
}

pub struct Executor<'b, B: Backend> {
    bag: SolValue,
    backend: &'b mut B,
}

impl<'b, B: Backend> Executor<'b, B> {
    pub fn new(initial_bag: SolValue, backend: &'b mut B) -> Self {
        Executor { bag: initial_bag, backend }
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

    fn finish(&mut self, program: &Node, cursor: &mut VecDeque<Frame>) -> Result<Outcome, ErrV1> {
        match self.exec(program, cursor)? {
            Flow::Done => Ok(Outcome::Completed { bag: self.bag.clone() }),
            Flow::Suspend(s) => Ok(Outcome::Parked { suspension: s, bag: self.bag.clone() }),
        }
    }

    fn exec(&mut self, node: &Node, cursor: &mut VecDeque<Frame>) -> Result<Flow, ErrV1> {
        let nid = &node.nid;
        match &node.kind {
            Kind::Const(v) => {
                // Sole whole-bag writer (§4.2.3): Const replaces the root bag.
                self.bag = v.clone();
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
                        s.frames.push_front(Frame::Seq(i));
                        return Ok(Flow::Suspend(s));
                    }
                }
                Ok(Flow::Done)
            }

            Kind::Let { bindings, body } => {
                // Lexical scope (§4.2.7): save shadowed keys, bind, run body, unwind.
                let resuming = matches!(cursor.front(), Some(Frame::Body));
                if resuming {
                    cursor.pop_front();
                }
                let mut saved: Vec<(String, Option<SolValue>)> = Vec::new();
                if !resuming {
                    for (key, value_expr) in bindings {
                        let val = self.eval(nid, value_expr)?;
                        let p = Path::parse(key).map_err(|e| shape(nid, format!("Let key: {e}")))?;
                        saved.push((key.clone(), self.bag_get(&p).cloned()));
                        self.write(nid, &p, val)?;
                    }
                }
                let out = self.exec(body, cursor)?;
                // Unwind bindings on the *completion* path only (park keeps them in the snapshot).
                if let Flow::Suspend(mut s) = out {
                    s.frames.push_front(Frame::Body);
                    return Ok(Flow::Suspend(s));
                }
                for (key, prior) in saved.into_iter().rev() {
                    let p = Path::parse(&key).unwrap();
                    match prior {
                        Some(v) => self.write(nid, &p, v)?,
                        None => { /* no implicit deletion in v0 golden flow */ }
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
                let branch = if arm { Some(then.as_ref()) } else { els.as_deref() };
                let out = match branch {
                    Some(b) => self.exec(b, cursor)?,
                    None => Flow::Done, // absent else = Identity (§8.2)
                };
                if let Flow::Suspend(mut s) = out {
                    s.frames.push_front(Frame::Branch(arm));
                    return Ok(Flow::Suspend(s));
                }
                Ok(out)
            }

            Kind::Guard { invariant, body, on_violation, check } => {
                let resuming = matches!(cursor.front(), Some(Frame::Body));
                if resuming {
                    cursor.pop_front();
                    // §8.4: all enclosing Guards re-evaluate on resume regardless of `check`.
                    self.check_guard(nid, invariant, on_violation.as_deref())?;
                } else {
                    if matches!(check, GuardCheck::Entry | GuardCheck::Both) {
                        self.check_guard(nid, invariant, on_violation.as_deref())?;
                    }
                }
                let out = self.exec(body, cursor)?;
                if let Flow::Suspend(mut s) = out {
                    s.frames.push_front(Frame::Body);
                    return Ok(Flow::Suspend(s));
                }
                if matches!(check, GuardCheck::Exit | GuardCheck::Both) {
                    self.check_guard(nid, invariant, on_violation.as_deref())?;
                }
                Ok(out)
            }

            Kind::Budget { body, .. } | Kind::Timeout { body, .. } => {
                // v0: meters recorded per §8.4; golden flow doesn't park inside a budget, so the
                // used-counter continuation (App I) isn't exercised. Body runs; a park propagates.
                let resuming = matches!(cursor.front(), Some(Frame::Body));
                if resuming {
                    cursor.pop_front();
                }
                let out = self.exec(body, cursor)?;
                if let Flow::Suspend(mut s) = out {
                    s.frames.push_front(Frame::Body);
                    return Ok(Flow::Suspend(s));
                }
                Ok(out)
            }

            Kind::Once { body, idem_key } => {
                // §8.4: claim via store (intent→result). Unknown outcome ⇒ Internal, never re-exec.
                let resuming = matches!(cursor.front(), Some(Frame::Body));
                if resuming {
                    cursor.pop_front();
                }
                let key = self.once_key(nid, idem_key)?;
                // Claim only on first entry; resume continues the same claim. Complete after body
                // finishes (including post-park resume of a body that never completed).
                if !resuming {
                    match self.backend.once_claim(nid, &key)? {
                        OnceClaim::Skip => return Ok(Flow::Done),
                        OnceClaim::Run => {}
                    }
                }
                let out = self.exec(body, cursor)?;
                if let Flow::Suspend(mut s) = out {
                    s.frames.push_front(Frame::Body);
                    return Ok(Flow::Suspend(s));
                }
                self.backend.once_complete(nid, &key)?;
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
                        })),
                    }
                } else {
                    // Frames remain — this park is on the descent path but not the target; shouldn't
                    // happen for a well-formed continuation.
                    Err(ErrV1::new(ReasonCode::Internal, nid, "unexpected frames at park"))
                }
            }

            Kind::Call { id, args, into } => {
                let projected = self.project_args(nid, args)?;
                let output = self.backend.call(nid, id, projected)?;
                self.write(nid, into, output)?;
                Ok(Flow::Done)
            }

            // ── ops present for closure; simple v0 (no cross-turn resume through them in the golden
            //    flow). Each executes its body/elements sequentially. ──
            Kind::Try { body, catch, err_into, finally } => {
                let result = self.exec(body, cursor);
                let out = match result {
                    Ok(Flow::Suspend(mut s)) => {
                        s.frames.push_front(Frame::Body);
                        return Ok(Flow::Suspend(s));
                    }
                    Ok(done) => Ok(done),
                    Err(e) => {
                        // Match a catch by deepest code prefix (§11.2).
                        if let Some((_, handler)) = best_catch(catch, &e.code) {
                            self.write(nid, err_into, e.to_sol())?;
                            self.exec(handler, &mut VecDeque::new())
                        } else {
                            Err(e)
                        }
                    }
                };
                if let Some(fin) = finally {
                    // finally runs on success and handled/unhandled error; original error wins.
                    let _ = self.exec(fin, &mut VecDeque::new());
                }
                out
            }
            Kind::Fallback(steps) => {
                let mut last = ErrV1::new(ReasonCode::Internal, nid, "empty fallback");
                for step in steps {
                    match self.exec(step, &mut VecDeque::new()) {
                        Ok(done) => return Ok(done),
                        Err(e) => last = e,
                    }
                }
                Err(last)
            }
            Kind::Loop { while_, body, max_iter } => {
                for _ in 0..*max_iter {
                    if !self.eval_bool(nid, while_)? {
                        return Ok(Flow::Done);
                    }
                    if let Flow::Suspend(_) = self.exec(body, &mut VecDeque::new())? {
                        return Err(ErrV1::new(ReasonCode::Internal, nid, "park inside Loop unsupported in v0 walker"));
                    }
                }
                // §8.3: exhaustion raises Budget.Iter, never a silent exit.
                if self.eval_bool(nid, while_)? {
                    return Err(ErrV1::new(ReasonCode::BudgetIter, nid, "Loop max_iter exhausted (§8.3)"));
                }
                Ok(Flow::Done)
            }
            Kind::Map { over, body, into, max_items, .. } => {
                let items = self.bag_get(over).and_then(SolValue::as_list).map(<[_]>::to_vec).unwrap_or_default();
                if items.len() as u64 > *max_items {
                    return Err(ErrV1::new(ReasonCode::BudgetIter, nid, "Map exceeds max_items (§8.3)"));
                }
                let mut collected = Vec::new();
                for item in items {
                    // Child bag IS the element (§6.3). Golden flow doesn't use Map; simple v0.
                    let mut child = Executor::new(item, self.backend);
                    match child.exec(body, &mut VecDeque::new())? {
                        Flow::Done => collected.push(child.bag.clone()),
                        Flow::Suspend(_) => return Err(ErrV1::new(ReasonCode::Internal, nid, "park inside Map unsupported in v0 walker")),
                    }
                }
                self.write(nid, into, SolValue::List(collected))?;
                Ok(Flow::Done)
            }
            Kind::Filter { over, pred, into, max_items } => {
                let items = self.bag_get(over).and_then(SolValue::as_list).map(<[_]>::to_vec).unwrap_or_default();
                if items.len() as u64 > *max_items {
                    return Err(ErrV1::new(ReasonCode::BudgetIter, nid, "Filter exceeds max_items"));
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
            Kind::Tee { body, side, side_root } => {
                let out = self.exec(body, cursor)?;
                if let Flow::Suspend(mut s) = out {
                    s.frames.push_front(Frame::Body);
                    return Ok(Flow::Suspend(s));
                }
                // Fire-and-record: side runs; failure is a report, main continues (§8.2).
                let _ = self.exec(side, &mut VecDeque::new());
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
                let vals: Vec<SolValue> = args.iter().map(|a| self.eval(nid, a)).collect::<Result<_, _>>()?;
                compute::apply(op, &vals).map_err(|e| ErrV1::new(e.code, nid, e.detail))
            }
        }
    }

    fn eval_bool(&self, nid: &str, expr: &Expr) -> Result<bool, ErrV1> {
        match self.eval(nid, expr)? {
            SolValue::Bool(b) => Ok(b),
            _ => Err(ErrV1::new(ReasonCode::Type, nid, "predicate must be bool (§8.2)")),
        }
    }

    fn check_guard(&mut self, nid: &str, invariant: &Expr, _on_violation: Option<&Node>) -> Result<(), ErrV1> {
        if self.eval_bool(nid, invariant)? {
            Ok(())
        } else {
            Err(ErrV1::new(ReasonCode::GuardViolation, nid, "guard invariant false (§8.3)"))
        }
    }

    /// Default idem_key = blake3(nid ‖ canonical(bag projection of template or empty)).
    /// Override template: each Expr evaluates to a Sol fragment folded into the key material.
    fn once_key(&self, nid: &str, template: &Option<Vec<Expr>>) -> Result<String, ErrV1> {
        let mut parts = vec![SolValue::str(nid)];
        if let Some(exprs) = template {
            for e in exprs {
                parts.push(self.eval(nid, e)?);
            }
        }
        Ok(aelio_sol::value_hash(&SolValue::List(parts)))
    }

    /// §4.2.4 / §6.3: build the callee input from `args` only — least privilege.
    fn project_args(&self, nid: &str, args: &[(String, Expr)]) -> Result<SolValue, ErrV1> {
        let mut pairs = Vec::new();
        for (slot, expr) in args {
            pairs.push((slot.clone(), self.eval(nid, expr)?));
        }
        Ok(SolValue::map(pairs))
    }

    fn bag_get(&self, path: &Path) -> Option<&SolValue> {
        path.get(&self.bag)
    }
    fn bag_root(&self) -> &SolValue {
        &self.bag
    }
    fn write(&mut self, nid: &str, path: &Path, value: SolValue) -> Result<(), ErrV1> {
        set_path(&mut self.bag, path, value).map_err(|e| ErrV1::new(ReasonCode::Shape, nid, e))
    }
}

fn set_path(root: &mut SolValue, path: &Path, value: SolValue) -> Result<(), &'static str> {
    let mut bag = crate::bag::Bag::from_value(std::mem::replace(root, SolValue::Null));
    let r = bag.set(path, value);
    *root = bag.value().clone();
    r
}

fn best_catch<'a>(catch: &'a [(String, Node)], code: &ReasonCode) -> Option<&'a (String, Node)> {
    let full = code.code();
    catch
        .iter()
        .filter(|(prefix, _)| full == prefix.as_str() || full.starts_with(&format!("{prefix}.")))
        .max_by_key(|(prefix, _)| prefix.len())
}

fn shape(nid: &str, why: impl Into<String>) -> ErrV1 {
    ErrV1::new(ReasonCode::Shape, nid, why)
}
fn frame_mismatch(nid: &str, got: Frame) -> ErrV1 {
    ErrV1::new(ReasonCode::Internal, nid, format!("resume frame mismatch: {got:?}"))
}
