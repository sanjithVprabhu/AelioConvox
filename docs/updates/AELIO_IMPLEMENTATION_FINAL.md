# Aelio Implementation — Final Record (Phase 1 + Phase 2 A–C)

**Date:** 2026-08-07  
**Scope:** Everything implemented in-repo from the verification audit through Phase 2 Sessions A–C.  
**Authority:** HKv4 · [`HARNESS_KERNEL_V4.md`](HARNESS_KERNEL_V4.md) · [`IMPLEMENTATION_VERIFICATION.md`](IMPLEMENTATION_VERIFICATION.md) · [`FLAGS.md`](../../FLAGS.md) F-033 / F-034 / F-035

**Status snapshot:** Phase 1 and Phase 2 Sessions A–C are **complete**. Sessions D+ (effect contracts, op catalog, residue C1–C9, time module, Starlark, full acceptance suite) are **not started**.

For day-to-day “what’s next”, see [`AELIO_PHASE_FINAL_STATUS.md`](AELIO_PHASE_FINAL_STATUS.md).

---

## Executive summary

We audited the 104-point verification checklist against `aelio-os`, fixed eight confirmed bugs in the orchestration bridge, ratified three FLAGS decisions (HKv4 authority, hybrid Starlark codegen, fail-closed gating), then closed Phase 2 Sessions A–C:

| Session | Deliverable |
|---------|-------------|
| **A** | `harness-core` crate (HKv4 substrate), single canonical serialiser for `TaskGraph`, CI guard |
| **B** | Typed `JournalUnderrun`, `GraphSuspension` resume, wavefront determinism audit, per-step replay |
| **C** | Bridge reuse counters (`cold_executions`, `warm_hits`, executions-per-key) |

---

## Strategic decisions (FLAGS)

| ID | Decision |
|----|----------|
| **F-033** | Mother doc frozen; HKv4 + verification doc are north star. Bridge stays until substrate closes. |
| **F-034** | Hybrid codegen: model may emit Starlark **text**; **parsed AST is authoritative**; `harness_hash` = BLAKE3(canonical re-render), never raw text. |
| **F-035** | Fake semantic eval removed; shape-only gate + fail-closed on refinement exhaustion. GraphSuspension and JournalUnderrun closed in Phase 2 B. |

---

## Phase 1 — Verification audit & bug fixes

### Documents produced

| File | Purpose |
|------|---------|
| [`IMPLEMENTATION_VERIFICATION_REPORT.md`](IMPLEMENTATION_VERIFICATION_REPORT.md) | Line-by-line audit scorecard (rev. 2) |
| [`PHASE_1_CHANGELOG.md`](PHASE_1_CHANGELOG.md) | Detailed Phase 1 change log |
| [`PHASE_2_INSTRUCTIONS.md`](PHASE_2_INSTRUCTIONS.md) | Ordered Phase 2 work order (Tasks 1–10) |
| [`PHASE_2_ACCEPTANCE_SUITE.md`](PHASE_2_ACCEPTANCE_SUITE.md) | Sessions A–C + 60+ acceptance tests |
| [`BRIDGE_HKV4_CONVERGENCE.md`](BRIDGE_HKV4_CONVERGENCE.md) | Bridge → HKv4 convergence map |

### Code fixes (8)

| # | Problem | Fix | Primary files |
|---|---------|-----|---------------|
| 1 | Promotion rebuilt σ instead of turn `SituationKey` | Pass turn σ into `observe_task_graph_success` | `blocks/orchestrate.rs`, `blocks/turn.rs` |
| 2 | `TaskGraph::canonical_hash` used serde (Phase 1: manual writer) | Phase 2: collapsed onto `SolValue` → `aelio_sol::canonical` | `aelio-sol/src/task_graph.rs` |
| 3 | Wavefront `args_hash` used serde_json | BLAKE3 over §4.3 `value_to_sol` → `value_hash` | `orchestration/wavefront.rs`, `ops/pure.rs` |
| 4 | `NeedUser` fell through to ProposePath | Suspended `TurnResult` with `opened_loop` | `blocks/orchestrate.rs` |
| 5 | Doc drift: orchestrate vs Conductor order | Comment: orchestrate runs **before** Conductor on Tier3 | `blocks/turn.rs` |
| 6 | Effectful promotion used string heuristics | `path_is_effectful(registry, &path)` | `orchestration/promotion.rs` |
| 7 | Missing determinism edge cases | Tests: `-0.0`, float sum order, list disambiguation | `tests/canonical_conformance.rs` |
| 8 | Fake semantic eval (`pass: true`) | Shape-only eval; hard error after refinement budget | `orchestration/executor.rs` |

---

## Phase 2 Session A — Port + one serialiser

### A.1 — `harness-core` crate

**Location:** `aelio-os/crates/harness-core/`

Implemented from HKv4 spec (§2.1–2.5, §3.5, §4.5, §5.9, §17). **Not ported** from the original 54-test deliverable — reimplemented in-repo (**40 tests**). **Session D0** ([`PHASE_2D_INSTRUCTIONS.md`](PHASE_2D_INSTRUCTIONS.md)) must reconcile test-by-test before any further D+ work; fourteen gaps likely encode contested build findings (division scale, CAS, negative banker's ties, agg order sentinel, etc.).

| Module | Responsibility |
|--------|----------------|
| `numeric.rs` | `Int` / `Float` / `Decimal` tower; type-strict ops; banker's rounding; NaN/∞ guard |
| `agg.rs` | `Empty{reason}`, `count_rows`, `count_values`, `mean_skip_null`, `mean_strict`, pairwise float sum |
| `exec.rs` | `Step`, `ReplayReport`, `verify_replay`, `BudgetPool` with CAS drain |
| `taint.rs` | `TaintedValue`, join/propagate, map-key absorption, 9 prohibited positions, `PcTaint` |
| `versioning.rs` | `SystemVersion`, exhaustive `hash()`, `impl_hash`, Merkle root, `Verified<T>` |
| `lib.rs` | Re-exports + `aelio_sol` canonical re-exports |

**Tests:** 40 (debug + release identical hashes)

**Pinned reference hashes** (`tests/determinism.rs`, workspace `blake3`):

- `canonical_form_reference`: `a66bcd6a407d14189842ba22dc2627c7fdc023acccd7836a8e9d3fe40f4f2f80`
- `float_sum_reference`: `0140b77a15543ec6dd2a95205d3c3e2982e7cc673ae12245440c4c61561e1d4d`

### A.2 — Collapse dual canonical writer

**Before:** ~300 lines of independent `write_*` functions in `task_graph.rs` plus `aelio_sol::canonical`.

**After:**

- `TaskGraph::to_sol_value()` — materialises graph as `SolValue` (nodes sorted by `id`, merge fields sorted)
- `TaskGraph::canonical_bytes()` → `aelio_sol::canonical::to_bytes`
- `TaskGraph::canonical_hash()` → `aelio_sol::value_hash`

Deleted: all `write_task_graph`, `write_json_value`, etc.

### A.3 — CI guard

**Script:** [`scripts/check-canonical-writer.sh`](../../scripts/check-canonical-writer.sh)

- Fails if `fn write_task_graph` exists in `aelio-sol`
- Asserts exactly one `fn write_value(` in `aelio-sol/src/canonical.rs`

**Wired in:** `.github/workflows/ci.yml` (aelio-os job)

---

## Phase 2 Session B — Four debts

### B.1 — Typed `JournalUnderrun` (Q22 / A7)

**Kernel:** `ReasonCode::JournalUnderrun` → code string `Journal.Underrun`

**Behaviour:** `ReplayBackend::pop` past end of journal returns typed underrun — never dispatches tools during replay.

**Files:**

- `aelio-kernel/src/error.rs` — new variant + `from_code`
- `aelio-kernel/src/driver.rs` — `pop` uses `JournalUnderrun`; unit test on empty backend
- `aelio-kernel/tests/journal_underrun.rs` — empty journal + truncated two-call Seq integration tests

### B.2 — `GraphSuspension` wired (B7 partial)

**Types:** `orchestration/suspension.rs` — `GraphSuspension`, `SuspendedExecutorSnapshot`

**Flow:**

1. `GraphExecution::NeedUser` carries `pending_node_id`, `executor_state`, `slots`
2. `finish_orchestrated_execution` builds `GraphSuspension` on NeedUser
3. `TurnInput` / `TurnResult` gain `graph_suspension: Option<GraphSuspension>`
4. `World.user_graph_suspensions` persists per user
5. Next turn: `try_resume_orchestrated_graph` merges utterance into missing slots and calls `execute_task_graph_from_state`

**Files:**

- `orchestration/executor.rs` — `execute_task_graph_from_state`
- `blocks/orchestrate.rs` — `try_resume_orchestrated_graph`, `finish_orchestrated_execution`
- `blocks/turn.rs` — resume hook after `SituationKey`
- `runtime/world.rs` — load/save suspension state

**Remaining:** Full B7 end-to-end with client tool + user answer (Session D+).

### B.3 — Wavefront HashMap audit (A4 hygiene)

**Change:** `ExecutorState.completed` → `BTreeSet<String>`; `outputs` → `BTreeMap<String, Value>`

**Rationale:** Membership/lookup only today; BTree types align with F-032 determinism discipline and avoid accidental hash-path coupling.

**File:** `orchestration/wavefront.rs`

### B.4 — Per-step replay (Q21 / A1 partial)

**Harness-core:** `exec::verify_replay` — reports first diverging `seq`, length mismatch

**Kernel integration:** `aelio-kernel/tests/replay_steps.rs` (dev-dep on `harness-core`)

---

## Phase 2 Session C — Reuse counters

**Module:** `aelio-agent/src/reuse_metrics.rs`

| Counter | Meaning |
|---------|---------|
| `cold_executions` | Tier2 + Tier3 lookups |
| `warm_hits` | Tier0 + Tier1 lookups |
| `distinct_situation_keys` | Unique σ hashes seen |
| `executions_per_key` | Per-σ execution count (distribution verdict) |

**API:**

- `record_lookup(tier, situation_hash)` — called from `turn.rs` after `lookup_tier`
- `snapshot()` — test / ops introspection
- `daily_log_line()` — raw counters only; **no ratios at emitter** (§14.1)

---

## New and materially changed files

```
aelio-os/crates/harness-core/          # NEW — full HKv4 substrate (7 src + 6 test files)
aelio-os/crates/aelio-sol/src/task_graph.rs   # to_sol_value; manual writer removed
aelio-os/crates/aelio-kernel/src/error.rs     # JournalUnderrun
aelio-os/crates/aelio-kernel/src/driver.rs    # typed underrun + unit test
aelio-os/crates/aelio-kernel/tests/journal_underrun.rs
aelio-os/crates/aelio-kernel/tests/replay_steps.rs
aelio-os/crates/aelio-agent/src/reuse_metrics.rs
aelio-os/crates/aelio-agent/src/orchestration/executor.rs
aelio-os/crates/aelio-agent/src/orchestration/wavefront.rs
aelio-os/crates/aelio-agent/src/orchestration/suspension.rs
aelio-os/crates/aelio-agent/src/blocks/orchestrate.rs
aelio-os/crates/aelio-agent/src/blocks/turn.rs
aelio-os/crates/aelio-agent/src/runtime/world.rs
scripts/check-canonical-writer.sh
.github/workflows/ci.yml                      # canonical writer check step
FLAGS.md                                        # F-033, F-034, F-035
docs/updates/*                                  # audit + phase docs
```

---

## Verification commands

```bash
# Single canonical writer (Session A.4)
bash scripts/check-canonical-writer.sh

cd aelio-os

# Substrate + serialiser
cargo test -p harness-core
cargo test -p harness-core --release   # hashes must match debug
cargo test -p aelio-sol task_graph
cargo test -p aelio-sol --test canonical_conformance

# Kernel replay
cargo test -p aelio-kernel --test journal_underrun --test replay_steps

# Bridge
cargo test -p aelio-agent reuse_metrics
cargo test -p aelio-agent suspension
cargo test -p aelio-agent orchestrate
cargo test -p aelio-agent orchestration
```

---

## Audit scorecard movement (high level)

| Item | Phase 1 | After Phase 2 A–C |
|------|---------|-------------------|
| Dual canonical writer | KILL debt | **Closed** |
| harness-core stubs | BLOCKER | **Closed** (40 tests) |
| Q22 JournalUnderrun | PARTIAL (`Internal`) | **Closed** (`Journal.Underrun`) |
| GraphSuspension | Unwired | **Wired** (B7 E2E pending) |
| Wavefront HashMap | Unaudited | **Closed** (BTree) |
| Per-step replay | Missing | **Partial** (harness-core + kernel test) |
| Reuse counters | Missing | **Closed** |
| Residue C1–C9 | Not built | Open (D+) |
| Starlark embedding | Not started | Open (D+, last) |

Full checklist: [`IMPLEMENTATION_VERIFICATION_REPORT.md`](IMPLEMENTATION_VERIFICATION_REPORT.md)

---

## Not implemented (Phase 2 Sessions D+)

**Blocker:** [`PHASE_2D_INSTRUCTIONS.md`](PHASE_2D_INSTRUCTIONS.md) **Session D0** — reconcile 40 vs 54 `harness-core` tests before D1.

- Session D0: test-by-test reconciliation, API-shape checks, hash provenance freeze
- Session D1: aarch64 harness-core CI, A4 restart drill
- Op catalog, trybuild, time module (Task 6)
- Residue adversarial set **C1–C9** (Task 8)
- Starlark embedding (Task 7 — **last**, per F-033)
- Full acceptance suite sections B4/B5, C*, H*, I*, J integration tests
- Dedicated A4 process-restart hash drill
- Cross-arch harness-core CI on aarch64 (aelio-sol only today)

Work order: [`PHASE_2_INSTRUCTIONS.md`](PHASE_2_INSTRUCTIONS.md)  
Acceptance spec: [`PHASE_2_ACCEPTANCE_SUITE.md`](PHASE_2_ACCEPTANCE_SUITE.md)

---

## Document map

```
docs/updates/
├── AELIO_MASTER_PLAN.md            ← DONE vs REMAINING + implementation plan (read this to plan)
├── AELIO_IMPLEMENTATION_FINAL.md   ← THIS FILE (complete implementation record)
├── AELIO_PHASE_FINAL_STATUS.md     ← status + what's next
├── PHASE_1_CHANGELOG.md            ← Phase 1 detail only
├── PHASE_2_INSTRUCTIONS.md         ← D+ work order
├── PHASE_2_ACCEPTANCE_SUITE.md     ← 60+ tests spec
├── IMPLEMENTATION_VERIFICATION_REPORT.md
├── IMPLEMENTATION_VERIFICATION.md
└── BRIDGE_HKV4_CONVERGENCE.md
```

---

## One-line summary

**Implemented:** Phase 1 eight bridge fixes + FLAGS F-033/034/035; Phase 2 A–C — full `harness-core`, single TaskGraph serialiser, typed replay underrun, GraphSuspension resume, deterministic wavefront state, reuse counters. **Not implemented:** Starlark, residue C1–C9, full acceptance suite, Sessions D+.
