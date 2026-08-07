# Phase 1 Changelog — Implementation Verification Audit & Fixes

**Date:** 2026-08-07  
**Session:** Verification audit against [`IMPLEMENTATION_VERIFICATION.md`](IMPLEMENTATION_VERIFICATION.md), Phase 1 bug fixes, report review, Phase 2 work order.  
**Authority going forward:** HKv4 per [`FLAGS.md` F-033](../../FLAGS.md)

---

## What we set out to do

1. Walk the 104-point verification checklist line by line against real `aelio-os` code.
2. Fix confirmed bugs without papering over gaps.
3. Document honest scorecard results and Phase 2 work order.
4. Incorporate review pushback (Starlark hybrid, harness-core port vs stubs, fake gates, reuse counters, scorecard corrections).

---

## Documents created or updated

| File | Action | Purpose |
|------|--------|---------|
| [`IMPLEMENTATION_VERIFICATION_REPORT.md`](IMPLEMENTATION_VERIFICATION_REPORT.md) | Created → rev. 2 | Audit scorecard, bugs fixed, debt admitted, Phase 2 sequencing |
| [`PHASE_2_INSTRUCTIONS.md`](PHASE_2_INSTRUCTIONS.md) | Referenced | Ordered work order for Phase 2 (Tasks 1–10) |
| [`BRIDGE_HKV4_CONVERGENCE.md`](BRIDGE_HKV4_CONVERGENCE.md) | Created | Bridge → HKv4 mapping; prevents two permanent reuse systems |
| [`PHASE_1_CHANGELOG.md`](PHASE_1_CHANGELOG.md) | Created | This file — final record of changes |
| [`FLAGS.md`](../../FLAGS.md) | Updated | F-033, F-034, F-035 decision-log entries |

---

## FLAGS decision log (new entries)

### F-033 — Framework migration
- **Mother doc** (`AELIO_DSL_MOTHER.md`) frozen as historical reference.
- **North star:** HKv4 + `IMPLEMENTATION_VERIFICATION.md`.
- Orchestration bridge stays until Starlark substrate is closed.
- Phase 1 fixes listed; Phase 2 work order in `PHASE_2_INSTRUCTIONS.md`.

### F-034 — Starlark codegen (hybrid)
- Model **may emit Starlark text**.
- **Parsed AST is the only authoritative artifact.**
- `harness_hash` = BLAKE3(canonical re-render(parse(text))) — **never raw text.**
- Parse failure → capped repair (max 3) → Clarify.
- Renderer round-trip property test required before production codegen.

### F-035 — Fail-closed gating
- Removed fake semantic eval (`pass: true`) in orchestration executor.
- Shape-only evaluation until semantic gate is implemented.
- Refinement exhaustion → hard error, not silent success.
- Pending: wire or delete `GraphSuspension`; typed `JournalUnderrun` for replay.

---

## Code changes — bug fixes (Phase 1)

### 1. Orchestration promotion situation key [CORR]

**Problem:** Promoted TaskGraphs used a rebuilt `SituationKey` (raw clause, empty slots/caps) instead of the turn's real σ. Warm-path lookup could never match.

**Change:**
- `try_orchestrated_cold_path` now accepts `sigma: &SituationKey` from the turn spine.
- `observe_task_graph_success` uses that σ directly.

**Files:**
- `aelio-os/crates/aelio-agent/src/blocks/orchestrate.rs`
- `aelio-os/crates/aelio-agent/src/blocks/turn.rs`

---

### 2. TaskGraph canonical hash off serde [KILL]

**Problem:** `TaskGraph::canonical_hash()` used `serde_json::to_string` — unstable across serde versions.

**Change:**
- Added `TaskGraph::canonical_bytes()` — manual §4.3-style writer, nodes sorted by `id`.
- `canonical_hash()` = BLAKE3 over canonical bytes.

**Test added:** `canonical_hash_is_stable_across_node_insertion_order`

**File:** `aelio-os/crates/aelio-sol/src/task_graph.rs`

**Debt:** Second canonical writer — must reconcile with `harness_core::canonical` on port (Task 2).

---

### 3. Wavefront ledger args hash off serde [KILL]

**Problem:** `wavefront::hash_args()` hashed serde JSON for idempotency keys.

**Change:**
- Added `value_to_sol()` in `ops/pure.rs` (agent `Value` → `SolValue`).
- `hash_args()` sorts keys, converts to `SolValue::Map`, hashes via `aelio_sol::value_hash()`.

**Files:**
- `aelio-os/crates/aelio-agent/src/orchestration/wavefront.rs`
- `aelio-os/crates/aelio-agent/src/ops/pure.rs`

**Test:** `repeat_execution_produces_an_identical_ledger` still passes.

---

### 4. NeedUser suspension [CORR]

**Problem:** Unbound client-tool params caused orchestration to return `None` and fall through to ProposePath (re-plan instead of ask).

**Change:**
- `GraphExecution::NeedUser` returns suspended `TurnResult` with clarifying question.
- Trace step: `Orchestrate.NeedUser` (not `Orchestrate.Skip`).
- `suspended: true`, `opened_loop: true`.

**Test added:** `need_user_returns_suspended_turn`

**File:** `aelio-os/crates/aelio-agent/src/blocks/orchestrate.rs`

**Debt:** `GraphSuspension` types still unwired for mid-graph resume (Task 3.3).

---

### 5. Turn spine documentation

**Problem:** Module comment said orchestrate runs after Conductor; code runs it **before** on Tier3.

**Change:** Comment updated to match execution order.

**File:** `aelio-os/crates/aelio-agent/src/blocks/turn.rs`

---

### 6. Promotion effectful detection [CORR]

**Problem:** `promotion.rs` used substring heuristics (`"invoke"`, `"tool."`) on strategy hints.

**Change:** Uses `path_is_effectful(registry, &path)` after graph → `AbilityPath` conversion.

**File:** `aelio-os/crates/aelio-agent/src/orchestration/promotion.rs`

---

### 7. Determinism conformance tests

**Added to** `aelio-os/crates/aelio-sol/tests/canonical_conformance.rs`:

| Test | Checklist |
|------|-----------|
| `minus_zero_and_zero_hash_equally` | Q9 |
| `float_sum_and_literal_differ_in_hash` | Q8 |
| `adjacent_string_list_elements_hash_differ` | Q11 |

---

### 8. Fake eval gate removed [SEC] (rev. 2 / F-035)

**Problem:** Orchestration executor semantic hook always returned `pass: true`. Refinement exhaustion returned `Done` even when verdict failed.

**Change:**
- Semantic stub removed; only `evaluate_shape()` runs.
- Shape failure after refinement budget → `Err(Validation)`, not silent completion.

**File:** `aelio-os/crates/aelio-agent/src/orchestration/executor.rs`

---

## New crate scaffolded (not yet ported)

**Path:** `aelio-os/crates/harness-core/`

| Module | Status |
|--------|--------|
| `numeric.rs` | Stub — **port from deliverables (Task 2)** |
| `agg.rs` | Stub |
| `taint.rs` | Stub |
| `versioning.rs` | Stub |
| `exec.rs` | Stub |
| `clippy.toml` | HashMap/HashSet ban |
| `tests/determinism.rs` | 1 smoke test |

Added to `aelio-os/Cargo.toml` workspace members. `Cargo.lock` updated.

**Note:** Full implementation (2,111 lines, 54 tests) exists in external deliverables — Phase 1 incorrectly created stubs instead of porting. Task 2 is **port, not reimplement**.

---

## Verification audit summary

| Tier | Count | Examples |
|------|-------|----------|
| PASS / FIXED | ~15 | Canonical float tests, replay past-ledger refuse, residue Bangalore case, reply numeric guard |
| PARTIAL / DEBT | ~12 | Dual canonical writer, wavefront HashMap, Q22 underrun typing, reuse counters missing |
| GAP (pre-Starlark) | ~70 | Decimal tower, taint, SystemVersion, Starlark embedding, pairwise sum |

**Scorecard corrections (rev. 2):**
- Q103/Q104: OPEN — F-033 is authority shift, not micro-decision log or gap honesty statement.
- Q22: PARTIAL — behaviour correct (`Internal` on past-ledger); needs typed `JournalUnderrun`.

---

## Tests run (all green after changes)

```bash
cd aelio-os
cargo test -p aelio-sol                           # 23 tests
cargo test -p harness-core                        # 1 test
cargo test -p aelio-agent blocks::orchestrate     # 2 tests
cargo test -p aelio-agent orchestration           # 32 tests
cargo clippy -p aelio-sol -p harness-core --all-targets -- -D warnings
```

---

## File index (this session's touch points)

### Rust — modified
```
aelio-os/Cargo.toml
aelio-os/crates/aelio-sol/src/task_graph.rs
aelio-os/crates/aelio-sol/tests/canonical_conformance.rs
aelio-os/crates/aelio-agent/src/blocks/orchestrate.rs   (new)
aelio-os/crates/aelio-agent/src/blocks/turn.rs
aelio-os/crates/aelio-agent/src/ops/pure.rs
aelio-os/crates/aelio-agent/src/orchestration/promotion.rs
aelio-os/crates/aelio-agent/src/orchestration/wavefront.rs
aelio-os/crates/aelio-agent/src/orchestration/executor.rs
```

### Rust — new
```
aelio-os/crates/harness-core/**   (scaffold)
aelio-os/crates/aelio-sol/clippy.toml
```

### Docs — new/updated
```
docs/updates/IMPLEMENTATION_VERIFICATION_REPORT.md
docs/updates/PHASE_2_INSTRUCTIONS.md              (pre-existing, referenced)
docs/updates/BRIDGE_HKV4_CONVERGENCE.md
docs/updates/PHASE_1_CHANGELOG.md                 (this file)
FLAGS.md                                          (F-033, F-034, F-035)
```

---

## What is NOT done (Phase 2 — do not skip order)

| Task | Description | Priority |
|------|-------------|----------|
| **2** | Port full `harness-core` from deliverables; one canonical serialiser | First coding task |
| **3** | Finish fake-gate inventory; CI grep; wire/delete GraphSuspension | This week |
| **4** | Close determinism PARTIALs (HashMap audit, per-step replay, CI parity) | After Task 2 |
| **10** | Reuse counters: warm hits vs cold authorings per SituationKey | **Do not defer** |
| **5–6** | Effect contracts, op catalog, trybuild, time module | Substrate |
| **8** | Residue adversarial test set (7 cases) | Alongside 6 |
| **9** | Convergence map | **Done** → `BRIDGE_HKV4_CONVERGENCE.md` |
| **7** | Starlark embedding | Last |

---

## One-line summary

We audited the HKv4 checklist against real code, fixed seven orchestration/determinism bugs, removed a fake eval gate, documented honest gaps, ratified hybrid Starlark codegen (F-034), and set Phase 2 to **port harness-core and measure reuse** before wiring Starlark.

---

## Next: execution + acceptance

**Immediate work:** [`PHASE_2_ACCEPTANCE_SUITE.md`](PHASE_2_ACCEPTANCE_SUITE.md) — Sessions A–C (port, close four debts, reuse counters) and 60+ acceptance tests defining "working perfectly."

**Master status:** [`AELIO_PHASE_FINAL_STATUS.md`](AELIO_PHASE_FINAL_STATUS.md) — what's done vs not done (read this first).
