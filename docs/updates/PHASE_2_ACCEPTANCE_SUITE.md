# Phase 2 Execution Order + System Acceptance Suite

**Input:** [`PHASE_1_CHANGELOG.md`](PHASE_1_CHANGELOG.md) — F-033/034/035 recorded, 8 fixes landed, five open debts  
**Work order:** [`PHASE_2_INSTRUCTIONS.md`](PHASE_2_INSTRUCTIONS.md)  
**Convergence:** [`BRIDGE_HKV4_CONVERGENCE.md`](BRIDGE_HKV4_CONVERGENCE.md)

Two parts:

1. **Immediate execution order** — Sessions A–C, picking up exactly where the changelog's debts left off.
2. **Acceptance suite** — 60+ tests across ten sections that define what "working perfectly" actually means.

---

# PART 1 — NEXT SESSIONS (A–C)

The changelog earned a short Part 1. Five open debts; three sessions this week.

## Session A — Port + one serialiser (Task 2)

| Step | Action | Exit criterion |
|------|--------|----------------|
| A.1 | Port the 2,111-line `harness-core` implementation over the stubs in `aelio-os/crates/harness-core/` | Source files match deliverables |
| A.2 | Re-pin reference hashes with workspace `blake3` version; record provenance in `tests/determinism.rs` comment | Hashes pinned; blake3 version noted |
| A.3 | **Collapse the dual canonical writer** — rebuild `TaskGraph::canonical_hash()` on `harness_core::canonical` (TaskGraph → Value conversion, hash the Value); **delete** the independent writer in `aelio-sol/src/task_graph.rs` | One serialiser in workspace |
| A.4 | CI grep: `canonical_bytes` / independent canonical writers — exactly one definition path; fail build on second writer | Grep in CI |
| A.5 | `cargo test -p harness-core` debug + release | 54+ green; hash output identical |

**Debt closed:** second canonical writer (KILL-tier, changelog § debt table).

---

## Session B — Close the four named debts

| Step | Debt | Action | Test |
|------|------|--------|------|
| B.1 | Q22 — typed underrun | `ReplayBackend::pop` past-ledger returns **`JournalUnderrun`** (typed divergence), surfaced to replay verifier — not generic `Internal` swallowed by caller | **A7** |
| B.2 | F-035 — GraphSuspension | **Wire** NeedUser mid-graph resume + test, **or delete** types and note in FLAGS | **B7** (full round-trip) |
| B.3 | Q13 — wavefront HashMap | Audit every `HashMap`/`HashSet` in `wavefront.rs`: reaches hash / dispatch order / journal / serialisation → `BTreeMap`; provably not → clippy allow + one-line justification | **A4** (process-restart) |
| B.4 | Q21 — per-step replay | Port `Step` / `verify_replay` from harness-core; divergence reports first diverging `seq` | **A1** |

**Rule:** every spec ambiguity resolved this session → Q103 running list in FLAGS, same session.

---

## Session C — Reuse counters (Task 10, same week, not after)

| Step | Action |
|------|--------|
| C.1 | Raw counters on the bridge: `cold_executions`, `warm_hits`, `distinct_situation_keys`, `executions_per_key` |
| C.2 | Daily log line; **no ratios at emitter** (§14.1) |
| C.3 | Wire into existing warm path (`LookupTier` Tier0/Tier1 hits vs Tier3 cold authorings) |

**Debt closed:** "no reuse measurement" — highest-information task; starts on bridge **now**, not after Starlark.

---

## Sessions D+ — unchanged

Substrate (Tasks 5, 6, 8) → Starlark (Task 7) per [`PHASE_2_INSTRUCTIONS.md`](PHASE_2_INSTRUCTIONS.md).

---

# PART 2 — SYSTEM ACCEPTANCE SUITE

Defines **working perfectly**. Organised by what each section *proves*.  
Format: setup → action → expected → what failure means.  
Implement as integration tests where possible; **[manual]** = operational drill.

**Run while building:** A after every substrate change; B+C when matching lands; D as effect driver grows; E against harness-core; F/G on bridge now; H before external users; I with time module; **J starting this week on the bridge**.

---

## A. Determinism (I1) — the foundation

| ID | Test | Expected | Failure means |
|----|------|----------|---------------|
| A1 | Byte-identical replay | Live execute → replay from journal → every `Step.output_hash` + final hash identical | Nondeterminism in kernel/op/builtin |
| A2 | Cross-profile parity | All hash tests debug vs release → zero diff | FMA / optimisation-sensitive float |
| A3 | Cross-architecture parity | x86_64 + aarch64 CI → pinned hashes match | M1 exit gate open |
| **A4** | **Process-restart stability** | Run full hash suite → **restart process** → run again → diff identical | **HashMap in hash path** — single process is internally consistent; normal runs pass; only restart catches per-process seed |
| A5 | Tool-order independence | Same rows, three arrival orders → three identical `output_hash` | §5.2 ordering missing or too late |
| A6 | Float order sentinel | `[1e16, 1.0, -1e16]` reorder → sum **changes** | Documents why A5 must exist |
| A7 | Journal underrun, never dispatch | Truncate journal by one → typed **`JournalUnderrun`** at exact seq; **mock tool call count = 0 during replay** | Second assertion fails = replays have side effects |
| A8 | Serialiser edge sweep | `-0.0`/`0.0` equal; `0.1+0.2`/`0.3` differ; `["ab","c"]`/`["a","bc"]` differ; `Int(1)`/`Decimal(1.000000)` differ; `Empty`/`Int(0)` differ; two `Empty` reasons differ; nested map two orders equal | Canonical writer bug |

---

## B. The product loop — reuse under proof

Run on bridge now; on HKv4 matching when it lands.

| ID | Test | Expected | Failure means |
|----|------|----------|---------------|
| B1 | Author once | "average height of people over 28" cold → correct answer; skeleton saved; ledger `Authored` | Loop doesn't close |
| B2 | Reuse, same words | Identical request → warm hit; zero authoring; ledger `Cached` | Promotion/lookup broken |
| B3 | Reuse, re-phrase | "mean height for people older than 28" → same skeleton, no author | Intent normalisation too literal |
| **B4** | **Reuse with new bindings** | "average **weight** of people over **40**" → same skeleton as height-over-28, new bindings; **zero authoring** | **Thesis test:** abstraction generalised |
| **B5** | **Structure change forces re-author** | "average height aged **between 28 and 40**" → arity-3 structure; **must NOT** bind gt-skeleton | Over-eager matching = silent wrong answer |
| B6 | Turn idempotency | Same `turn_key` + body → cached; same key different body → loud reject | Retry/cache bug |
| B7 | Suspension round-trip | Missing param → `Orchestrate.NeedUser`, suspended; user answers → completes | GraphSuspension debt (Session B.2) |

---

## C. Matching adversarial set — precision over recall (I3)

Every case: plausible candidate retrieved, then **REJECTED or caught by a named gate**. Assert **which gate**, not just failure.

| ID | Case | Must happen | Gate |
|----|------|-------------|------|
| C1 | + "in Bangalore" (extra constraint) | reject → author 2-filter version | **residue** |
| C2 | Request omits required skeleton filter | reject | **bind** (unfilled hole) |
| **C3** | **"median" vs cached mean** | mean must NOT run | **verification** — same signature; residue can't see it; test must assert verification gate, not "something failed" |
| C4 | `>= 28` vs cached `> 28` | 28 included exactly once | binding correctness |
| C5 | "weight" vs "height" | different binding | bind |
| C6 | "employees" vs "people" source | reject or distinct binding | bind |
| C7 | null-policy variant | different skeleton, not parameter | §6.2 structure |
| C8 | "typical height" (implicit agg) | plan summary before execute | §7.7 implicit-term |
| C9 | High embed score, fails residue | fall through to author; **no** nearest-neighbour bind | §7.6 fail-closed |

> **Scoring:** Section **C red is worse than down** — confidently wrong. Disable matching (cold-path everything) until green.

---

## D. Effect safety — writes cannot double-fire

| ID | Test | Key assertion |
|----|------|---------------|
| D1 | Shadow stubs writes | Live write once; shadow `stubbed: true`; mock counter = 1 |
| D2 | Client retry | Same `turn_key` + body → write counter = 1 |
| D3 | `map_tool` element keys | 5 elements → keys `(K, seq, 0..4)`; 5 writes not 1 deduped (N6) |
| D4 | Writes never auto-retry | Write fails once → error; read retried |
| D5 | `map_tool` all-or-nothing | Element 57 fails → whole error; journal in input-index order |
| D6 | Promotion evidence | Zero live tool calls from evidence run (`promotion.rs`) |
| D7 | Registration | Missing `effect_class` / `completeness` / `returns_entity` → hard refuse |
| D8 | Unstable source | Mid-scan mutation → `UnstableSource` (when pagination lands) |

---

## E. Taint — exfiltration (I-SEC)

| ID | Test |
|----|------|
| E1 | Nine prohibited positions — one test, tainted value each |
| E2 | Laundering chain → reject at idempotency key |
| E3 | Map-key laundering — container + extracted key tainted (N13) |
| E4 | Implicit flow — PC-taint; nested clean predicate does not clear |
| E5 | `fail(reason)` with tainted interpolation → reject |
| E6 | `declassify_count` only sanctioned untaint |

---

## F. Budget and exhaustion

| ID | Test |
|----|------|
| F1 | Shared pool across 10-deep nesting |
| F2 | 8 threads, 1M rows, exact drain (N14 CAS) |
| F3 | Exhaust mid-aggregate → error naming dimension; **no numeric in response** |
| F4 | `model_calls=0` default → first call refused |
| F5 | Take exactly N succeeds; N+1 fails |
| F6 | Audit pool exhaustion does not affect user turn |

---

## G. Concurrency and sessions

| ID | Test |
|----|------|
| G1 | Concurrent same-session turns → serial; `prev_hash` chain intact |
| G2 | Third turn beyond depth 2 → rejected, not queued |
| G3 | Second instance same store → refuse start |
| G4 | 50-turn session → full chain walk valid |

---

## H. Failure injection — 3am drills

| ID | Test | Pass condition |
|----|------|----------------|
| H1 | Tool down mid-workflow | Fail closed; names tool |
| **H2** | **Corrupted effect-set cache** | **(a)** Invalid hash → load refuses. **(b)** Valid hash of **smaller** set than AST references → **`AstExceedsEffectSet`** on AST walk. Both directions loud — (b) is I5, not ordinary integrity |
| H3 | Dialect drift | `enable_recursion=true` → refuse start (post-Starlark) |
| H4 | Control-plane unreachable [manual] | Cached runs continue; author degrades; permissions fail closed |
| H5 | Clock skew | Journaled `now()` backwards → replay + cache bucket from journal |
| **H6** | **Prompt injection in tool data** | `"ignore previous instructions…"` in field → tainted; aggregated normally; **never** reaches prompt/tool name; rendered escaped. **Injection does nothing = pass** |

---

## I. Time edge cases

| ID | Test | Pass condition |
|----|------|----------------|
| **I1** | **Tenant midnight rollover** | Kolkata; cache `LastNDays(30)` at 23:50 local; advance to 00:10 local (still previous day UTC) → **cache MISS**, window recomputed. Off-by-5.5h bug (B3) |
| I2 | Half-open intervals | Sep 30 23:59:59.999 in; Oct 1 00:00:00.000 out |
| I3 | DST fall-back | 25h bucket; ambiguous instant → earlier offset; stable bucket hash |
| I4 | tzdb pinning | Shipped tzdb hash stable; host tzdb never read |

---

## J. Kill criterion — is the product worth existing?

**Not pass/fail.** The measurement that decides everything. **Start this week on the bridge.**

| ID | What | Notes |
|----|------|-------|
| J1 | Counters live | One week: cold, warm, distinct keys, executions-per-key — daily, raw only |
| J2 | The reading | Top-heavy tail → thesis supported. Flat median ≈ 1 → check SituationKey fragmentation before concluding premise wrong |
| J3 | Warm-path spot-check [manual] | Sample 20 warm hits; re-run cold; compare. Disagreement = false hit + gate bug |

> Everything in A–I proves the system **works as designed**. **J alone says whether the design was worth building.**

---

## Scoring the suite

| Section red | Action |
|-------------|--------|
| **A + D + E** | **Stop.** Invariants broken; features on top are decoration. |
| **B** | Loop doesn't loop. Fix before measuring J. |
| **C** | **Worse than down** — confidently wrong. **Disable matching** until green. |
| **F / G / H / I** | Production-readiness gaps; ship-blockers, not architecture-invalidators. |
| **J** | **Verdict**, not red/green. |

---

## Traceability — changelog debts → sessions → tests

| Changelog debt | Session | Acceptance tests |
|----------------|---------|------------------|
| Dual canonical writer | A.3–A.4 | A8, A2, A3 |
| harness-core stubs | A.1–A.5 | A1–A8, E1–E6, F1–F2 |
| Typed JournalUnderrun | B.1 | A7 |
| GraphSuspension unwired | B.2 | B7 |
| Wavefront HashMap | B.3 | A4, A5 |
| Per-step replay | B.4 | A1 |
| No reuse counters | C | J1–J3, B2, B4 |
| Residue one-case only | D+ / Task 8 | C1, C9 |
| Fake eval gate (fixed) | — | C3 gate naming |

---

## Document index

| Doc | Role |
|-----|------|
| [`PHASE_1_CHANGELOG.md`](PHASE_1_CHANGELOG.md) | What we changed |
| [`PHASE_2_INSTRUCTIONS.md`](PHASE_2_INSTRUCTIONS.md) | Full Phase 2 task list |
| [`PHASE_2_ACCEPTANCE_SUITE.md`](PHASE_2_ACCEPTANCE_SUITE.md) | This file — execution order + acceptance |
| [`IMPLEMENTATION_VERIFICATION_REPORT.md`](IMPLEMENTATION_VERIFICATION_REPORT.md) | Audit scorecard |
