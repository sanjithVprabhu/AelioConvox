//! HKv4 execution journal, replay verification, and shared budget pool (§3.5, §4.5).

use std::sync::atomic::{AtomicI64, Ordering};

/// BLAKE3 content hash used in traces and ledger entries.
pub type ContentHash = [u8; 32];

/// One hashed execution step in a trace (§4.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub seq: u64,
    pub inputs_hash: ContentHash,
    pub output_hash: ContentHash,
}

/// Result of comparing a live trace against a replay trace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayReport {
    pub matched: bool,
    pub diverged_at: Option<u64>,
    pub expected_output_hash: Option<ContentHash>,
    pub actual_output_hash: Option<ContentHash>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetDimension {
    ResultRows,
    ModelCalls,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetExhausted {
    pub dimension: BudgetDimension,
    /// Dynamic frame that attempted the charge.  Amounts deliberately do not
    /// escape this error: a partial aggregate must never be presentation data.
    pub frame: String,
}

/// Shared per-turn budget pool with CAS depletion (§3.5, N14).
///
/// Uses compare-and-swap rather than `fetch_sub` so concurrent drainers cannot
/// overshoot before any thread observes exhaustion.
#[derive(Debug)]
pub struct BudgetPool {
    result_rows: AtomicI64,
    model_calls: AtomicI64,
}

/// A named execution frame borrowing the one turn-wide budget pool.
///
/// Frames do not receive allowances of their own; nested calls must construct
/// another `BudgetFrame` over the same pool.
#[derive(Debug)]
pub struct BudgetFrame<'a> {
    pool: &'a BudgetPool,
    name: String,
}

impl BudgetPool {
    pub fn new(result_rows: i64) -> Self {
        Self::with_limits(result_rows, 0)
    }

    /// Construct a turn-wide pool. Model calls intentionally default to zero
    /// in `new`; granting them is an explicit promotion-time decision (§3.4).
    pub fn with_limits(result_rows: i64, model_calls: i64) -> Self {
        Self {
            result_rows: AtomicI64::new(result_rows),
            model_calls: AtomicI64::new(model_calls),
        }
    }

    /// Name an execution frame without allocating a separate budget.
    pub fn frame(&self, name: &str) -> BudgetFrame<'_> {
        BudgetFrame {
            pool: self,
            name: name.to_owned(),
        }
    }

    pub fn remaining_result_rows(&self) -> i64 {
        self.result_rows.load(Ordering::Acquire)
    }

    pub fn remaining_model_calls(&self) -> i64 {
        self.model_calls.load(Ordering::Acquire)
    }

    /// Attempt to drain `amount` rows from the shared pool.
    pub fn try_drain_result_rows(&self, amount: i64) -> Result<(), BudgetExhausted> {
        self.frame("root").try_drain_result_rows(amount)
    }

    /// Attempt to charge one model call from the shared pool.
    pub fn try_take_model_call(&self) -> Result<(), BudgetExhausted> {
        self.frame("root").try_take_model_call()
    }

    /// Total rows successfully drained (initial minus remaining).
    pub fn consumed_result_rows(&self, initial: i64) -> i64 {
        initial - self.remaining_result_rows()
    }
}

impl BudgetFrame<'_> {
    /// Attempt to drain `amount` rows while retaining the failing frame name.
    pub fn try_drain_result_rows(&self, amount: i64) -> Result<(), BudgetExhausted> {
        drain(
            &self.pool.result_rows,
            amount,
            BudgetDimension::ResultRows,
            &self.name,
        )
    }

    /// Attempt to charge one model call while retaining the failing frame name.
    pub fn try_take_model_call(&self) -> Result<(), BudgetExhausted> {
        drain(
            &self.pool.model_calls,
            1,
            BudgetDimension::ModelCalls,
            &self.name,
        )
    }
}

fn drain(
    pool: &AtomicI64,
    amount: i64,
    dimension: BudgetDimension,
    frame: &str,
) -> Result<(), BudgetExhausted> {
    if amount <= 0 {
        return Ok(());
    }
    loop {
        let current = pool.load(Ordering::Acquire);
        if current < amount {
            return Err(BudgetExhausted {
                dimension,
                frame: frame.to_owned(),
            });
        }
        match pool.compare_exchange_weak(
            current,
            current - amount,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return Ok(()),
            Err(_) => continue,
        }
    }
}

/// Execution modes with their effect-dispatch contract (§3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Live,
    Replay,
    Shadow,
    Dry,
}

/// The effect classes relevant to execution-mode dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectClass {
    Read,
    Write,
    IdempotentWrite,
    Model,
}

/// Audit record for an attempted effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub seq: u64,
    pub effect: String,
    pub stubbed: bool,
}

/// Minimal ordered journal used by the harness execution substrate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectJournal {
    entries: Vec<JournalEntry>,
}

impl EffectJournal {
    pub fn entries(&self) -> &[JournalEntry] {
        &self.entries
    }

    fn record(&mut self, effect: &str, stubbed: bool) {
        self.entries.push(JournalEntry {
            seq: self.entries.len() as u64,
            effect: effect.to_owned(),
            stubbed,
        });
    }

    pub fn replay_cursor(&self) -> ReplayCursor {
        ReplayCursor {
            entries: self.entries.clone(),
            next: 0,
        }
    }
}

/// Typed replay failure when execution needs an absent journal entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalUnderrun {
    pub seq: u64,
}

/// Ordered reader for a replay journal.
#[derive(Debug)]
pub struct ReplayCursor {
    entries: Vec<JournalEntry>,
    next: usize,
}

impl ReplayCursor {
    pub fn take(&mut self, effect: &str) -> Result<&JournalEntry, JournalUnderrun> {
        let seq = self.next as u64;
        let entry = self.entries.get(self.next).ok_or(JournalUnderrun { seq })?;
        self.next += 1;
        if entry.effect == effect {
            Ok(entry)
        } else {
            // A different path is a divergence; it must not be reinterpreted
            // as permission to dispatch an unjournaled effect.
            Err(JournalUnderrun { seq })
        }
    }
}

/// Outcome of routing an effect through an execution mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectOutcome<T> {
    Dispatched(T),
    Stubbed,
    Replayed(JournalEntry),
}

/// Execute exactly when the mode permits it, always recording stubs loudly.
///
/// Replay consumes the supplied cursor before the dispatcher is even observed,
/// preventing a missing or divergent journal from causing side effects.
pub fn execute_effect<T, F>(
    mode: ExecutionMode,
    class: EffectClass,
    effect: &str,
    journal: &mut EffectJournal,
    replay: Option<&mut ReplayCursor>,
    dispatch: F,
) -> Result<EffectOutcome<T>, JournalUnderrun>
where
    F: FnOnce() -> T,
{
    if mode == ExecutionMode::Replay {
        let entry = match replay {
            Some(cursor) => cursor.take(effect)?.clone(),
            None => return Err(JournalUnderrun { seq: 0 }),
        };
        return Ok(EffectOutcome::Replayed(entry));
    }

    let stubbed = matches!(mode, ExecutionMode::Dry)
        || (mode == ExecutionMode::Shadow
            && matches!(class, EffectClass::Write | EffectClass::IdempotentWrite));
    journal.record(effect, stubbed);
    if stubbed {
        Ok(EffectOutcome::Stubbed)
    } else {
        Ok(EffectOutcome::Dispatched(dispatch()))
    }
}

/// Compare every step hash between live and replay traces (§4.5, Q21).
pub fn verify_replay(live: &[Step], replay: &[Step]) -> ReplayReport {
    let live_len = live.len();
    let replay_len = replay.len();
    let common = live_len.min(replay_len);

    for idx in 0..common {
        let left = &live[idx];
        let right = &replay[idx];
        if left.seq != right.seq
            || left.inputs_hash != right.inputs_hash
            || left.output_hash != right.output_hash
        {
            return ReplayReport {
                matched: false,
                diverged_at: Some(left.seq),
                expected_output_hash: Some(left.output_hash),
                actual_output_hash: Some(right.output_hash),
            };
        }
    }

    if live_len != replay_len {
        let divergent = if live_len > replay_len {
            live.get(replay_len)
        } else {
            replay.get(live_len)
        };
        return ReplayReport {
            matched: false,
            diverged_at: divergent.map(|step| step.seq),
            expected_output_hash: live.get(common).map(|step| step.output_hash),
            actual_output_hash: replay.get(common).map(|step| step.output_hash),
        };
    }

    ReplayReport {
        matched: true,
        diverged_at: None,
        expected_output_hash: live.last().map(|step| step.output_hash),
        actual_output_hash: replay.last().map(|step| step.output_hash),
    }
}

/// Hash arbitrary bytes for step recording.
pub fn hash_step_payload(label: &str, payload: &[u8]) -> ContentHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(label.as_bytes());
    hasher.update(payload);
    *hasher.finalize().as_bytes()
}
