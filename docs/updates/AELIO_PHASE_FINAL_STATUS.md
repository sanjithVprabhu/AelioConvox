# Aelio Verification & Phase 2 — Final Status

**Date:** 2026-08-07  
**Authority:** HKv4 · [`FLAGS.md`](../../FLAGS.md) F-033, F-034, F-035

This is the **one document** to read for: what is done, what is not, and what to do next.

---

## TL;DR

| Phase | Status |
|-------|--------|
| **Phase 1** — verification audit + 8 bug fixes + docs | **DONE** |
| **Phase 2 Sessions A–C** — port, debts, counters | **DONE** |
| **Phase 2 Sessions D+** — substrate, Starlark, full acceptance suite | **NOT STARTED** (D0 blocks) |

Phase 1 and Phase 2 Sessions A–C are complete in repo. **Session D0** ([`PHASE_2D_INSTRUCTIONS.md`](PHASE_2D_INSTRUCTIONS.md)) must run first — 40 vs 54 harness-core tests. Then D1–E and completion gates.

---

## What we finished (Phase 1)

### Code fixes (merged in repo)

| # | Fix | File(s) |
|---|-----|---------|
| 1 | Promotion uses turn's `SituationKey`, not rebuilt σ | `blocks/orchestrate.rs`, `blocks/turn.rs` |
| 2 | `TaskGraph::canonical_hash` via single `SolValue` serialiser | `aelio-sol/src/task_graph.rs` |
| 3 | Wavefront `args_hash` via `value_to_sol` → `value_hash` | `wavefront.rs`, `ops/pure.rs` |
| 4 | `NeedUser` → suspended turn, not ProposePath fall-through | `blocks/orchestrate.rs` |
| 5 | Turn spine doc: orchestrate before Conductor on Tier3 | `blocks/turn.rs` |
| 6 | Effectful detection via `path_is_effectful` | `orchestration/promotion.rs` |
| 7 | Determinism tests: `-0.0`, float sum, list disambiguation | `canonical_conformance.rs` |
| 8 | Fake eval gate removed; shape-only + fail-closed on exhaustion | `orchestration/executor.rs` |

---

## What we finished (Phase 2 Sessions A–C)

### Session A — Port + one serialiser

| Step | Status | Notes |
|------|--------|-------|
| Port `harness-core` over stubs | **DONE** | numeric, agg, exec, taint, versioning — 40 tests |
| Re-pin reference hashes | **DONE** | `tests/determinism.rs` |
| Collapse dual canonical writer | **DONE** | `TaskGraph::to_sol_value()` → `aelio_sol::canonical` |
| CI grep: one serialiser | **DONE** | `scripts/check-canonical-writer.sh` in CI |
| debug == release hashes | **DONE** | harness-core + aelio-sol green |

### Session B — Four changelog debts

| Debt | Status | Test |
|------|--------|------|
| Typed `JournalUnderrun` | **DONE** | `aelio-kernel/tests/journal_underrun.rs` (A7) |
| `GraphSuspension` wired | **DONE** | resume path in `orchestrate.rs` + World persistence (B7 partial — unit round-trip) |
| Wavefront HashMap audit | **DONE** | `ExecutorState` uses `BTreeSet`/`BTreeMap` |
| Per-step replay | **DONE** | `harness-core::verify_replay` + kernel `replay_steps.rs` (A1 partial) |

### Session C — Reuse counters

| Step | Status |
|------|--------|
| Raw counters on bridge | **DONE** | `reuse_metrics.rs` |
| Daily log, no ratios | **DONE** | `daily_log_line()` |
| Wired to lookup tier | **DONE** | `turn.rs` after `lookup_tier` |

**Tests passing (last run):**
```bash
cd aelio-os
bash ../scripts/check-canonical-writer.sh
cargo test -p harness-core                    # 40
cargo test -p aelio-sol task_graph            # 6
cargo test -p aelio-kernel --test journal_underrun --test replay_steps  # 3
cargo test -p aelio-agent reuse_metrics suspension orchestrate
```

---

## What we did NOT finish (Phase 2 Sessions D+)

### Sessions D+ (next)

- [ ] Effect contract registration (Task 5)
- [ ] Op catalog, trybuild, time module (Task 6)
- [ ] Residue adversarial set C1–C9 (Task 8)
- [ ] Starlark embedding (Task 7 — last)
- [ ] Full 60+ acceptance tests in [`PHASE_2_ACCEPTANCE_SUITE.md`](PHASE_2_ACCEPTANCE_SUITE.md)
- [ ] Cross-arch CI parity job expansion beyond `aelio-sol` (A3)
- [ ] Process-restart hash drill (A4) as dedicated integration test

---

## Open debts (remaining)

| Debt | Severity | Session | Acceptance test |
|------|----------|---------|-----------------|
| Residue: only Bangalore case | CORR | D+ | C1, C9 |
| Matching section C not built | — | D+ | C1–C9 |
| GraphSuspension B7 end-to-end | CORR | D+ | B7 full client-tool round-trip |
| Starlark / effect driver | BLOCKER | D+ | H*, I* |

**Closed in Sessions A–C:** dual canonical writer, harness-core stubs, JournalUnderrun, wavefront HashMap, per-step replay helper, reuse counters.

---

## Strategic decisions (locked)

1. **Product:** reuse under proof — match skeleton + bindings, or author once and save.
2. **Codegen (F-034):** model may emit Starlark text; **parsed AST is authoritative**; hash from canonical re-render, never raw text.
3. **Bridge:** orchestration stays until substrate closed; must converge to HKv4 ([`BRIDGE_HKV4_CONVERGENCE.md`](BRIDGE_HKV4_CONVERGENCE.md)).
4. **Starlark:** do not wire until Sessions D+ substrate closed (Section 8 = KILL if wired early).

---

## Acceptance suite — what "working perfectly" means

Full spec: [`PHASE_2_ACCEPTANCE_SUITE.md`](PHASE_2_ACCEPTANCE_SUITE.md)

**Tests worth knowing by name:**

| ID | Why it matters |
|----|----------------|
| **A4** | Process-restart hash diff — only test that catches HashMap in hash path |
| **B4** | "weight over 40" reuses height-over-28 skeleton — thesis in one test |
| **B5** | "between 28 and 40" must NOT bind — mirror of B4 |
| **C3** | "median" vs cached mean — assert **verification** gate, not just failure |
| **H2** | Corrupt effect-set both ways — `AstExceedsEffectSet` is I5, not generic integrity |
| **H6** | Prompt injection in tool data — pass = injection does nothing |
| **I1** | Kolkata midnight — cache must miss at tenant rollover |
| **J** | Not pass/fail — executions-per-key distribution is the verdict |

**Scoring rule:** Section **C red is worse than down** (disable matching until green). **A + D + E red → stop.**

---

## Document map

```
docs/updates/
├── AELIO_MASTER_PLAN.md            ← DONE vs REMAINING + detailed plan (planning doc)
├── AELIO_IMPLEMENTATION_FINAL.md   ← complete implementation record (Phase 1 + 2 A–C)
├── AELIO_PHASE_FINAL_STATUS.md     ← YOU ARE HERE (master status)
├── PHASE_1_CHANGELOG.md            ← what changed in Phase 1 (detail)
├── IMPLEMENTATION_VERIFICATION_REPORT.md  ← audit scorecard
├── PHASE_2_INSTRUCTIONS.md         ← full task list
├── PHASE_2_ACCEPTANCE_SUITE.md     ← Sessions A–C + 60+ tests
├── BRIDGE_HKV4_CONVERGENCE.md      ← bridge → HKv4 table
└── IMPLEMENTATION_VERIFICATION.md  ← original checklist
```

---

## Next action (start here)

1. Run **Session D0** per [`PHASE_2D_INSTRUCTIONS.md`](PHASE_2D_INSTRUCTIONS.md): reconcile 40→54 tests, API shapes, freeze hashes.
2. Run **D1–D6** in order; Starlark (Session E) last.
3. Gates 1–7 green + Gate J verdict recorded before declaring fully complete.

---

## One-line summary

**Phase 1 done:** eight bugs fixed, honest audit, FLAGS F-033/034/035. **Phase 2 Sessions A–C done:** harness-core ported, canonical writer collapsed, JournalUnderrun typed, GraphSuspension wired, reuse counters on bridge. **Phase 2 D+ not started:** full acceptance suite, residue adversarial set, Starlark.
