# Aelio Execution Playbook — Implement, Test, Monitor, Complete

**Companion to** [`AELIO_MASTER_PLAN.md`](AELIO_MASTER_PLAN.md). The master plan says *what* the sessions are; this document says **how to build each one, step by step**, how every piece feeds the whole, and what signal proves each piece is doing its job once live.

Read Part 0 once before any session. Then work one session at a time; each session chapter is self-contained enough to hand to an agent.

---

# READ BEFORE HANDING SESSIONS TO CURSOR

Three sections in this playbook matter more than the rest. Read them before pasting any session prompt to an agent.

## 1. Part 0.2 — Blast-radius table + reading rule

Every component: **depends on**, **what silently breaks downstream if wrong**, **one health signal** proving it helps.

**The reading rule (memorise this):** when a symptom appears, walk **up the dependency column**, not sideways.

| Symptom you see | Walk up to suspect first | Not sideways into |
|-----------------|--------------------------|-------------------|
| Wrong aggregate answer | Canonical serialiser, float **ordering** | agg.rs logic in isolation |
| Matching miss-storm | SituationKey / intent fragmentation | Embedding model quality |
| Replay diverges but agg looks fine | Step trace assignment, journal order | Wavefront scheduling cosmetic |
| Warm hit but wrong answer | Residue / verification gates | Tool implementation |
| Residue never fires (<5%) | Gate not wired (failure that looks like success) | "Matching is working great" |

Full table: **Part 0.2** below.

## 2. Part 5c — Urgent check (do NOT wait for Session D4)

Before the time module lands, grep the bridge warm path for **cached time-relative windows**:

```bash
grep -rn 'days_ago\|last_.*days\|LastNDays\|since\|window' aelio-os/crates/aelio-agent/src/ | grep -iE 'cache|promot|warm|lookup|situation'
```

If the bridge stores resolved date ranges anywhere in σ or the warm store, it is serving **stale windows today**. Hotfix: add **validity bucket into σ** before the rest of the time module.

**Status (2026-08-07 grep):** No `LastNDays` / calendar-window cache found on the warm lookup path. σ includes `last_seen_bucket` (visit recency, not calendar time) — document in FLAGS; re-run this grep before D4 and before enabling matching on real traffic.

## 3. Part 6 (D5) — C3 prediction: verification gate must be built in D5

**C3** ("median" vs cached mean skeleton) must be caught by the **verification** gate — residue cannot see it (same signature, same binding holes).

F-035 removed the fake semantic eval (`pass: true`); **nothing replaced it**. Therefore **C3 is currently unpassable** — and that is the point of writing the test. D5 must implement a narrow verification gate (discriminating slots: requested agg term vs bound op; §7.7 implicit-term list) **in that session**, not deferred.

If an agent reports "C3 failed because no gate exists" — that is success; build the gate.

---

**Also embedded in later parts (do not skip when implementing):**

| Design decision | Where | Rule |
|-----------------|-------|------|
| Step `seq` = planned topological index | Part 3 (D2) | Not completion order — do it now while Map is sequential |
| Starlark value type = tainted wrapper day one | Part 8 (E) | No "raw values temporarily" — the seam becomes permanent |
| Monitoring incrementally | Part 9 | One panel per session; residue rejections healthy band **5–20%** (<5% = gate not firing) |
| Finish line artifact | Part 10 | Four-request trace — when printable, the product exists |

---

# PART 0 — THE SYSTEM AS ONE MACHINE

## 0.1 The single flow every piece serves

Every component exists to make this flow correct, cheap, or provable:

```
 utterance
    │
    ▼
 [TRIAGE] ──Reply──► render (no effects, no numbers about tenant data)
    │Compute
    ▼
 [INTENT] constraint set ──round-trip check──► Clarify if divergent
    │
    ▼
 [MATCH]  Tier0 exact ─► Tier1 embed ─► BIND+GATES (residue·fanout·units) ─► VERIFY
    │hit                                                            │no candidate
    ▼                                                               ▼
 [EXECUTE]◄──────────────────────────────────────────────[AUTHOR] text→AST→checks→dry
    │  authorise(Verified<EffectSet>) BEFORE first instruction
    │  budget pool · taint · journal · step trace
    ▼
 [RECORD] trace + ledger entry + reuse counter
    │
    ▼
 [ABSTRACT] normalise → skeleton → signature → promote ladder
    │
    ▼
 next same-shape request ──► Tier0/1 hit  (the product)
```

## 0.2 Dependency and blast-radius table

For each component: what it needs, what silently breaks downstream if it is wrong, and the **one health signal** that tells you it is helping the whole.

| Component | Depends on | If wrong, what breaks downstream (silently) | Health signal |
|---|---|---|---|
| Canonical serialiser | nothing | every hash in the system → matching, promotion, replay, ledger all keyed on garbage | pinned-hash CI on 2 arches × 2 profiles stays green |
| Numeric tower | serialiser | every aggregate answer; money rounds; Empty becomes 0 | agg golden cases + D0 54-test suite |
| Taint | value repr | data exfiltrates via emit/fail/keys; injections steer tools | E1–E6 green; `taint_rejections_total` > 0 in prod (zero means it's not firing) |
| Budget pool | nothing | runaway cost; partial aggregates served as truth | `budget_exhaustions_total` by dimension; F2 exactness test |
| Step trace + journal | serialiser | replay can't localise; audits become archaeology | nightly `replay_divergences_total == 0` |
| Effect contracts | registry | paginated tools feed truncated aggregates; shadow fires writes | `registration_rejections_total`; D1 shadow-write test |
| SituationKey / intent | triage | reuse fragments (every key unique) or over-merges (wrong binds) | `distinct_keys / total` trend + C-suite green |
| Bind + residue gates | intent, skeleton store | **confident wrong answers** — the one failure worse than downtime | `residue_rejections_total` in **5–20%** band; **<5% = gate not firing (failure that looks like success)**; C1–C9 green |
| Verification (Phase 3) | render fidelity; **must exist for C3** | mean served for median; C3 unpassable until built in D5 | audit disagreement < 0.5%; C3 asserts `Gate::Verification` |
| Promotion ladder | trace, effect contracts | bad skeletons reach warm path; writes promote unreviewed | per-stage counts; zero write-workflows outside Promoted |
| Ledger | serialiser, session lock | audit story dead; chain forks under concurrency | full-chain walk in G4; `chain_breaks_total == 0` |
| Reuse counters | turn spine | you never learn whether the product works | executions-per-key distribution reviewed weekly |
| Starlark runtime | ALL of the above | — (this is why it is last) | Gate 7 |

**The reading rule for this table:** when any downstream symptom appears, walk **UP the dependency column**, not sideways. A wrong aggregate is more likely a serialiser/ordering bug than an agg bug; a matching miss-storm is more likely intent fragmentation than embedding quality. **This rule saves days of debugging in the wrong module.**

## 0.3 The three numbers that summarise system health

If you watch nothing else, watch these daily:

1. **`replay_divergences_total` = 0.** Any non-zero → I1 broken → every other number is unreliable. Page-level.
2. **Audit disagreement rate < 0.5%** (once D5+ audit live; until then, J3 manual spot-checks). Rising → matching gates leaking → disable matching per red rule.
3. **Executions-per-key distribution** (weekly). Top-heavy → product working. Flattening → investigate key fragmentation before anything else.

---

# PART 1 — SESSION D0: RECONCILE HARNESS-CORE (blocker)

**Goal:** 40 → ≥54 tests; contested behaviours locked; API shapes proven; hashes frozen.

### Step-by-step

**1. Generate the gap list mechanically.**
```bash
cd aelio-os/crates/harness-core
for t in no_implicit_coercion int_division_yields_decimal_not_float \
  decimal_addition_is_exact_where_float_is_not bankers_rounding_ties_to_even \
  division_by_zero_is_an_error nan_is_rejected_at_the_op_boundary \
  decimal_scale_alignment_preserves_value decimal_ordering_across_scales \
  negative_zero_hashes_as_positive_zero map_hash_is_independent_of_insertion_order \
  float_precision_survives_the_round_trip empty_and_zero_hash_differently \
  distinct_empty_reasons_hash_differently int_and_decimal_with_equal_value_hash_differently \
  length_prefixing_prevents_concatenation_collisions nested_structure_is_stable \
  count_rows_and_count_values_differ_on_nulls count_of_empty_is_zero_not_empty \
  the_adversarial_null_case_from_the_golden_set mean_of_all_nulls_is_empty_not_zero \
  mean_over_int_returns_decimal_not_float sum_over_int_stays_exact_at_scale \
  pairwise_sum_is_deterministic float_sum_depends_on_order pairwise_beats_naive \
  mixed_numeric_types_in_one_column_are_rejected wrapping_in_a_list_does_not_launder \
  nesting_two_deep_does_not_launder map_keys_cannot_launder_taint \
  group_by_shaped_construction_still_works indexing_out_of_a_tainted_container \
  a_clean_element_of_a_tainted_container comparison_result_carries_taint \
  length_carries_taint_until_explicitly_declassified every_prohibited_position \
  implicit_flow_through_branch_selection pc_taint_is_inherited_by_nested_branches \
  nested_frames_share_one_pool exhaustion_names_the_dimension_and_frame \
  model_calls_default_to_zero concurrent_takes_cannot_overshoot \
  identical_traces_verify a_diverged_step_is_located_by_sequence \
  replay_reads_the_journal_and_never_dispatches \
  a_replay_taking_a_different_path_underruns stubbed_writes_are_marked \
  every_version_field_affects_the_hash op_catalog_hash_is_order_independent \
  ast_exceeding_the_verified_set_is_rejected; do
  grep -rq "$t" src/ tests/ || echo "MISSING: $t"
done
```
Every MISSING line is a work item. Implement the test FIRST, watch it fail (or reveal the behaviour is absent), then fix the implementation.

**2. The five contested behaviours — implement to these exact answers, they are decisions not preferences:**
- `[1, null, 3]` under three policies: skip→`Decimal 2.000000`, strict→distinct `NullEncountered` refusal (NOT generic Err — the renderer needs to say "column has nulls, you asked strict"), zero→`Decimal 1.333333`.
- Banker's on negatives: `-1/2→0`, `-3/2→-2`. Round on the magnitude, apply sign after.
- `Int/Int` division → `Decimal{6}`. `Decimal{s}/Decimal` → `Decimal{max(s,6)}`. One rule, one function, no per-call-site scales.
- `Decimal{s+6} > MAX_SCALE(28)` → clamp to 28 via `.min(MAX_SCALE)` and note it in the op doc.
- Budget drain: CAS loop. `compare_exchange_weak` retry, never `fetch_sub`.

**3. API shape proofs (do these even if tests pass):**
```bash
# Verified<T> privacy — must print nothing:
grep -n "pub fn new\|impl From" src/versioning.rs | grep -i verified
# SystemVersion exhaustive destructure — must show `let Self {`:
grep -n "let Self {" src/versioning.rs
# then: add `pub dummy: Hash` to the struct, run cargo check — MUST fail at hash(); remove.
```

**4. Freeze hashes.** If any fix changed serialisation, re-pin ONCE, add to both fixture comments:
`// Frozen 2026-08-XX blake3=<ver>. Change = SystemVersion.serialiser bump + migration. NEVER a quiet edit.`
FLAGS entry F-036 closed with the final values.

**Monitoring hook added this session:** none (pure substrate). Health = the CI matrix itself.

**Done when:** gap script prints nothing; ≥54 green debug+release identical; the dummy-field check fails compilation; FLAGS updated.

---

# PART 2 — SESSION D1: DETERMINISM CI + DRILLS

**Goal:** the two tests that can't pass by accident — cross-arch and cross-process.

### Steps

**1. aarch64 job.** In `.github/workflows/ci.yml`, extend the matrix:
```yaml
strategy:
  matrix:
    include:
      - { os: ubuntu-latest,      target: x86_64 }
      - { os: ubuntu-24.04-arm,   target: aarch64 }
    profile: [dev, release]
steps:
  - run: cargo test -p harness-core --profile ${{ matrix.profile }}
  - run: cargo test -p aelio-sol --profile ${{ matrix.profile }}
```
If aarch64 diverges: suspect FMA contraction in pairwise sum → add `RUSTFLAGS="-C target-feature=-fma"` for the aggregate crate OR replace any `mul_add` — then re-run. Do NOT edit the fixture to match ARM.

**2. Restart drill.** `scripts/restart-hash-drill.sh`:
```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../aelio-os"
run() { cargo test -p harness-core --test determinism -- --nocapture 2>/dev/null | grep -E '= [0-9a-f]{64}' | sort; }
A=$(run); B=$(run)   # separate cargo invocations = separate processes
diff <(echo "$A") <(echo "$B") && echo "RESTART DRILL: PASS"
```
Wire into CI. This is the ONLY automated catch for a per-process-seeded HashMap in a hash path.

**3. Hygiene greps in CI** (workspace-wide): fast-math flag scan; `cargo tree -e features | grep preserve_order` must be empty; clippy `disallowed_types` extended beyond aelio-sol to every crate that touches hashing or effects.

**Done when:** both arches green with identical pinned hashes; drill green; hygiene steps in CI.

---

# PART 3 — SESSION D2: LIVE STEP TRACES

**Goal:** the kernel/agent EMITS what the verifier verifies.

### Steps

**1. Emit at node completion.** In `orchestration/wavefront.rs`, where a node's output lands in `ExecutorState.outputs`:
```rust
// step seq = completion order under the session lock; inputs_hash over the
// node's canonicalised args (already available from hash_args), output over
// the canonical value of the node result.
trace.steps.push(harness_core::exec::Step {
    seq: trace.steps.len() as u32,
    op: node.op_name.clone(),
    inputs_hash: args_hash,          // reuse the idempotency-path hash
    output_hash: aelio_sol::value_hash(&value_to_sol(&output)),
});
```
One subtlety: with parallel wavefronts later, **seq must be assigned by input topological index, not completion order** — assign step slots when the wavefront is planned, fill them on completion. Do it that way NOW while Map is sequential, so parallelism doesn't change trace shape.

**2. Accumulate + persist.** `executor.rs` owns a turn-local `Trace`; on turn end it is persisted next to the orchestration ledger record (same durable write — a trace without its ledger entry, or vice versa, is an audit hole).

**3. Integration test** `tests/orchestration_replay_steps.rs`: build a 3-node graph (fetch→filter→agg with a mock tool), run Live, capture trace; run Replay from the journal, `verify_replay` → Ok; corrupt `steps[1].output_hash`, verify → `diverged_at == 1`.

**4. Monitoring hook:** counter `replay_divergences_total` + a `just replay-sample` task that replays the last N traces. This becomes the nightly job later; land the counter now.

**Done when:** live graphs produce traces; corruption localises; counter exists.

---

# PART 4 — SESSION D3: EFFECT CONTRACTS

**Goal:** the registry refuses ignorance; promotion respects completeness; keys are per-element.

### Steps

**1. Contract struct + refusing registration** in `abilities/registry.rs`:
```rust
pub struct ToolContract {
    pub effect_class: EffectClass,        // Read | Write | IdempotentWrite  — REQUIRED
    pub completeness: Completeness,       // Complete | Paginated{cursor_key, max} | Sampled | Unknown — REQUIRED
    pub returns_entity: EntityId,         // REQUIRED
    pub pushdown: Vec<String>,
    pub max_result_rows: Option<u64>,
    pub row_scoped: bool,
}
pub fn register(spec: ToolSpec) -> Result<(), RegError> {
    let c = spec.contract.ok_or(RegError::MissingContract)?;
    // no defaults, no inference — absence is refusal
    ...
}
```
Backfill every existing demo/catalog tool with honest values — `Unknown` is an honest value; it just blocks aggregates.

**2. Promotion gate** in `orchestration/promotion.rs`: walk the graph's tools; if any aggregate op is downstream of a tool whose completeness is `Unknown|Sampled` → promotion refused with the tool named. Test: register a tool as Unknown, build filter→mean graph over it, assert refusal message names the tool.

**3. Key composition.** Wherever idempotency keys are built (wavefront `hash_args` consumer): `blake3(turn_key ‖ effect_seq ‖ element_index?)`. Test D3: mock idempotent-write tool that dedupes on key; map 5 elements; assert **5** writes recorded.

**4. Monitoring hooks:** `registration_rejections_total`, `promotion_blocked_completeness_total`. Both should be >0 early (people forget fields) and trend to 0 — a permanent 0 from day one means the gate isn't wired.

**Done when:** incomplete registration hard-fails (D7 test); Unknown blocks aggregate promotion; 5-element key test green.

---

# PART 5 — SESSION D4: CATALOG + TRYBUILD + TIME

### 5a. Op catalog
Each harness-core op gets `impl_hash = blake3(canonical fn body bytes or a manually bumped impl version string)`; pragmatically: a `const IMPL_REV: &str` per op, hash that, and enforce by convention + review that any behavioural change bumps it. Merkle root over `BTreeMap<op_name, impl_hash>` → `SystemVersion.op_catalog`. For each `std.*` bridge tool write one line in a `CATALOG.md`: `catalog-op | bridge-only(retire-when: …)`.

### 5b. Trybuild — the structural closures
```toml
[dev-dependencies] trybuild = "1"
```
`tests/compile_fail.rs`:
```rust
#[test] fn ui() { let t = trybuild::TestCases::new(); t.compile_fail("tests/ui/*.rs"); }
```
`tests/ui/unverified_effect_set.rs` — construct an `EffectSet` directly, pass to `authorize` → expected error: mismatched types (needs `&Verified<EffectSet>`). `tests/ui/raw_verified_construction.rs` — `Verified(...)` or `Verified::new(...)` → must not compile. Check in the `.stderr` snapshots.

### 5c. Time module (the big one)
1. `time.rs`: `now_ms()` journaled only; `IanaTz` newtype; **tzdb via `chrono-tz` pinned exact version**, that version string into `SystemVersion.tzdb`. Never `TZ` env, never `/usr/share/zoneinfo`.
2. `TemporalBinding { expression, granularity, tz, resolved: [start,end), resolved_at }` — resolution from journaled now, recorded in the binding.
3. `calendar_bucket(now_utc, granularity, tz)` — calendar-aware: Day flips at tenant midnight, Month on the 1st tenant-local, Quarter on quarter starts. NOT floor-division above Hour.
4. Tests I1–I4 from the acceptance suite, PLUS the DST fall-back bucket (Europe/Berlin, repeated hour → 25h bucket, earlier offset, stable hash).
5. **The urgent check first:** `grep -rn "days_ago\|last_.*days\|since\|window" aelio-agent/src/ | grep -i cache` — if the bridge warm path stores any resolved time window, that cache is serving stale answers TODAY; hotfix by adding the validity bucket to σ before the rest of the module even lands.

**Done when:** Merkle root feeds SystemVersion; both compile-fail tests fail correctly; I1–I4 + DST green; the urgent check answered in FLAGS.

---

# PART 6 — SESSION D5: THE MATCHING GATES (C1–C9 + B4/B5)

**This is the session that makes the product safe to turn on.** Red rule in force: any C failure → matching disabled (all traffic cold) until green.

**C3 prediction (read before implementing):** F-035 removed fake semantic eval; no verification gate exists yet. C3 is **designed to fail first** — implementing the narrow verification gate (discriminating slots: requested agg vs bound op) is **in scope for D5**, not a follow-up.

### Steps

**1. Build the harness for the tests before the tests** — a fixture tenant: entities Person{age,height,weight,city}, Employee{…}; tools `get_people` (Complete), `get_employees`; two cached skeletons (single-filter mean-agg; group-agg) planted in the warm store with known σ.

**2. Constraint decomposition on the bridge.** If σ today is `(state, intent-class, slots, caps)`, the residue check needs slots to be a **complete constraint list**, not a lossy summary. Add: after slot extraction, every clause qualifier in the utterance must map to a slot or be flagged unconsumed. This is where the Bangalore drop happens if it happens.

**3. The nine tests** in `tests/residue_adversarial.rs` — each ends with an assertion on the **gate name** in the rejection/decision record:
```rust
assert_eq!(decision.outcome, Outcome::Rejected);
assert_eq!(decision.gate, Gate::Residue);          // C1
// C3 is the special one:
assert_eq!(decision.gate, Gate::Verification);      // residue CANNOT see mean-vs-median
```
If C3 has no gate to name — because the fake semantic eval was removed in F-035 and nothing replaced it — then **C3 is currently unpassable and that is the finding**: the verification gate must be implemented in this session (even a narrow one: for discriminating slots, a focused check comparing requested aggregation term against bound op, with the implicit-term list from §7.7).

**4. B4/B5 loop tests** in the same fixture: author height>28 → request weight>40 (must warm-hit with new bindings, zero authoring — assert via cold counter unchanged) → request between-28-and-40 (must cold-author — assert cold counter +1).

**5. Monitoring hooks:** `residue_rejections_total`, `gate_decisions_total{gate,outcome}`, and the red-rule breaker: config flag `matching_enabled` that the C-suite CI job flips off on failure (in CI this is just: C-tests are required for the deploy job).

**Done when:** nine C-tests + B4 + B5 green, each asserting its gate; the C3 verification gate exists for real; red rule wired into deploy.

---

# PART 7 — SESSION D6: SUSPENSION END-TO-END

1. **Happy path test:** tool `book_slot(date REQUIRED)`; utterance omits date → assert `Orchestrate.NeedUser`, suspension persisted in `World.user_graph_suspensions`, question mentions the missing param by name. Second turn "tomorrow at 3" → `try_resume_orchestrated_graph` merges, graph completes, tool called ONCE with the merged arg. Ledger: both turns present, resume turn references the suspended turn's id.
2. **Negative:** answer "whenever lol" (unparseable to the slot type) → re-Clarify with a narrower question; suspension retained; retry counter on suspension increments; after 3 unusable answers → abandon gracefully (`Clarify` exhausted → plain cold response), never a guess, never a crash.
3. **Expiry:** suspension older than TTL (config, default 24h) → next turn treats as fresh, does not resume a stale graph against changed context.
4. **Monitoring:** `suspensions_opened/resumed/expired/abandoned`. Healthy ratio: resumed/opened > 0.7; a low ratio means the clarifying questions are bad.

---

# PART 8 — SESSION E: STARLARK (only after Gates 1–4 + J verdict)

Strict internal order — each step has a test before the next starts:

1. **Toolchain + pin.** Workspace toolchain file to an edition2024-capable Rust; `starlark = "=<ver>"` exact-pinned; version string → `SystemVersion.dialect` input.
2. **Dialect lockdown.** Build `Dialect { enable_recursion:false, enable_sets:false, enable_top_level_stmt:false, .. }`; hash the config struct; assert at startup against SystemVersion; test H3: flip recursion in a test config → startup returns Err, process refuses.
3. **Value type = tainted wrapper from the first line.** Implement starlark's value traits for `TV` (or a wrapper enum bridging `TV` ↔ starlark heap values with taint carried in the wrapper). Do NOT wire raw values "temporarily" — the seam becomes permanent.
4. **Budget threading.** Evaluator step counter → `Budget.take_steps` per N instructions via the eval callback; heap: set starlark's heap limit AND write the test that allocates a 100MB list — record whether it enforces (error) or approximates (OOM/overrun). Either answer goes in FLAGS; if approximate, `result_rows` + steps are the real bounds and the doc says so.
5. **Effect bridge.** `spawn_blocking` around eval; builtins send `EffectRequest{resp: oneshot}` over mpsc to the async driver (principal check → taint check → contract → dispatch/stub → journal → respond). Test: builtin tool call round-trips; in Replay mode the driver serves from journal and the mock tool records zero calls.
6. **Cycle check.** Kernel-side stack of harness hashes in the eval context; `call_harness` to a hash on the stack → PermissionDenied("cycle"); test A→B→A.
7. **F-034 codegen path.** `author(utterance) → llm_emit_text → parse (starlark AST) → normalise → canonical_render → blake3 = harness_hash → static checks (symbols resolve, effect set ⊆ grants, no call_tool-in-for warn) → Dry run → cache`. Round-trip property test: for every authored program, `parse(render(parse(text)))` AST-equal; two whitespace variants of the same program → ONE hash (test with a hand-made variant pair).
8. **Gate 7 finale:** one real LLM-authored harness executes; the identical request re-arrives; assert cache hit, zero authoring, ledger `path: Cached`.

**Monitoring hooks:** `authoring_attempts/parse_failures/repair_iterations` (repair capped at 3 — the counter proves the cap), `starlark_step_depletion_p99`, `codegen_cache_hits`.

---

# PART 9 — THE MONITORING LAYER (assemble as you go, not at the end)

Every session above added counters. This is the dashboard they add up to — build it incrementally, one panel per session:

| Panel | Metrics | Healthy | Anomaly → first place to look |
|---|---|---|---|
| **Invariants** | replay_divergences, chain_breaks, restart-drill status | all zero/green | HashMap in a new hash path; float op added without ordering |
| **Reuse (the product)** | warm_hits, cold_executions, distinct_keys, executions-per-key histogram | warm share rising; top-20 keys >50% | flat distribution → σ fragmentation (check intent normalisation) before doubting thesis |
| **Matching precision** | gate_decisions{gate,outcome}, residue_rejections %, audit disagreement | residue **5–20%**; disagreement <0.5% | **residue <5% → gate not firing (looks like success, isn't)**; >20% → holes need variadic slots |
| **Effects** | writes_total, stubbed_writes, dedup_hits, registration_rejections | zero double-fires; dedups >0 under retries | dedup 0 with retries present → key composition broke |
| **Taint** | taint_rejections{position} | >0, low, stable | 0 forever → not wired; spike in one position → new authored pattern probing |
| **Budget** | exhaustions{dimension}, p99 consumption per dimension | rare, named | result_rows spikes → pushdown missing on a tool |
| **Suspension** | opened/resumed/expired/abandoned | resumed/opened >0.7 | low → clarifying questions unclear |
| **Authoring** | parse_failures %, repair_iterations avg, cost/authored | failures <3%, repairs <1.5 avg | rising repairs → catalog docs unclear to the model |

**The weekly ritual (30 min, non-negotiable):** pull executions-per-key, eyeball the histogram, run J3 (10–20 warm hits re-run cold, diff answers), file every disagreement as golden case + gate bug, one line in FLAGS. This ritual IS the product feedback loop; skipping it means flying the thesis blind.

---

# PART 10 — ONE REQUEST THROUGH THE FINISHED SYSTEM (the acceptance walkthrough)

When Gates 1–7 are green, this exact trace must be producible on demand — it is **the demo, the debug tool, and the sales artifact in one**. When the system can print this trace, **the product exists**.

```
> "average height of people over 28 in bangalore"        [first time]
  triage=Compute · intent={filters:[age>28, city=bangalore], agg:mean, target:height}
  match: Tier1 candidate σ=7f3a… (single-filter mean) → BIND → RESIDUE REJECT (city unconsumed)
  → AUTHOR: text→AST→hash e2b1… · effect_set={get_people} ⊆ grants ✓ · dry ✓
  → EXECUTE: 1 tool call (pushdown age.gt, city.eq) · 4812 rows · 193 null height (skipped)
  → answer: 172.4 cm  ["averaged 4,619 of 4,812 records; 193 had no height"]
  → ledger seq=41 path=Authored sig=e2b1… · skeleton saved (2-filter) status=Draft
  → counters: cold+1

> "average height of people over 28 in bangalore"        [again]
  Tier0 exact hit e2b1… → execute → ledger path=Cached · counters: warm+1 · ZERO llm calls

> "mean weight of people above 40 in chennai"            [the thesis]
  Tier1 hit e2b1… → BIND {field:weight, threshold:40, city:chennai} → residue ✓ all consumed
  → VERIFY: "computes MEAN (alts: median,sum,min,max) — confirm" ✓
  → execute · ledger path=Matched · counters: warm+1 · ZERO authoring

> "median height of people over 28"
  Tier1 hit e2b1… → bind ✓ residue ✓ → VERIFY catches: bound op=mean, requested=median → REJECT
  → author median variant (same skeleton family, different discriminating binding)

replay 41 → byte-identical, zero tool dispatches, divergence report: none
```

Four requests, and they exercise: residue rejection, authoring, exact reuse, generalised reuse, the verification gate catching the one thing residue can't see, and replay. If the system can print this trace, the product exists. If any line can't happen, the gap analysis in Parts 1–8 says which session owes it.

---

## Document map

```
docs/updates/
├── AELIO_EXECUTION_PLAYBOOK.md   ← THIS FILE (how to build + monitor + finish line)
├── AELIO_MASTER_PLAN.md          ← what's done / remaining / session order
├── PHASE_2D_INSTRUCTIONS.md        ← D0 test list + gates
└── PHASE_2_ACCEPTANCE_SUITE.md   ← test IDs A–J
```
