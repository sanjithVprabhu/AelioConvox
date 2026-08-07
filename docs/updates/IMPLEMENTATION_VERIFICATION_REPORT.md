# Implementation Verification — Phase 1 Report (rev. 2)

**Date:** 2026-08-07  
**Scope:** Line-by-line audit of [`IMPLEMENTATION_VERIFICATION.md`](IMPLEMENTATION_VERIFICATION.md) against `aelio-os`, Phase 1 bug fixes, and Phase 2 work order.  
**Work order:** [`PHASE_2_INSTRUCTIONS.md`](PHASE_2_INSTRUCTIONS.md)  
**Convergence map:** [`BRIDGE_HKV4_CONVERGENCE.md`](BRIDGE_HKV4_CONVERGENCE.md)

---

## Executive summary

Phase 1 fixed **7 real bugs** in the orchestration bridge (promotion keys, hashing paths, NeedUser suspension, effectful detection, determinism tests). Gaps were stated honestly rather than papered over.

**Rev. 2 corrections** (from review of this report):

1. **Starlark codegen** — F-034 records a **hybrid**: model may emit text, but parsed AST is authoritative; hashes from canonical re-render, never raw text (§8.3 parse-failure class).
2. **`harness-core`** — Full implementation (2,111 lines, 54 tests) exists in **external deliverables**; Phase 1 incorrectly scaffolded stubs. Task 2 is **port, not reimplement**. Phase 1 fix #2 also created a **second canonical writer** in `aelio-sol` — must reconcile to one serialiser (finding-grade).
3. **Fake gates** — F-035; semantic eval stub removed; shape-only gate + fail-closed on refinement exhaustion (Task 3 in progress).
4. **Reuse measurement** — Task 10 (warm vs cold counters per SituationKey) is **do-not-defer**; not yet instrumented.
5. **Scorecard fixes** — Q103/Q104 were miscategorised as FIXED; Q22 downgraded to PARTIAL pending typed `JournalUnderrun`.

---

## Strategic direction (ratified + corrected)

| Decision | Status |
|----------|--------|
| HKv4 + `IMPLEMENTATION_VERIFICATION.md` as north star | F-033 |
| **Hybrid Starlark codegen** (text emit → parse → AST authority → canonical hash) | **F-034** — not pure text-hashing |
| Orchestration bridge until Starlark substrate closed | Active |
| Do not wire Starlark before taint/budget/versioning | Unchanged |
| Bridge must converge to HKv4, not become permanent | [`BRIDGE_HKV4_CONVERGENCE.md`](BRIDGE_HKV4_CONVERGENCE.md) |

The product objective (restated in Phase 2 instructions): **reuse under proof** — match a discovered workflow or author once and save for future requests of the same shape. Starlark, numeric tower, and taint enforce the five invariants; they are not features in themselves.

---

## Phase 1 — Bugs fixed

| # | Issue | Fix | Files |
|---|-------|-----|-------|
| 1 | Promotion used wrong `SituationKey` | Pass turn's σ into `try_orchestrated_cold_path` | `orchestrate.rs`, `turn.rs` |
| 2 | `TaskGraph::canonical_hash` used serde | Manual `canonical_bytes()` + BLAKE3 | `task_graph.rs` — **must reconcile with harness-core canonical (Task 2)** |
| 3 | Wavefront `args_hash` used serde | `value_to_sol` → `value_hash` | `wavefront.rs`, `pure.rs` |
| 4 | NeedUser fell through to re-plan | Suspended `TurnResult` + question | `orchestrate.rs` |
| 5 | Doc: orchestrate after Conductor | Comment corrected: runs **before** on Tier3 | `turn.rs` |
| 6 | Effectful heuristic substring match | `path_is_effectful(registry, &path)` | `promotion.rs` |
| 7 | Missing determinism tests | `-0.0`, float sum, list disambiguation | `canonical_conformance.rs` |

---

## Phase 1 — Known debt introduced (must fix in Phase 2)

| Debt | Severity | Task |
|------|----------|------|
| Second canonical writer in `aelio-sol/task_graph.rs` alongside future `harness_core::canonical` | **KILL** | Task 2.2 |
| Stubs instead of ported `harness-core` | **BLOCKER** | Task 2 |
| Semantic eval gate was fake (`pass: true`) | **SEC** | Task 3 — shape-only + fail-closed applied (F-035) |
| `GraphSuspension` types unused | **CORR** | Task 3.3 — wire or delete |
| No reuse counters | **Highest information** | Task 10 — do not defer |

---

## FLAGS entries

| ID | Topic |
|----|-------|
| F-033 | Framework authority → HKv4 + Starlark |
| F-034 | Hybrid codegen: text emit, AST authority, canonical re-render hash |
| F-035 | Fail-closed gating; fake semantic stub removed |

---

## Verification checklist — scorecard (rev. 2)

Legend: **PASS** · **PARTIAL** · **GAP** · **FIXED** · **DEBT**

### Section 0

| Q | Verdict | Notes |
|---|---------|-------|
| 1 | PARTIAL | Bridge + sol/kernel exist; harness-core **not ported** |
| 2 | PARTIAL | Failure tests in sol/kernel; orchestration shape gate only |
| 3 | PARTIAL | NeedUser fixed; semantic fake gate **removed** (F-035); GraphSuspension unwired |
| 4 | PARTIAL | F-033 = authority shift, not micro-decision log |
| 5 | PASS | Workspace + CI |

### Section 1 — Determinism [KILL]

| Q | Verdict | Notes |
|---|---------|-------|
| 6–7 | DEBT | serde removed from agent hashes; **two canonical writers** remain until Task 2 |
| 8–11 | PASS | Determinism tests added |
| 12 | GAP | No Decimal until harness-core port |
| 13–14 | PARTIAL | sol clippy ban; wavefront HashMap unaudited (Task 4a) |
| 22 | **PARTIAL** | `ReplayBackend::pop` refuses past ledger with `ReasonCode::Internal` — behaviour correct, **not typed `JournalUnderrun`** for nightly paging (Task 4b) |
| 20–21, 23–24 | PARTIAL / GAP | Per Phase 2 Task 4 |

### Sections 2–8

Mostly **GAP** until harness-core port (Task 2) and Starlark (Task 7). Residue Q97: one Bangalore case only — adversarial set required (Task 8).

### Section 10 — Meta (corrected)

| Q | Verdict | Answer |
|---|---------|--------|
| 100 | PARTIAL | No debug/release hash CI for full workspace |
| **103** | **OPEN** | Unilateral micro-decisions — running list (append per session): |
| | | • TaskGraph canonical writer hand-rolled in aelio-sol (should delegate to harness-core) |
| | | • NeedUser → suspended turn without GraphSuspension persistence (minimal fix; resume TBD) |
| | | • F-034 hybrid text/AST (ratified; not silent text-hashing) |
| **104** | **OPEN** | What would pass tests but be wrong: |
| | | • Semantic gate approving everything (was true; fixed F-035) |
| | | • Promotion with wrong σ (was true; fixed) |
| | | • Two canonical writers producing divergent hashes for same logical graph |
| | | • Warm path hit without executions-per-key telemetry (unmeasurable reuse thesis) |
| | | • Residue check with only Bangalore case (other six adversarial cases un tested) |

---

## Phase 2 work order (sequencing)

```
Task 1 (F-034 decision)     ── done
Task 2 (port harness-core) ── FIRST coding task — deliverables → aelio-os/crates/harness-core/
Task 3 (fake gates)         ── in progress (F-035 executor fix)
Task 4 (KILL partials)      ── after Task 2
Task 10 (reuse counters)    ── parallel, do-not-defer
Task 5, 6                   ── substrate completion
Task 8                      ── residue adversarial set
Task 9                      ── BRIDGE_HKV4_CONVERGENCE.md done
Task 7                      ── Starlark last
```

**Phase 2 done when:** harness-core ported (54+ tests, one canonical serialiser); zero fake gates; determinism PARTIALs at PASS; residue adversarial set green; reuse counters producing daily numbers; then Starlark.

---

## Tests (Phase 1 baseline)

```bash
cd aelio-os
cargo test -p aelio-sol                    # 23 pass
cargo test -p aelio-agent orchestration    # 32 pass (post F-035)
cargo test -p aelio-agent blocks::orchestrate
```

After Task 2: `cargo test -p harness-core` → 54+ pass, debug == release hashes.

---

## References

- Checklist: [`IMPLEMENTATION_VERIFICATION.md`](IMPLEMENTATION_VERIFICATION.md)
- Work order: [`PHASE_2_INSTRUCTIONS.md`](PHASE_2_INSTRUCTIONS.md)
- Convergence: [`BRIDGE_HKV4_CONVERGENCE.md`](BRIDGE_HKV4_CONVERGENCE.md)
- FLAGS: [F-032](../../FLAGS.md), [F-033](../../FLAGS.md), [F-034](../../FLAGS.md), [F-035](../../FLAGS.md)
