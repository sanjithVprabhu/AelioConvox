# Sessions D+ — Work Order and Completion Gates

**Input:** [`AELIO_IMPLEMENTATION_FINAL.md`](AELIO_IMPLEMENTATION_FINAL.md) (Phase 1 + Sessions A–C complete)  
**Authority:** HKv4 · [`FLAGS.md`](../../FLAGS.md) F-033 / F-034 / F-035

This document: (1) **Session D0** — mandatory reconciliation before any other D+ work; (2) Sessions D1–D6 in order; (3) **Part 2** — seven ordered completion gates plus Gate J (the reuse verdict).

---

## Why D0 exists

Phase 2 Session A reported: *"No external bundle was found on disk; implementation was written in-repo against the spec."*

That means `harness-core` was **reimplemented from the spec, not ported**. The in-repo crate has **40 tests**; the original deliverable had **54**. Fourteen missing tests means fourteen behaviours that are untested or unimplemented — and several originals exist precisely because the spec alone would not produce them (they encode the **seven build findings**: division scale, MAX_SCALE clamp, NullEncountered, CAS vs fetch_sub, JournalUnderrun, map-key taint, implicit flow).

A from-spec rewrite will have re-made some of the same ambiguous calls without knowing they were contested. **Session D0 comes before everything else.**

---

# PART 0 — SESSION D0: RECONCILE THE REWRITE

### D0.0 — Inventory baseline (run now)

```bash
cd aelio-os
cargo test -p harness-core -- --list 2>/dev/null | grep ': test ' | wc -l   # expect 40 today
grep -Rh '#\[test\]' crates/harness-core/tests crates/harness-core/src | wc -l
```

**Target exit:** ≥54 tests green; debug == release hash output identical.

---

### D0.1 — Test-by-test reconciliation (grep each name)

For each required test below: **grep the repo** for the exact name or an documented equivalent. Mark `PRESENT` / `MISSING` / `PARTIAL` in FLAGS Q103 when behaviour differs.

**How to grep:**

```bash
cd aelio-os/crates/harness-core
for name in no_implicit_coercion bankers_rounding_ties_to_even float_sum_depends_on_order \
  concurrent_takes_cannot_overshoot pc_taint_is_inherited_by_nested_branches; do
  echo -n "$name: "; grep -rl "$name" . || echo MISSING
done
```

#### numeric (8 required)

| Required test | In-repo today (2026-08-07) | Notes |
|---------------|----------------------------|-------|
| `no_implicit_coercion` | PARTIAL → `add_int_float_is_type_error` | Rename or extend |
| `int_division_yields_decimal_not_float` | PRESENT → `int_div_returns_decimal_not_int` | |
| `decimal_addition_is_exact_where_float_is_not` | **MISSING** | |
| `bankers_rounding_ties_to_even` | PARTIAL → `decimal_bankers_rounding_half_to_even` | **Must cover negative ties** — where half-even breaks |
| `division_by_zero_is_an_error_not_a_panic_or_infinity` | PARTIAL → `div_by_zero_is_err` | All three types: Int, Float, Decimal |
| `nan_is_rejected_at_the_op_boundary` | PARTIAL → `float_rejects_non_finite` | Inf **and** −Inf at op boundary |
| `decimal_scale_alignment_preserves_value` | PRESENT → `decimal_add_aligns_scales` | |
| `decimal_ordering_across_scales` | **MISSING** | e.g. 1.00 == 1 across scales |

#### canonical (8 required)

| Required test | In-repo today | Notes |
|---------------|---------------|-------|
| `negative_zero_hashes_as_positive_zero` | PARTIAL → `serialiser_edge_sweep_a8`, `float_normalizes_negative_zero` | |
| `map_hash_is_independent_of_insertion_order` | PARTIAL | In `determinism` / aelio-sol — confirm harness-core coverage |
| `float_precision_survives_the_round_trip` | PARTIAL → `serialiser_edge_sweep_a8` | 0.1+0.2 ≠ 0.3 |
| `empty_and_zero_hash_differently` | PARTIAL | |
| `distinct_empty_reasons_hash_differently` | **MISSING** | |
| `int_and_decimal_with_equal_value_hash_differently` | **MISSING** | |
| `length_prefixing_prevents_concatenation_collisions` | **MISSING** | |
| `nested_structure_is_stable` | PARTIAL | |

#### agg (10 required)

| Required test | In-repo today | Notes |
|---------------|---------------|-------|
| `count_rows_and_count_values_differ_on_nulls` | PRESENT → `count_rows_and_values_differ` | |
| `count_of_empty_is_zero_not_empty` | PRESENT → `count_empty_returns_zero` | |
| `the_adversarial_null_case_from_the_golden_set` | **MISSING** | All **three** null policies on `[1,null,3]` |
| `mean_of_all_nulls_is_empty_not_zero` | PRESENT → `mean_all_null_is_empty_not_zero` | |
| `mean_over_int_returns_decimal_not_float` | **MISSING** | |
| `sum_over_int_stays_exact_at_scale` | PARTIAL → `sum_int_is_exact` | Past 2^53 |
| `pairwise_sum_is_deterministic_for_a_fixed_input_order` | PRESENT → `sum_float_is_pairwise_by_index` | Bit-compare |
| **`float_sum_depends_on_order_which_is_why_ordering_is_mandated`** | PARTIAL → `float_sum_order_sentinel` (unit in `agg.rs`) | Must assert **harness agg** reorder **changes** sum — documents why A5/A6 exist |
| `pairwise_beats_naive_accumulation_on_error` | **MISSING** | |
| `mixed_numeric_types_in_one_column_are_rejected` | **MISSING** | |

#### taint (11 required)

| Required test | In-repo today | Notes |
|---------------|---------------|-------|
| `wrapping_in_a_list_does_not_launder` | **MISSING** | |
| `nesting_two_deep_does_not_launder` | **MISSING** | |
| **`map_keys_cannot_launder_taint`** | PRESENT → `map_keys_cannot_launder_taint` | Construction **and** key-extraction |
| `group_by_shaped_construction_still_works` | PRESENT | |
| `indexing_out_of_a_tainted_container_stays_tainted` | **MISSING** | |
| `a_clean_element_of_a_tainted_container_is_conservatively_tainted` | **MISSING** | |
| `comparison_result_carries_taint` | **MISSING** | |
| `length_carries_taint_until_explicitly_declassified` | **MISSING** | |
| **`every_prohibited_position_rejects_tainted_values`** | PRESENT → `all_nine_prohibited_positions_reject_tainted_values` | One loop, all nine |
| `implicit_flow_through_branch_selection_is_caught` | **MISSING** | |
| **`pc_taint_is_inherited_by_nested_branches`** | PRESENT → `pc_taint_inherits_and_does_not_clear_on_clean_branch` | Clean inner predicate must **not** clear |

#### exec (9 required)

| Required test | In-repo today | Notes |
|---------------|---------------|-------|
| `nested_frames_share_one_pool` | **MISSING** | |
| `exhaustion_names_the_dimension_and_frame` | PARTIAL → `budget_pool_refuses_overshoot` | Must name dimension + frame; no numeric leak |
| `model_calls_default_to_zero` | **MISSING** | |
| **`concurrent_takes_cannot_overshoot`** | PARTIAL → `budget_pool_cas_drain_is_exact` | 8 threads; drained total **exactly** equals limit; fails intermittently with fetch_sub |
| `identical_traces_verify` | PRESENT → `verify_replay_matches_identical_traces` | |
| `a_diverged_step_is_located_by_sequence` | PRESENT → `verify_replay_reports_first_diverging_seq` | |
| `replay_reads_the_journal_and_never_dispatches` | **KERNEL** → `journal_underrun.rs` | Not harness-core — keep both |
| `a_replay_taking_a_different_path_underruns_rather_than_dispatching` | **MISSING** | |
| `stubbed_writes_are_marked_in_the_journal` | **MISSING** | |

#### versioning (3 required)

| Required test | In-repo today | Notes |
|---------------|---------------|-------|
| `every_version_field_affects_the_hash` | PARTIAL → `system_version_hash_is_exhaustive` | Macro over **all** fields |
| `op_catalog_hash_is_order_independent_of_insertion` | PARTIAL → `system_version_from_ops_builds_catalog_merkle` | |
| `ast_exceeding_the_verified_set_is_rejected` | **MISSING** | Distinct from `verified_load_rejects_hash_mismatch` |

#### determinism fixtures (5 required)

| Required test | In-repo today | Notes |
|---------------|---------------|-------|
| pinned `float_sum_reference` | PRESENT → `float_sum_reference_hash_pinned` | |
| pinned `canonical_form_reference` | PRESENT → `canonical_form_reference_hash` | |
| `repeated_evaluation_is_bit_identical` | **MISSING** | 100× |
| `map_hashing_is_stable_across_construction_paths` | **MISSING** | 500 keys forward vs reverse |
| mean reference | **MISSING** | |

**High-priority gaps** (spec-reader would not write these):

1. `float_sum_depends_on_order_which_is_why_ordering_is_mandated` — agg-level sentinel, not only naive f64
2. `concurrent_takes_cannot_overshoot` — proves CAS, not fetch_sub
3. `bankers_rounding_ties_to_even` — **negative** ties
4. `pc_taint_is_inherited_by_nested_branches` — present but verify depth
5. `the_adversarial_null_case_from_the_golden_set` — three policies

---

### D0.2 — API-shape spot checks

Things a from-spec rewrite plausibly got differently:

| Check | How to verify |
|-------|----------------|
| `Verified<T>` constructor genuinely private | Grep: no public `new`, no public `From` — only verifying `load` |
| `SystemVersion.hash()` uses exhaustive destructuring | `let Self { … } = self`, not field access. **Prove:** add dummy field → `cargo check` must fail → remove |
| Budget drain is CAS loop, not `fetch_sub` | Grep `compare_exchange` in `exec.rs`; 8-thread test is the runtime proof |
| `mean_zero_null` present (three policies, not two) | Grep + test all three on `[1,null,3]` |
| `PcTaint`: stack-based, join-on-enter, nesting inherits | Code review + nested-branch test |

---

### D0.3 — Hash provenance freeze

Current pinned hashes belong to the **in-repo** serialiser and are now law:

| Fixture | Hash |
|---------|------|
| `canonical_form_reference` | `a66bcd6a407d14189842ba22dc2627c7fdc023acccd7836a8e9d3fe40f4f2f80` |
| `float_sum_reference` | `0140b77a15543ec6dd2a95205d3c3e2982e7cc673ae12245440c4c61561e1d4d` |

Add to `tests/determinism.rs`:

```rust
//! Provenance: workspace blake3, x86_64, aelio-sol canonical §4.3.
//! A change to either hash is a SystemVersion.serialiser bump + migration — NEVER a fixture edit.
```

If D0.1 fixes change serialisation: **re-pin once** at end of D0, record in FLAGS, then freeze.

---

### Exit D0

- [ ] Every listed test **PRESENT** and green (≥54 total)
- [ ] Three API shapes confirmed (D0.2)
- [ ] Hashes frozen with provenance comment (D0.3)
- [ ] Any behavioural difference → Q103 entry in FLAGS, same session

**Nothing in D1–E starts until D0 exit is checked.**

---

# PART 1 — SESSIONS D1–D6 (+ E)

### Session D1 — CI and drills still open

1. **harness-core on aarch64** — extend CI matrix (today: aelio-sol only). Pinned hashes must match x86_64, debug and release. **M1 exit gate — still open.**
2. **A4 process-restart drill** — run hash suite → restart process → run again → diff identical. Only test that catches per-process HashMap seed in hash path.
3. Fast-math grep + `preserve_order` feature-tree check workspace-wide.

### Session D2 — Per-step replay in REAL execution

Report marks B.4 **Partial**: dev-dep test proves `verify_replay` works; kernel does **not** emit steps from live graph execution.

Wire `Step { seq, inputs_hash, output_hash }` at wavefront node completion; persist with ledger; integration test: 3-node graph live → replay → corrupt middle step → localised by seq.

### Session D3 — Effect contracts (Task 5)

Registration refuses tools without `effect_class` / `completeness` / `returns_entity`; backfill; `Completeness::Unknown|Sampled` blocks aggregate promotion; idempotency `(turn_key, effect_seq, element_index)`.

### Session D4 — Op catalog + trybuild + time (Task 6)

1. Register harness-core ops with `impl_hash`; Merkle → `SystemVersion.op_catalog`. Record each `std.*`: catalog op vs bridge-only.
2. **Trybuild** — `tests/ui/unverified_effect_set.rs`; SystemVersion missing-field compile-fail.
3. Time module: UTC epoch ms, pinned tzdb in-crate, `TemporalBinding`, tenant-tz buckets. **Check bridge warm path** for time-relative cache — if yes, urgent.

### Session D5 — Residue adversarial C1–C9 (Task 8)

Each case asserts **named gate**. C3 (median vs mean) must name **verification** gate. Bangalore / clause-drop question: test bridge σ-matcher, not argument.

### Session D6 — GraphSuspension B7 end-to-end

Client tool unbound param → suspend → user answers → resume completes → both turns linked. Negative: unusable answer → re-Clarify, not crash/guess.

### Session E — Starlark (Task 7, last, F-033)

Toolchain → dialect lockdown at startup → tainted values day one → budget threaded (heap-limit test) → spawn_blocking bridge → cycle check → F-034 codegen (text → parse → AST → canonical re-render → hash).

### Continuous — Gate J prep

Reuse counters live (Session C). After **one week** of traffic: review executions-per-key distribution → record verdict in FLAGS **before Session E**.

---

# PART 2 — COMPLETION GATES

"Fully complete" = every gate green (Gate J recorded, not pass/fail). A red gate **blocks everything listed after it**.

### GATE 1 — Substrate determinism [blocks all]

- [ ] D0 reconciliation: ≥54 tests, all listed behaviours
- [ ] `cargo test -p harness-core` debug == release hashes
- [ ] aarch64 CI: harness-core pinned hashes match x86_64
- [ ] A4 restart drill green
- [ ] `check-canonical-writer.sh` green
- [ ] Trybuild: Verified&lt;T&gt;, SystemVersion missing-field

### GATE 2 — Execution integrity [blocks matching + Starlark]

- [ ] D2: real per-step traces; corrupt step localised by seq
- [ ] A7: truncated journal → `Journal.Underrun`; zero tool dispatches on replay
- [ ] F2: 8-thread budget drain exact total
- [ ] F3: exhaustion names dimension+frame; no numeric in response
- [ ] F4: model_calls=0 refuses; zero-budget = exhausted
- [ ] H2 both directions: bad cache hash refused; valid-hash-smaller-set → `AstExceedsEffectSet`

### GATE 3 — Effect safety [blocks real tools]

- [ ] D1 shadow write → counter 1, `stubbed: true`
- [ ] D2 retry same turn_key → counter 1; different body → reject
- [ ] D3 map_tool 5 elements → 5 keys, 5 writes
- [ ] D4 write transient → surfaces, no auto-retry; read retried
- [ ] D5 element 57/100 fails → whole call errors naming 57
- [ ] D7 incomplete registration hard-fails

### GATE 4 — Taint [blocks Starlark conditionals]

- [ ] E1–E6: nine positions, launder chain, map-key both directions, implicit flow, fail reason, declassify_count only exit

### GATE 5 — Matching precision [blocks reuse on real traffic]

- [ ] C1–C9 green, each names its gate
- [ ] B1–B5 product loop (B4 thesis, B5 structure change re-authors)
- [ ] Bridge Bangalore / clause-drop test
- [ ] **Red rule:** any C-case red → **disable matching** (cold everything) until green. Confidently wrong &gt; down.

### GATE 6 — Sessions and time [blocks external users]

- [ ] G1–G4, B6/B7, I1–I4, H1/H4/H6

### GATE 7 — Starlark [final]

- [ ] Dialect hash at startup; F-034 round-trip; first LLM harness cache hit on re-request

### GATE J — The verdict [not pass/fail]

- [ ] One week executions-per-key distribution reviewed → verdict in FLAGS (before Session E)
- [ ] J3: 20 warm hits re-run cold; disagreements → golden case + gate bug

**Definition of done:** Gates 1–7 green, Gate J recorded. Gate J answers whether the world wants reuse — top-heavy → full speed; flat → check σ fragmentation; genuinely flat → honest conversation before Starlark.

---

## Document map

```
docs/updates/
├── PHASE_2D_INSTRUCTIONS.md      ← THIS FILE (D0 first, then D1–E, gates)
├── AELIO_IMPLEMENTATION_FINAL.md ← what landed in A–C
├── AELIO_PHASE_FINAL_STATUS.md   ← status snapshot
├── PHASE_2_ACCEPTANCE_SUITE.md   ← test IDs (C1–C9, A4, B4, …)
└── PHASE_2_INSTRUCTIONS.md       ← original Tasks 1–10
```
