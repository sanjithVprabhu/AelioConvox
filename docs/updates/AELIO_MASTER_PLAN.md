# Aelio — Master Plan: Done, Remaining, and How to Finish

**Purpose:** One document you can read to understand what was built, what is still missing, and how to plan the rest. Use this as context when asking an agent to implement the next session.

**Date:** 2026-08-07  
**Authority:** HKv4 · [`HARNESS_KERNEL_V4.md`](HARNESS_KERNEL_V4.md) · [`IMPLEMENTATION_VERIFICATION.md`](IMPLEMENTATION_VERIFICATION.md) · [`FLAGS.md`](../../FLAGS.md)

**Related docs:**

| Doc | Use when |
|-----|----------|
| [`AELIO_EXECUTION_PLAYBOOK.md`](AELIO_EXECUTION_PLAYBOOK.md) | **How** to implement each session, monitoring, finish-line trace |
| [`AELIO_IMPLEMENTATION_FINAL.md`](AELIO_IMPLEMENTATION_FINAL.md) | Quick inventory of landed code |
| [`PHASE_2D_INSTRUCTIONS.md`](PHASE_2D_INSTRUCTIONS.md) | D0 test list + completion gates (technical) |
| [`PHASE_2_ACCEPTANCE_SUITE.md`](PHASE_2_ACCEPTANCE_SUITE.md) | Every acceptance test ID (A–J) |
| [`PHASE_2_INSTRUCTIONS.md`](PHASE_2_INSTRUCTIONS.md) | Original Tasks 1–10 narrative |

---

# Part 1 — What we are building (so the plan makes sense)

## The product thesis

Aelio’s goal is **reuse under proof**:

1. A user asks something (e.g. “average height of people over 28”).
2. The system either **matches** a previously saved abstract workflow (skeleton + new bindings) or **authors once** and saves it.
3. Every execution is **deterministic**, **auditable**, and **authorised before effects run**.

The five invariants (from HKv4):

| Invariant | Plain English |
|-----------|---------------|
| **I1 Determinism** | Same inputs → same bytes/hashes everywhere (debug/release, x86/ARM, replay). |
| **I2 Pre-execution authorisation** | Effects cannot run until policy/verification passes. |
| **I3 Precision over recall** | Better to cold-author than bind the wrong skeleton. |
| **I4 Auditability** | Ledger + replay; no silent side effects on replay. |
| **I5 Content-addressed trust** | Hashes over canonical forms; verified effect sets. |

## Two layers (do not confuse them)

```
┌─────────────────────────────────────────────────────────────┐
│  BRIDGE (aelio-agent) — working today                         │
│  SituationKey → LookupTier → TaskGraph → promotion → warm   │
│  This is scaffolding; must converge to HKv4, not replace it   │
└─────────────────────────────────────────────────────────────┘
                              ↓ converges to
┌─────────────────────────────────────────────────────────────┐
│  SUBSTRATE (harness-core + Starlark) — partially built        │
│  numeric · agg · taint · exec · versioning → Starlark ops     │
│  This is how invariants are enforced long-term                │
└─────────────────────────────────────────────────────────────┘
```

**Risk:** The bridge becomes permanent → two reuse systems, two hash schemes, drift. Every remaining task should answer: *does this move the bridge toward HKv4, or entrench it?*

## The reuse loop (bridge today)

```
User utterance
    → SituationKey σ (state + intent + slots + caps)
    → LookupTier:
         Tier0  exact σ hash hit        → warm (zero LLM)
         Tier1  embedding near-hit      → warm
         Tier2  compose procedures      → cold-ish
         Tier3  orchestrate / propose   → cold (author)
    → Execute TaskGraph or AbilityPath
    → On success: promote skeleton for future Tier0/1 hits
```

**Reuse counters** (Session C) now record `cold_executions`, `warm_hits`, `executions_per_key` so you can later judge if reuse is real (Gate J).

---

# Part 2 — What is DONE (detailed)

## Phase 1 — Verification audit + 8 bridge fixes ✅

These fixed real bugs found walking the 104-point checklist. Each one closed a path to silent wrong behaviour.

### Fix 1 — Promotion used wrong SituationKey

**Problem:** After orchestration succeeded, promotion stored the TaskGraph under a **rebuilt** σ (empty slots, raw clause) instead of the turn’s real σ. Warm lookup could never find promoted graphs.

**Fix:** Pass the turn’s `SituationKey` from `turn.rs` into `observe_task_graph_success`.

**Files:** `blocks/orchestrate.rs`, `blocks/turn.rs`

**Why it mattered:** The entire “author once, reuse forever” loop was broken at the storage step.

---

### Fix 2 — TaskGraph hash unstable (then fixed again in Phase 2)

**Phase 1:** Replaced `serde_json` with a manual §4.3 writer (~300 lines).  
**Phase 2:** Collapsed onto **one** serialiser: `TaskGraph::to_sol_value()` → `aelio_sol::canonical`.

**File:** `aelio-sol/src/task_graph.rs`

**Why it mattered:** Promotion evidence and matching depend on stable content hashes.

---

### Fix 3 — Wavefront args_hash used serde_json

**Problem:** Idempotency keys for node args were hashed via JSON, not §4.3 canonical form.

**Fix:** `value_to_sol()` → `value_hash()` in `hash_args()`.

**Files:** `orchestration/wavefront.rs`, `ops/pure.rs`

---

### Fix 4 — NeedUser fell through to ProposePath

**Problem:** When a client tool needed a parameter, orchestration didn’t suspend — it fell through and the LLM might guess.

**Fix:** `GraphExecution::NeedUser` → suspended `TurnResult` with `opened_loop: true`.

**File:** `blocks/orchestrate.rs`

---

### Fix 5 — Doc: orchestrate before Conductor on Tier3

**Fix:** Comment in turn spine clarifies order: composite queries try orchestration **before** Conductor starters.

**File:** `blocks/turn.rs`

---

### Fix 6 — Effectful promotion used string heuristics

**Problem:** Promotion could treat paths as non-effectful based on string matching.

**Fix:** `path_is_effectful(registry, &path)`.

**File:** `orchestration/promotion.rs`

---

### Fix 7 — Determinism edge tests

**Added:** `-0.0`/`0.0`, float sum order, list disambiguation in `canonical_conformance.rs`.

---

### Fix 8 — Fake semantic eval removed

**Problem:** Eval gate returned `pass: true` unconditionally — downstream believed verification ran.

**Fix:** Shape-only evaluation; hard error when refinement budget exhausted.

**File:** `orchestration/executor.rs`

---

### Phase 1 documents ✅

- `IMPLEMENTATION_VERIFICATION_REPORT.md` — audit scorecard  
- `PHASE_1_CHANGELOG.md` — detailed changelog  
- `BRIDGE_HKV4_CONVERGENCE.md` — bridge → HKv4 map  
- FLAGS **F-033, F-034, F-035**

---

## Phase 2 Session A — Substrate + single serialiser ✅ (with caveat)

### A.1 — `harness-core` crate

**Location:** `aelio-os/crates/harness-core/`

| Module | What it does |
|--------|--------------|
| `numeric.rs` | Int / Float / Decimal; type-strict ops; banker's rounding |
| `agg.rs` | Aggregates with explicit null policy; `Empty{reason}`; count_rows vs count_values |
| `exec.rs` | `Step`, `verify_replay`, `BudgetPool` with **CAS** drain (not fetch_sub) |
| `taint.rs` | Per-value taint, map-key absorption, 9 prohibited positions, PC-taint |
| `versioning.rs` | `SystemVersion`, `Verified<T>`, op `impl_hash`, Merkle root |
| `lib.rs` | Re-exports + `aelio_sol` canonical |

**Tests today:** 40 (not 54)  
**Pinned hashes:** `a66bcd6a…`, `0140b77a…` in `tests/determinism.rs`

⚠️ **Caveat (F-036):** Reimplemented from spec, **not ported** from original 54-test bundle. Session **D0 must reconcile** before trusting substrate.

---

### A.2 — Canonical writer collapse ✅

- Deleted ~300 lines of manual `write_*` in `task_graph.rs`
- Single path: `to_sol_value()` → `aelio_sol::canonical::to_bytes` / `value_hash`

---

### A.3 — CI guard ✅

- `scripts/check-canonical-writer.sh` — fails if second canonical writer appears
- Wired in `.github/workflows/ci.yml`

---

## Phase 2 Session B — Four debts ✅ (one partial)

### B.1 — Typed JournalUnderrun ✅

**What:** Replay past end of journal → `ReasonCode::JournalUnderrun` (`Journal.Underrun`), **never** dispatches tools.

**Files:** `aelio-kernel/src/error.rs`, `driver.rs`, `tests/journal_underrun.rs`

**Acceptance:** A7 (kernel level)

---

### B.2 — GraphSuspension wired ⚠️ PARTIAL

**What works:**
- Types in `orchestration/suspension.rs`
- NeedUser saves `GraphSuspension` on `TurnResult`
- `World.user_graph_suspensions` persists per user
- Next turn: `try_resume_orchestrated_graph` in `orchestrate.rs`

**What’s missing:** Full **B7** E2E — client tool with unbound param → user answers → graph completes → both turns linked in ledger. Negative path: bad answer → re-Clarify.

---

### B.3 — Wavefront HashMap audit ✅

**Change:** `ExecutorState.completed` → `BTreeSet`; `outputs` → `BTreeMap`

**File:** `orchestration/wavefront.rs`

**Note:** A4 process-restart drill still not automated (D1).

---

### B.4 — Per-step replay ⚠️ PARTIAL

**What works:** `harness-core::verify_replay` + kernel `tests/replay_steps.rs` (verifier logic)

**What’s missing:** Kernel/agent **does not emit** `Step` records during live graph execution (D2).

---

## Phase 2 Session C — Reuse counters ✅

**Module:** `aelio-agent/src/reuse_metrics.rs`

| Counter | When incremented |
|---------|-------------------|
| `warm_hits` | Tier0 or Tier1 lookup |
| `cold_executions` | Tier2 or Tier3 lookup |
| `executions_per_key` | Every lookup, keyed by situation hash |

**API:** `record_lookup()`, `snapshot()`, `daily_log_line()` (raw only, no ratios)

**Wired in:** `blocks/turn.rs` after `lookup_tier`

**Gate J:** Counters live; **verdict not yet recorded** (needs ~1 week of traffic + manual review).

---

# Part 3 — What is PARTIAL (landed but not trustworthy yet)

| Item | Status | Why partial | Session to finish |
|------|--------|-------------|-------------------|
| harness-core tests | 40/54 | Spec rewrite missed contested behaviours | **D0** |
| Per-step replay | Verifier only | No live Step emission | **D2** |
| GraphSuspension | Unit + wiring | No B7 E2E | **D6** |
| aarch64 determinism | aelio-sol only | harness-core not on ARM CI | **D1** |
| A4 restart drill | Not scripted | HashMap-in-hash-path undetected | **D1** |
| Residue matching | Bangalore only | C1–C9 not built | **D5** |
| Effect contracts | Partial in bridge | D7 registration gate open | **D3** |
| Semantic verification | Removed fake pass | Real verification gate missing (C3) | **D5** + eval work |
| Starlark | Not started | Blocked on substrate + taint + budget | **E** |

---

# Part 4 — What is NOT DONE (detailed remaining work)

## Session D0 — Reconcile harness-core (BLOCKER — do first)

### Why this blocks everything

The spec alone does not encode **seven build findings** discovered during original implementation. A spec-reader rewrite likely re-made wrong choices for:

1. **Division scale** — Int/Int → Decimal at what scale?
2. **MAX_SCALE clamp** — overflow behaviour
3. **NullEncountered** — three null policies on `[1,null,3]`
4. **CAS vs fetch_sub** — budget pool concurrent drain
5. **JournalUnderrun** — (fixed in kernel; harness exec tests may still gap)
6. **Map-key taint** — construction + extraction directions
7. **Implicit flow** — PC-taint through nested branches

### What to do

1. Walk the 54-test checklist in [`PHASE_2D_INSTRUCTIONS.md`](PHASE_2D_INSTRUCTIONS.md) Part 0 — grep each name
2. Implement missing tests + fix implementation until all green
3. Run API-shape checks (D0.2): `Verified<T>` privacy, exhaustive `SystemVersion.hash()`, CAS not fetch_sub
4. Freeze hash provenance (D0.3) — re-pin once if serialisation changes

### High-priority missing tests (bet list)

| Test | Why a spec-reader skips it |
|------|---------------------------|
| `float_sum_depends_on_order_which_is_why_ordering_is_mandated` | Documents *why* ordering rules exist (agg-level, not naive f64) |
| `concurrent_takes_cannot_overshoot` | Proves CAS; fetch_sub fails intermittently |
| `bankers_rounding_ties_to_even` (negative ties) | Half-even breaks on negative |
| `the_adversarial_null_case_from_the_golden_set` | Three policies, not two |
| `implicit_flow_through_branch_selection_is_caught` | PC-taint semantics |

### Exit criteria

- [ ] ≥54 tests green, debug == release
- [ ] All D0.2 API checks pass
- [ ] Hash comment + FLAGS entry if re-pinned
- [ ] Q103 list updated for any behavioural decisions

### Context to give an agent

> “Implement Session D0 per PHASE_2D_INSTRUCTIONS.md Part 0. Start by grepping each required test name. Add missing tests and fix harness-core until ≥54 pass. Do not start D1 until D0 exit checklist is complete.”

---

## Session D1 — CI and determinism drills

### Goals

1. **harness-core on aarch64 CI** — extend matrix; pinned hashes must match x86_64 (M1 exit gate)
2. **A4 process-restart script** — run hash suite → new process → run again → diff must be empty
3. **Fast-math grep** + `preserve_order` feature-tree check workspace-wide

### Why it matters

- Single-process tests can pass with HashMap in hash paths; **restart** catches per-process seed bugs
- ARM Mac users would see silent hash divergence without aarch64 job

### Exit criteria

- [ ] CI job runs `cargo test -p harness-core` on `ubuntu-24.04-arm`
- [ ] Pinned hashes identical x86_64 vs aarch64
- [ ] `scripts/restart-hash-drill.sh` (or similar) green locally and in CI

---

## Session D2 — Per-step replay in real execution

### Current gap

`verify_replay` exists in harness-core. The **agent wavefront executor** does not record `Step { seq, inputs_hash, output_hash }` when nodes complete.

### What to build

1. At each wavefront node completion, append a `Step` to a turn-local trace
2. Persist trace alongside orchestration ledger (or embed in durable turn record)
3. Integration test: 3-node TaskGraph live → save trace → replay → corrupt middle step → `diverged_at == seq of middle`

### Files likely touched

- `orchestration/wavefront.rs` — emit step on node complete
- `orchestration/executor.rs` — accumulate trace
- `runtime/durable.rs` — persist if durable path needed
- New test: `tests/orchestration_replay_steps.rs`

### Exit criteria (Gate 2)

- [ ] Live execution produces step trace
- [ ] Corrupted step localised by seq
- [ ] A1 acceptance test green at agent layer (not just harness-core unit test)

---

## Session D3 — Effect contracts (Task 5)

### Goals

- Tool registration **hard-fails** without `effect_class`, `completeness`, `returns_entity`
- Backfill existing demo/catalog tools
- `Completeness::Unknown|Sampled` blocks aggregate promotion
- Idempotency keys: `(turn_key, effect_seq, element_index)`

### Why it matters

Without this, promotion and shadow runs can treat effectful tools as safe. D7 acceptance test.

### Files likely touched

- `abilities/registry.rs`, `tenant/` tool specs
- `orchestration/promotion.rs` — block promotion on incomplete completeness
- Tests for D7

---

## Session D4 — Op catalog + trybuild + time module (Task 6)

### Three workstreams

**4a. Op catalog**

- Register harness-core ops with `impl_hash`
- Merkle root → `SystemVersion.op_catalog`
- Document each `std.*` bridge tool: catalog op vs bridge-only

**4b. Trybuild (structural closure)**

- `tests/ui/unverified_effect_set.rs` — raw value into `authorize` must not compile
- SystemVersion missing-field → compile fail (proves exhaustive destructuring)

**4c. Time module**

- UTC epoch ms, pinned tzdb in-crate
- `TemporalBinding`, tenant-tz calendar buckets
- **Urgent check:** does bridge warm path cache time-relative windows? If yes, fix before matching goes live

### Exit criteria

- Gate 1 trybuild items
- I1–I4 time tests (Gate 6) — can start after time module lands

---

## Session D5 — Residue adversarial set C1–C9 (Task 8)

### The red rule

**Any C-case failing → disable matching entirely (cold-path everything) until green.** Confidently wrong is worse than down.

### Each case (must assert **named gate**)

| ID | Scenario | Gate that must fire |
|----|----------|---------------------|
| C1 | + “in Bangalore” | **residue** |
| C2 | Missing required filter | **bind** |
| C3 | “median” vs cached mean | **verification** (not residue!) |
| C4 | `>= 28` vs `> 28` | binding correctness |
| C5 | weight vs height | bind |
| C6 | employees vs people | bind |
| C7 | null-policy variant | structure (§6.2) |
| C8 | “typical height” | implicit-term (§7.7) |
| C9 | High embed, fails residue | fail-closed → author |

Also: **Bangalore clause-drop test** — can σ-matcher bind while silently dropping a qualifier?

### Files likely touched

- `abilities/residue.rs` — expand beyond Bangalore
- `abilities/learn.rs` — matching gates
- `tests/residue_adversarial.rs` (new)
- Bridge integration tests B4, B5

---

## Session D6 — GraphSuspension B7 end-to-end

### Happy path

1. Client tool with required unbound param
2. `Orchestrate.NeedUser` → suspended turn + `GraphSuspension` saved
3. User provides answer next turn
4. `try_resume_orchestrated_graph` completes graph
5. Ledger links both turns

### Negative path

User answer unusable → re-Clarify (not crash, not guess)

### Exit criteria

- [ ] B7 acceptance test green
- [ ] Gate 6 B7 item checked

---

## Session E — Starlark (LAST — F-033)

**Do not start until Gates 1–4 are green.**

Order within Session E:

1. Starlark toolchain pinned
2. Dialect lockdown asserted at startup (H3)
3. Tainted value type from day one
4. Budget threaded through interpreter (F1, heap-limit test)
5. `spawn_blocking` effect bridge
6. Runtime cycle check (A→B→A rejected)
7. F-034 codegen: text → parse → AST authoritative → canonical re-render → hash
8. First LLM-authored harness: cache hit on re-request (Gate 7)

---

## Gate J — Reuse verdict (continuous, not pass/fail)

**Already live:** reuse counters from Session C.

**After ~1 week of traffic:**

1. Pull `executions_per_key` distribution
2. Record verdict in FLAGS **before Session E**:
   - **Top-heavy tail** → thesis supported, full speed
   - **Flat median ≈ 1** → check SituationKey fragmentation before concluding premise wrong
3. J3 manual: 20 warm hits re-run cold; disagreements → golden case + gate bug

---

# Part 5 — Completion gates (plain English)

Gates are **ordered**. A red gate blocks everything after it.

| Gate | Proves | Blocks |
|------|--------|--------|
| **1 Substrate** | Hashes stable everywhere; ≥54 tests; one serialiser; trybuild | All |
| **2 Execution** | Replay finds exact step; underrun never dispatches; budget exact | Matching, Starlark |
| **3 Effects** | Writes cannot double-fire; registration complete | Real tools |
| **4 Taint** | Exfiltration paths closed | Starlark conditionals |
| **5 Matching** | C1–C9 + B4/B5; red rule enforced | Reuse on real traffic |
| **6 Sessions/time** | Idempotency, suspension E2E, timezone correctness | External users |
| **7 Starlark** | LLM harness executes + caches | “Fully complete” |
| **J Verdict** | Does the world want reuse? | Product decision, not code |

**Scoring (from acceptance suite):**

| Section red | Action |
|-------------|--------|
| A + D + E | **Stop** — invariants broken |
| C | **Disable matching** until green |
| B | Loop doesn’t loop — fix before measuring J |
| F/G/H/I | Ship-blockers, not architecture-invalidators |
| J | Verdict only |

---

# Part 6 — Recommended implementation plan

## Order (strict)

```
D0  harness-core 54-test reconciliation     ← START HERE
 ↓
D1  aarch64 CI + A4 restart drill
 ↓
D2  live Step traces in wavefront executor
 ↓
D3  effect contracts + D7
 ↓
D4  op catalog + trybuild + time module
 ↓
D5  residue C1–C9 + B4/B5 + Bangalore test
 ↓
D6  GraphSuspension B7 E2E
 ↓
    [ Gate J: 1 week counters review → FLAGS ]
 ↓
E   Starlark (only if Gates 1–4 green)
```

## Rough effort (for planning)

| Session | Complexity | Depends on |
|---------|------------|------------|
| D0 | High (many small tests + subtle numeric/taint) | Nothing |
| D1 | Medium (CI + scripts) | D0 |
| D2 | Medium | D0 |
| D3 | Medium | D0 |
| D4 | High (time module is large) | D0, D3 partial |
| D5 | High (9 integration tests + gates) | D3, bridge stable |
| D6 | Medium | B.2 wiring (done) |
| E | Very high | Gates 1–4 |

## Parallelisation (if multiple people/agents)

- **After D0:** D1 and D2 can run in parallel
- D3 and D4a (catalog) can overlap
- D5 must wait for D3 (effect/completeness gates affect C7)
- D6 can run anytime after D0
- **Never** parallelise Starlark (E) with D0

---

# Part 7 — How to give an agent correct context

Copy-paste templates for each session:

### D0 template

```
Authority: AELIO_MASTER_PLAN.md + PHASE_2D_INSTRUCTIONS.md Part 0 + FLAGS F-036.
Task: Session D0 — reconcile harness-core from 40 to ≥54 tests.
Do: grep each required test name; implement missing tests and fix code; D0.2 API checks; D0.3 hash freeze.
Do NOT: start D1, change TaskGraph serialiser, or wire Starlark.
Exit: PHASE_2D_INSTRUCTIONS.md D0 checklist all checked.
```

### D1 template

```
Authority: AELIO_MASTER_PLAN.md Session D1 + PHASE_2_ACCEPTANCE_SUITE A3/A4.
Prerequisite: D0 complete.
Task: aarch64 harness-core CI; A4 process-restart hash drill script; fast-math grep.
Exit: pinned hashes match on arm64; restart drill green in CI.
```

### D2 template

```
Authority: AELIO_MASTER_PLAN.md Session D2 + PHASE_2_ACCEPTANCE_SUITE A1.
Prerequisite: D0 complete.
Task: Emit harness_core::exec::Step from wavefront node completion; persist; integration test with corrupted middle step.
Files: orchestration/wavefront.rs, executor.rs, new integration test.
Exit: diverged_at reports correct seq on live graph replay.
```

### Session D5 template

```
Authority: AELIO_EXECUTION_PLAYBOOK.md Part 6 + AELIO_MASTER_PLAN.md Session D5 + PHASE_2_ACCEPTANCE_SUITE Section C.
Prerequisite: D3 effect contracts at least partially done.
Task: Implement C1–C9; each test asserts named gate. C3 MUST assert Gate::Verification — build the verification gate in this session (F-035 removed fake eval; C3 is expected to fail until gate exists).
Red rule: if any C test fails, matching must fail-closed.
Also: B4 (weight/40 reuses skeleton), B5 (between 28-40 must re-author).
Read first: EXECUTION_PLAYBOOK Part 0.2 (debug upstream, not sideways).
```

### E template

```
Authority: F-033, F-034, PHASE_2_INSTRUCTIONS.md Task 7.
Prerequisite: Gates 1–4 green; Gate J verdict recorded in FLAGS.
Task: Starlark per Session E order in AELIO_MASTER_PLAN.md Part 4.
Do NOT: skip taint or budget threading.
```

---

# Part 8 — Verification commands (current baseline)

```bash
# Canonical writer guard
bash scripts/check-canonical-writer.sh

cd aelio-os

# What exists today (Phase 2 A–C)
cargo test -p harness-core                    # 40 tests — D0 raises to 54
cargo test -p harness-core --release
cargo test -p aelio-sol task_graph
cargo test -p aelio-kernel --test journal_underrun --test replay_steps
cargo test -p aelio-agent reuse_metrics suspension orchestrate orchestration

# After D0
cargo test -p harness-core  # expect ≥54

# After D1
# CI aarch64 job green; bash scripts/restart-hash-drill.sh
```

---

# Part 9 — Summary table

| Area | Done? | Next session |
|------|-------|--------------|
| Phase 1 bridge fixes | ✅ Yes | — |
| Phase 2 A canonical collapse | ✅ Yes | — |
| Phase 2 A harness-core | ⚠️ 40/54 tests | **D0** |
| Phase 2 B JournalUnderrun | ✅ Yes | — |
| Phase 2 B GraphSuspension | ⚠️ Wired, not E2E | **D6** |
| Phase 2 B per-step replay | ⚠️ Verifier only | **D2** |
| Phase 2 C reuse counters | ✅ Yes | Gate J review |
| aarch64 / A4 drill | ❌ No | **D1** |
| Effect contracts | ❌ No | **D3** |
| Op catalog / trybuild / time | ❌ No | **D4** |
| Residue C1–C9 | ❌ No | **D5** |
| B4/B5 product loop tests | ❌ No | **D5** |
| Starlark | ❌ No | **E (last)** |
| Full acceptance A–J | ❌ Mostly open | Gates 1–7 |

---

# Part 10 — One-page answer

**Is everything done?** No.

**What is done?** Phase 1 (8 fixes + audit) and Phase 2 Sessions A–C (substrate scaffold, single serialiser, typed replay underrun, suspension wiring, reuse counters).

**What must happen next?** **Session D0** — reconcile harness-core to 54 tests and freeze API shapes. Nothing else should start until D0 exits.

**What does “fully complete” mean?** Gates 1–7 green in [`PHASE_2D_INSTRUCTIONS.md`](PHASE_2D_INSTRUCTIONS.md) Part 2, plus Gate J verdict recorded. That is the full acceptance suite in [`PHASE_2_ACCEPTANCE_SUITE.md`](PHASE_2_ACCEPTANCE_SUITE.md) — determinism, product loop, adversarial matching, effects, taint, budget, sessions, failure injection, time, and reuse measurement.

**Use this file** when planning sprints or pasting context into implementation sessions.

**For step-by-step build instructions, monitoring, and the Part 10 finish-line trace:** [`AELIO_EXECUTION_PLAYBOOK.md`](AELIO_EXECUTION_PLAYBOOK.md) — read its **"READ BEFORE HANDING TO CURSOR"** section first (Part 0.2 blast-radius, Part 5c urgent check, Part 6 C3 prediction).
