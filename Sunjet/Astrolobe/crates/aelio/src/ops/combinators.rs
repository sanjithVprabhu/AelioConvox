//! S0-A — Combinators. Closed set; never extended by tenants.
//!
//! Combinators take Ops as arguments. Kept small because they are the branching
//! factor of tier-2 compositional search.

use crate::types::{AelioError, AelioResult, ReasonCode, Value};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Executable node in a composition tree.
#[derive(Debug, Clone)]
pub enum Op {
    /// Lift a literal.
    Const(Value),
    /// Pass input through.
    Identity,
    /// Named ability / pure op invocation.
    Call {
        id: String,
        args: indexmap::IndexMap<String, Value>,
    },
    Seq(Vec<Op>),
    Branch {
        pred: Box<Op>,
        then_arm: Box<Op>,
        else_arm: Box<Op>,
    },
    Loop {
        body: Box<Op>,
        while_pred: Box<Op>,
        max_iter: u32,
    },
    Try {
        body: Box<Op>,
        /// reason_code name → recovery op
        catch: indexmap::IndexMap<String, Op>,
        finally: Option<Box<Op>>,
    },
    Guard {
        invariant: Box<Op>,
        body: Box<Op>,
        on_violation: ReasonCode,
    },
    Fallback(Vec<Op>),
    Tee {
        body: Box<Op>,
        side: Box<Op>,
    },
    Once {
        body: Box<Op>,
        idem_key: String,
    },
    Budget {
        body: Box<Op>,
        tokens: Option<u64>,
        ms: Option<u64>,
        calls: Option<u64>,
    },
    Timeout {
        body: Box<Op>,
        ms: u64,
    },
    Map {
        body: Box<Op>,
        max_items: usize,
    },
    Filter {
        pred: Box<Op>,
        max_items: usize,
    },
    Let {
        bindings: indexmap::IndexMap<String, Op>,
        body: Box<Op>,
    },
    /// Park — ends the turn; not a blocking sleep.
    Park {
        until: ParkUntil,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParkUntil {
    Event(String),
    TtlSecs(u64),
    Instant(String),
}

/// Runtime bindings for combinator evaluation.
pub type InvokeFn<'a> =
    dyn FnMut(&str, &indexmap::IndexMap<String, Value>, &Value) -> AelioResult<Value> + 'a;

pub struct EvalCtx<'a> {
    pub invoke: &'a mut InvokeFn<'a>,
    pub once_seen: &'a mut HashSet<String>,
    /// Successful Once results, so a duplicate returns the same typed value instead of a
    /// synthetic marker that violates the body's output contract.
    pub once_results: indexmap::IndexMap<String, Value>,
    pub budget_calls_left: Option<u64>,
    pub budget_tokens_left: Option<u64>,
    pub locals: indexmap::IndexMap<String, Value>,
    pub llm_calls: u32,
    pub suspended: Option<ParkUntil>,
    deadline: Option<Instant>,
}

impl<'a> EvalCtx<'a> {
    pub fn new(invoke: &'a mut InvokeFn<'a>, once_seen: &'a mut HashSet<String>) -> Self {
        Self {
            invoke,
            once_seen,
            once_results: indexmap::IndexMap::new(),
            budget_calls_left: None,
            budget_tokens_left: None,
            locals: indexmap::IndexMap::new(),
            llm_calls: 0,
            suspended: None,
            deadline: None,
        }
    }
}

pub fn eval(op: &Op, input: &Value, ctx: &mut EvalCtx<'_>) -> AelioResult<Value> {
    check_deadline(ctx)?;
    if ctx.suspended.is_some() {
        return Err(AelioError::new(
            ReasonCode::Suspended,
            "instance already parked",
        ));
    }
    match op {
        Op::Const(v) => Ok(v.clone()),
        Op::Identity => Ok(input.clone()),
        Op::Call { id, args } => {
            if let Some(left) = ctx.budget_calls_left.as_mut() {
                if *left == 0 {
                    return Err(AelioError::new(
                        ReasonCode::BudgetExceeded,
                        "call budget exhausted",
                    ));
                }
                *left -= 1;
            }
            (ctx.invoke)(id, args, input)
        }
        Op::Seq(steps) => {
            let mut cur = input.clone();
            for step in steps {
                cur = eval(step, &cur, ctx)?;
                if ctx.suspended.is_some() {
                    break;
                }
            }
            Ok(cur)
        }
        Op::Branch {
            pred,
            then_arm,
            else_arm,
        } => {
            let p = eval(pred, input, ctx)?;
            let b = p.as_bool().ok_or_else(|| {
                AelioError::new(ReasonCode::TypeViolation, "Branch pred must be bool")
            })?;
            if b {
                eval(then_arm, input, ctx)
            } else {
                eval(else_arm, input, ctx)
            }
        }
        Op::Loop {
            body,
            while_pred,
            max_iter,
        } => {
            let mut cur = input.clone();
            for i in 0..*max_iter {
                let p = eval(while_pred, &cur, ctx)?;
                let b = p.as_bool().ok_or_else(|| {
                    AelioError::new(ReasonCode::TypeViolation, "Loop while must be bool")
                })?;
                if !b {
                    return Ok(cur);
                }
                cur = eval(body, &cur, ctx)?;
                if ctx.suspended.is_some() {
                    return Ok(cur);
                }
                if i + 1 == *max_iter {
                    // check if still true → budget
                    let still = eval(while_pred, &cur, ctx)?;
                    if still.as_bool() == Some(true) {
                        return Err(AelioError::new(
                            ReasonCode::LoopBudgetExceeded,
                            format!("Loop exceeded max_iter={max_iter}"),
                        ));
                    }
                }
            }
            Ok(cur)
        }
        Op::Try {
            body,
            catch,
            finally,
        } => {
            let result = eval(body, input, ctx);
            let out = match result {
                Ok(v) => Ok(v),
                Err(e) => {
                    let key = format!("{:?}", e.code).to_lowercase();
                    // also try snake variants of common names
                    let key2 = reason_key(&e.code);
                    if let Some(handler) = catch.get(&key2).or_else(|| catch.get(&key)) {
                        eval(handler, input, ctx)
                    } else {
                        Err(e)
                    }
                }
            };
            if let Some(fin) = finally {
                // Cleanup/audit failure is observable. Swallowing it makes a composed operation
                // report success after its required finalizer failed.
                eval(fin, input, ctx)?;
            }
            out
        }
        Op::Guard {
            invariant,
            body,
            on_violation,
        } => {
            let p = eval(invariant, input, ctx)?;
            let b = p.as_bool().unwrap_or(false);
            if !b {
                return Err(AelioError::new(*on_violation, "guard invariant violated"));
            }
            eval(body, input, ctx)
        }
        Op::Fallback(ops) => {
            let mut last_err = AelioError::new(ReasonCode::NotFound, "empty Fallback");
            for op in ops {
                match eval(op, input, ctx) {
                    Ok(v) => return Ok(v),
                    Err(e) => last_err = e,
                }
            }
            Err(last_err)
        }
        Op::Tee { body, side } => {
            let out = eval(body, input, ctx)?;
            eval(side, &out, ctx)?; // side value discarded; failure remains typed and observable
            Ok(out)
        }
        Op::Once { body, idem_key } => {
            if let Some(result) = ctx.once_results.get(idem_key) {
                return Ok(result.clone());
            }
            if ctx.once_seen.contains(idem_key) {
                return Err(AelioError::new(
                    ReasonCode::Conflict,
                    format!("Once result for key {idem_key} is unavailable for replay"),
                ));
            }
            let out = eval(body, input, ctx)?;
            ctx.once_seen.insert(idem_key.clone());
            ctx.once_results.insert(idem_key.clone(), out.clone());
            Ok(out)
        }
        Op::Budget {
            body,
            tokens,
            ms,
            calls,
        } => {
            // Nested budgets intersect (never widen).
            let prev_calls = ctx.budget_calls_left;
            let prev_tokens = ctx.budget_tokens_left;
            let prev_deadline = ctx.deadline;
            ctx.budget_calls_left = match (prev_calls, *calls) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };
            ctx.budget_tokens_left = match (prev_tokens, *tokens) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };
            ctx.deadline = intersect_deadline(prev_deadline, *ms);
            let result = eval(body, input, ctx);
            ctx.budget_calls_left = prev_calls;
            ctx.budget_tokens_left = prev_tokens;
            ctx.deadline = prev_deadline;
            result
        }
        Op::Timeout { body, ms } => {
            let previous = ctx.deadline;
            ctx.deadline = intersect_deadline(previous, Some(*ms));
            let result = eval(body, input, ctx);
            let expired = ctx
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline);
            ctx.deadline = previous;
            if expired {
                Err(AelioError::new(
                    ReasonCode::Timeout,
                    format!("operation exceeded its {ms}ms cooperative deadline"),
                ))
            } else {
                result
            }
        }
        Op::Map { body, max_items } => {
            let list = input.as_list().ok_or_else(|| {
                AelioError::new(ReasonCode::TypeViolation, "Map expects list input")
            })?;
            enforce_collection_bound("Map", list.len(), *max_items)?;
            let mut out = Vec::with_capacity(list.len());
            for item in list {
                out.push(eval(body, item, ctx)?);
            }
            Ok(Value::List(out))
        }
        Op::Filter { pred, max_items } => {
            let list = input.as_list().ok_or_else(|| {
                AelioError::new(ReasonCode::TypeViolation, "Filter expects list input")
            })?;
            enforce_collection_bound("Filter", list.len(), *max_items)?;
            let mut out = Vec::with_capacity(list.len().min(*max_items));
            for item in list {
                let p = eval(pred, item, ctx)?;
                if p.as_bool() == Some(true) {
                    out.push(item.clone());
                }
            }
            Ok(Value::List(out))
        }
        Op::Let { bindings, body } => {
            let saved = ctx.locals.clone();
            for (name, bop) in bindings {
                let v = eval(bop, input, ctx)?;
                ctx.locals.insert(name.clone(), v);
            }
            let result = eval(body, input, ctx);
            ctx.locals = saved;
            result
        }
        Op::Park { until } => {
            ctx.suspended = Some(until.clone());
            Err(AelioError::new(
                ReasonCode::Suspended,
                format!("parked until {until:?}"),
            ))
        }
    }
}

fn intersect_deadline(current: Option<Instant>, ms: Option<u64>) -> Option<Instant> {
    let proposed = ms.map(|value| Instant::now() + Duration::from_millis(value));
    match (current, proposed) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn check_deadline(ctx: &EvalCtx<'_>) -> AelioResult<()> {
    if ctx
        .deadline
        .is_some_and(|deadline| Instant::now() >= deadline)
    {
        Err(AelioError::new(
            ReasonCode::Timeout,
            "cooperative operation deadline exceeded",
        ))
    } else {
        Ok(())
    }
}

fn enforce_collection_bound(name: &str, observed: usize, max_items: usize) -> AelioResult<()> {
    if observed > max_items {
        Err(AelioError::new(
            ReasonCode::BudgetExceeded,
            format!("{name} input has {observed} items, maximum is {max_items}"),
        ))
    } else {
        Ok(())
    }
}

fn reason_key(code: &ReasonCode) -> String {
    format!("{code:?}")
        .chars()
        .enumerate()
        .flat_map(|(i, c)| {
            if c.is_uppercase() && i > 0 {
                vec!['_', c.to_ascii_lowercase()]
            } else {
                vec![c.to_ascii_lowercase()]
            }
        })
        .collect()
}

/// Helper builders.
pub fn seq(ops: Vec<Op>) -> Op {
    Op::Seq(ops)
}

pub fn call(id: impl Into<String>) -> Op {
    Op::Call {
        id: id.into(),
        args: indexmap::IndexMap::new(),
    }
}

pub fn call_args(id: impl Into<String>, args: indexmap::IndexMap<String, Value>) -> Op {
    Op::Call {
        id: id.into(),
        args,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn seq_threads_values() {
        let mut seen = HashSet::new();
        let mut inv = |id: &str, _: &indexmap::IndexMap<String, Value>, input: &Value| {
            if id == "inc" {
                Ok(Value::Int(input.as_i64().unwrap() + 1))
            } else {
                Ok(input.clone())
            }
        };
        let mut ctx = EvalCtx::new(&mut inv, &mut seen);
        let op = seq(vec![call("inc"), call("inc"), call("inc")]);
        assert_eq!(eval(&op, &Value::Int(0), &mut ctx).unwrap(), Value::Int(3));
    }

    #[test]
    fn once_dedups() {
        let mut seen = HashSet::new();
        let mut calls = 0u32;
        let mut inv = |id: &str, _: &indexmap::IndexMap<String, Value>, _: &Value| {
            if id == "effect" {
                calls += 1;
                Ok(Value::str("done"))
            } else {
                Ok(Value::Null)
            }
        };
        let mut ctx = EvalCtx::new(&mut inv, &mut seen);
        let op = Op::Once {
            body: Box::new(call("effect")),
            idem_key: "k1".into(),
        };
        let first = eval(&op, &Value::Null, &mut ctx).unwrap();
        let replay = eval(&op, &Value::Null, &mut ctx).unwrap();
        assert_eq!(calls, 1);
        assert_eq!(replay, first);
    }

    #[test]
    fn loop_respects_max_iter() {
        let mut seen = HashSet::new();
        let mut inv = |id: &str, _: &indexmap::IndexMap<String, Value>, input: &Value| match id {
            "lt5" => Ok(Value::Bool(input.as_i64().unwrap() < 5)),
            "inc" => Ok(Value::Int(input.as_i64().unwrap() + 1)),
            _ => Ok(Value::Null),
        };
        let mut ctx = EvalCtx::new(&mut inv, &mut seen);
        let op = Op::Loop {
            body: Box::new(call("inc")),
            while_pred: Box::new(call("lt5")),
            max_iter: 10,
        };
        assert_eq!(eval(&op, &Value::Int(0), &mut ctx).unwrap(), Value::Int(5));

        let mut ctx2 = EvalCtx::new(&mut inv, &mut seen);
        let op2 = Op::Loop {
            body: Box::new(call("inc")),
            while_pred: Box::new(Op::Const(Value::Bool(true))),
            max_iter: 3,
        };
        assert!(matches!(
            eval(&op2, &Value::Int(0), &mut ctx2).unwrap_err().code,
            ReasonCode::LoopBudgetExceeded
        ));
    }

    #[test]
    fn map_and_filter_reject_inputs_over_their_structural_bound() {
        let mut invoke = |_id: &str,
                          _args: &indexmap::IndexMap<String, Value>,
                          input: &Value|
         -> AelioResult<Value> { Ok(input.clone()) };
        let mut once = HashSet::new();
        let mut ctx = EvalCtx::new(&mut invoke, &mut once);
        let input = Value::List(vec![Value::Int(1), Value::Int(2)]);
        let map = Op::Map {
            body: Box::new(Op::Identity),
            max_items: 1,
        };
        assert_eq!(
            eval(&map, &input, &mut ctx).unwrap_err().code,
            ReasonCode::BudgetExceeded
        );

        let filter = Op::Filter {
            pred: Box::new(Op::Const(Value::Bool(true))),
            max_items: 1,
        };
        assert_eq!(
            eval(&filter, &input, &mut ctx).unwrap_err().code,
            ReasonCode::BudgetExceeded
        );
    }

    #[test]
    fn tee_does_not_hide_audit_failure() {
        let mut invoke = |id: &str,
                          _args: &indexmap::IndexMap<String, Value>,
                          input: &Value|
         -> AelioResult<Value> {
            if id == "audit" {
                Err(AelioError::new(
                    ReasonCode::Unavailable,
                    "audit unavailable",
                ))
            } else {
                Ok(input.clone())
            }
        };
        let mut once = HashSet::new();
        let mut ctx = EvalCtx::new(&mut invoke, &mut once);
        let op = Op::Tee {
            body: Box::new(Op::Identity),
            side: Box::new(call("audit")),
        };
        assert_eq!(
            eval(&op, &Value::str("result"), &mut ctx).unwrap_err().code,
            ReasonCode::Unavailable
        );
    }

    #[test]
    fn budget_intersects() {
        let mut seen = HashSet::new();
        let mut inv =
            |_id: &str, _: &indexmap::IndexMap<String, Value>, input: &Value| Ok(input.clone());
        let mut ctx = EvalCtx::new(&mut inv, &mut seen);
        let inner = Op::Budget {
            body: Box::new(seq(vec![call("a"), call("b"), call("c")])),
            tokens: None,
            ms: None,
            calls: Some(2),
        };
        let outer = Op::Budget {
            body: Box::new(inner),
            tokens: None,
            ms: None,
            calls: Some(5),
        };
        // Only 2 calls allowed (intersection of 5 and 2).
        let err = eval(&outer, &Value::Null, &mut ctx).unwrap_err();
        assert_eq!(err.code, ReasonCode::BudgetExceeded);
    }
}
