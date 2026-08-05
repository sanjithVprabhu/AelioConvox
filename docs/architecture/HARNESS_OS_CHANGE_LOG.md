# Harness OS — change log and tracker

**Purpose:** a running record of what was actually changed, by whom, and what it proves — so progress
can be tracked without re-deriving it from `git diff`.
**Session:** 2026-08-04 → 2026-08-05
**Companions:** `HARNESS_OS_EXECUTION_PLAN.md` (target) · `HARNESS_REMAINING_WORK.md` (gates) ·
`Blueprint/CONDUCTOR_HARNESS_E2E_CHECKLIST.md` (parallel session's tracker)

> **Important:** this repo had **two agent sessions working concurrently** on 2026-08-04. Attribution
> below separates them, because they collided once (see C-01). Verify before assuming ownership.

Legend: `[x]` done + test-proven · `[~]` partial · `[ ]` open

---

## 1. Changes made in this session (mine)

| # | Change | File | Proof | Status |
|---|---|---|---|---|
| M-01 | Restored deleted `contract` re-export — unblocked the whole crate's test compilation | `aelio-agent/src/lib.rs:36` | `cargo test -p aelio-agent` compiles all targets | [x] |
| M-02 | `MAX_INSTALLED_CONTRACTS = 4096`; `list_contract_ids` now **errors** instead of silently truncating | `aelio-kernel/src/sol_harness_lib.rs` | scan requests `limit+1` and refuses on saturation | [x] |
| M-03 | Diagnosed + resolved the learning-loop regression (2 failing tests) | `aelio-agent/tests/durable_workers.rs` | `durable_workers` 23/23 green | [x] |
| M-04 | New test: greeting contract under Conductor | `durable_workers.rs` | `conductor_answers_greetings_without_cold_path_or_model_cost` | [x] |
| M-05 | FLAGS entry documenting the narrowed learning surface | `FLAGS.md` | F-024 | [x] |
| M-06 | Extended P0 pure stdlib targets: 10 → 20 registered Call ids | `aelio-kernel/src/stdlib_targets.rs` | `binary_pure_slice_is_registered_and_evaluates` | [x] |
| M-07 | Guard test: nondeterministic families must not ship as `Pure` | `stdlib_targets.rs` | `nondeterministic_families_are_not_declared_pure` | [x] |
| M-08 | Execution plan authored | `docs/architecture/HARNESS_OS_EXECUTION_PLAN.md` | 666 lines, 8 phases, ~184-harness catalog | [x] |
| M-09 | Gate tracker authored | `docs/architecture/HARNESS_REMAINING_WORK.md` | gates H0–H7 + HS | [x] |
| M-10 | This change log | `docs/architecture/HARNESS_OS_CHANGE_LOG.md` | — | [x] |
| M-11 | **Unhung the test suite** — 7 unbounded `socket.next().await` reads now use a 20s timeout helper | `aelio-agent-api/tests/api.rs` | suite went from *never returning* to finishing in 1.19s | [x] |
| M-12 | Legacy-parity constructors made effective again (were silently inert) | `aelio-agent-api/src/lib.rs` | `rust_turn_api_executes_and_replays_a_durable_flow` + SDK socket tests green | [x] |
| M-13 | Sol cutovers skipped when an **admitted artifact runtime** owns the turn (pinning invariant) | `aelio-agent-api/src/lib.rs` | 4 lowered/materialized-flow tests green | [x] |
| M-14 | `api.rs` 12 pass/8 fail/2 hang → **19 pass / 3 fail / 0 hang** | — | see §7 | [~] 3 remain |
| M-15 | **Phase 1 contract model** — `SolHarnessContract` gains version/library/signature/effect/budget/identity | `harness_contract.rs`, `sol_harness_lib.rs` | 5 phase1 unit tests + every seed admits | [x] |
| M-16 | Identity = Mother §4.3 canonical Sol + BLAKE3 (`aelio_sol::value_hash`), never SHA-256 | `harness_contract::contract_identity` | `identity_stable_and_blake3_hex` (64 hex) | [x] |
| M-17 | Admission: compile + budget ceiling + Call registration + effect rank | `admit_contract` / `store_library_admitted` | `phase1_every_library_contract_admits_with_seed_registry` | [x] |
| M-18 | Backward-compatible store load for pre-Phase-1 3-field rows | `from_store_value` defaults | `phase1_legacy_three_field_rows_load_with_defaults` | [x] |
| M-19 | **Phase 2 Call ISA freeze** — process/tool/llm/memory/conv families | `aelio-kernel/src/call_isa.rs` | 11 `call_isa::*` tests green | [x] |
| M-20 | `tool.invoke@1` default stub + F-026 host-proxy rules (Once + §12.4) | `call_isa` + `FLAGS.md` | `tool_invoke_stub_echoes` | [x] |
| M-21 | Prompt artifact pins (BLAKE3) required by every `llm.*` Call | `PromptArtifactPin` / `seed_prompt_pins` | `llm_requires_known_prompt_id` | [x] |
| M-22 | `harness.invoke_seq@1` deterministic ordered fan-out/join | `call_isa::register_invoke_family` | `invoke_seq_runs_children_in_order` | [x] |
| M-23 | `harness.describe@1` / `list@1` over seed catalog | catalog from `sol_harness_library` | `describe_and_list_from_catalog` | [x] |

### M-19 detail — frozen Call families

| Family | Ids (representative) |
|---|---|
| Process | `harness.invoke@1`, `spawn@1`, `invoke_seq@1`, `return@1`, `exit_up@1`, `fresh@1`, `describe@1`, `list@1` |
| Tool | `tool.invoke@1` (canonical), `tool.act_stub@1` (legacy seed) |
| LLM | `llm.classify@1` … `llm.rerank@1` (9 ids; all require `prompt_id`) |
| Memory | `memory.search/write/forget@1`, `page.*`, `slot.*`, `context.attach@1` |
| Conv | `express.say@1`, `understand.classify@1`, `compute.hold@1` |
| Pure stdlib | `math.*`, `logic.*`, `collection.*` via `stdlib_targets` (allowed by `is_allowed_call_id`) |

Exit criterion met: Call table written down with effect + arg schema; seed library admits under
`frozen_admission_registry`; nested `harness.invoke@1` still green.

### M-15 detail — Phase 1 fields

```
SolHarnessContract {
  id, version, summary, library, program_json,
  signature: HarnessSignature { inputs, outputs, errors },
  effect: EffectClass, budget: BudgetSpec, identity: String // blake3 hex
}
```

Seed constructors use `SolHarnessContract::seed(id, summary, program_json)` which seals identity and
infers a coarse effect from Call ids. Full registry admission is optional via
`store_library_admitted(store, tenant, Some(&seed_admission_registry()))`.

### M-06 detail — new Call targets

Added as thin wrappers over existing `compute::apply` fns, so **no kernel opcode was added**:

```
math.modulo@1   math.min@1   math.max@1
logic.not_equals@1  logic.lt@1  logic.lte@1  logic.gte@1
logic.and@1  logic.or@1  logic.not@1
```

Deliberately **not** added: `time.now@1`, `id.uuid@1`, `math.random_bounded@1`. These are ledgered
nondeterminism and must not be declared `EffectClass::Pure`. M-07 asserts they stay out.

---

## 2. The one substantive finding — F-024

**What broke:** with the Conductor default-on, `TurnRuntime::run` returns at `LookupTier::Tier3` for
every starter selection except `Escalate`. `learn_from_turn` sits *inside* the ProposePath branch
below that return, so Conductor-handled turns stopped feeding `observe_and_promote`.

**Why nobody saw it:** `tests/durable_workers.rs` had not compiled since the harness slice landed
(the deleted `contract` re-export, M-01). Two failing tests were invisible behind a build error. This
is the concrete cost of the H0 gate and the reason it was sequenced first.

**Measured, not assumed.** I probed the actual behavior before touching anything:

| Utterance | Tiers across 5 turns | Learning observed |
|---|---|---|
| `"hi"` | Tier3 ×5 | **no** — Conductor answers it |
| `"list jobs"` | Tier2 ×3 → **Tier0** | yes, promoted durably |
| `"send otp to 900"` | Tier3 ×3 → **Tier0** | yes |
| `"cancel my order"` | Tier3 ×3 → **Tier0** | yes |

So the learning ladder is **fully intact**. Only utterances the Conductor answers directly stopped
learning — and for those it already reaches 0 `llm_calls` on turn **one**, where the old loop needed
3 observations to promote before reaching the same cost.

**Resolution (F-024 option a).** Accept the narrowing; it is an improvement, not a loss. But split the
two properties so neither is dropped:

- `repeated_success_promotes_durably_and_next_turn_is_tier_zero` → retargeted to a cold-path
  utterance. Its subject is promotion mechanics; `"hi"` was an incidental vehicle.
- `durable_promotion_and_tier_one_use_the_same_configured_embedding_space` → same retarget. Its
  subject is embedding-space identity.
- The greeting guarantee those tests used to carry (*"authored greeting learning must not manufacture
  model-token cost"*) is now asserted **more strongly** in M-04: zero model calls, no `ProposePath`,
  answered by a harness, and no learned procedure at all.

**What I did not do:** I did not delete a failing assertion to go green. The dropped
`token_cost_sum == 0` line was greeting-specific and its guarantee moved to M-04.

---

## 3. Concurrency incident — read this before trusting attribution

| # | Event | Detail |
|---|---|---|
| C-01 | Two sessions applied the **same** one-line H0 fix | `lib.rs` briefly contained the `pub use contract::{…}` line **twice** → duplicate-import compile error. I removed the duplicate (convergent, not a revert). |
| C-02 | `sol_harness_lib.rs` grew 73,547 → 76,123 bytes mid-command | `workflow_average_contract()` appeared while I was reading the file |
| C-03 | I stopped and asked before proceeding | Resumed only after confirming no edits for 10 minutes |

**Rule going forward:** one session per crate, or use `EnterWorktree` for isolation. The collision
produced a broken build in under 60 seconds.

---

## 4. Work landed by the parallel session

Not mine — recorded so the tracker is complete. Sizes from `wc -l`.

| Module | Lines | Role |
|---|---|---|
| `aelio-kernel/src/process_tree.rs` | 1139 | instance tree, joins, cancellation |
| `aelio-kernel/src/os_contract.rs` | 641 | HarnessContract / Event / Action / Instance schemas |
| `aelio-kernel/src/harness_syscalls.rs` | 533 | spawn / await / join syscalls |
| `aelio-kernel/src/process_store.rs` | 492 | durable process records |
| `aelio-kernel/src/promote.rs` | 433 | promotion gates |
| `aelio-kernel/src/tree_replay.rs` | 406 | tree-level replay |
| `aelio-kernel/src/event_admission.rs` | 288 | normalized event admission |
| `aelio-kernel/src/shadow.rs` | 276 | shadow-mode comparison |
| `aelio-kernel/src/library_bundle.rs` | 240 | on-disk library install |
| `aelio-kernel/src/cutover.rs` | 108 | route cutover control |
| `aelio-kernel/src/tool_workflows.rs` | 461 | tool/effect workflows |
| `aelio-kernel/src/sol_harness_lib.rs` | 2166 | **40** Sol harness contracts |
| API tests | — | `v2_events.rs`, `shadow_turns.rs`, `tool_harness_turns.rs` |

All wired into `aelio-kernel/src/lib.rs`.

---

## 5. Current state

**Test posture:** `durable_workers` 23/23 · `aelio-kernel stdlib` 3/3 · `aelio-agent` lib 112 ·
`harness_conductor` 6 · `golden_traces` 24 · `harness_program_benchmark` 2.
Full `cargo test --workspace` was running at the time of writing — **confirm before release.**

**Library:** 40 Sol harness contracts · 20 registered pure Call targets.

---

## 6. Still open — honest list

Merged from my gates and the parallel session's checklist. **None of these are done.**

| # | Item | Source | Why it matters |
|---|---|---|---|
| O-01 | Old agent orchestration still an authority — dual IR alive | E2E 6.1, my D1 | Two runtimes = silent behavior forks. `HarnessStepV1` must be deleted, not maintained |
| O-02 | Conductor selection still a Rust keyword ladder | my Phase 4.2, E2E 3.2 | Cannot see the 31 registered tools; installing a harness does not make it selectable |
| O-03 | `harness.spawn/await/join_all` not wrapped as registry Calls | E2E 3.1 | Rust API exists; programs cannot call it yet |
| O-04 | Harness identity still `serde_json` + SHA-256, not canonical Sol + BLAKE3 | my H2-1 | Same defect class already remediated as G1-3 |
| O-05 | `content_hash` uses `unwrap_or_default()` | my H2-2 | Two failing serializations hash identically |
| O-06 | Prompts still inline format strings, duplicated across two sites | my H4-1 | Blocks Phase 7 — drafted harnesses would reference unpinnable prompts |
| O-07 | `time.*` / `id.*` target families | E2E 1.5 | Need ledgered-nondet plumbing, not `Pure` wrappers |
| O-08 | Retrieval + model disambiguation over top-K | E2E 5.1 | Rule select works; needed before the catalog gets large |
| O-09 | Canary + pin rollback | E2E 4.6 | No safe rollout path |
| O-10 | Security review, load/soak, restore drill | E2E 6.3 | Production gate |
| O-11 | Learn candidate → shadow → promote (create-harness) | E2E 7.1, my H5 | The growth loop; zero lines written |
| O-12 | Tenant authoring SDK/CLI, domain libraries | E2E 7.2/7.3 | Ecosystem |
| O-13 | The harness slice is still **uncommitted** | my H7-1 | `harness/`, its tests, and `docs/architecture/` are untracked |

---

## 7. Recommended next three

1. **O-13 — commit.** The tree holds ~1,771 insertions of uncommitted work across two sessions plus
   a dozen untracked files. Everything else risks losing it.
2. **O-04 + O-05 — the hash.** Small, mechanical, and it is an *identity* baked into persisted turn
   traces. Every day it stays SHA-256 makes the eventual migration touch more stored data.
3. **O-02 — Conductor selection from the registry.** It gates O-11: a growth loop that installs
   harnesses the Conductor cannot select produces nothing observable.

Do **not** start O-11 before O-02, O-03, and O-06 — a drafted harness needs a selectable slot, a
callable spawn, and a pinnable prompt, or it is written to a dead end.

## 2026-08-05 — OTP park/resume, OS-default turns, Phase 0.4 scan

- **Phase 0.4:** `list_contract_ids_handles_three_hundred_without_truncation` (MAX=4096, refuse if over).
- **OTP journey:** Sol `tool.otp_login` send→Park→verify (kernel `otp_journey.rs`); API multi-turn via `otp_sessions_v1` (send → code `123456` → authenticated).
- **Path B default:** agent dual-IR only if `AELIO_AGENT_LEGACY=1`; otherwise fail-closed if no harness (full_reply catch-all covers free-form).
- **Tests:** kernel lib 48+; `otp_multi_turn` API green.

---

## 7. `api.rs` — the three remaining failures, root-caused

Start of session: **12 pass / 8 fail / 2 hang forever.** Now: **19 pass / 3 fail / 0 hang.**

The two hangs were the reason nobody had seen the eight failures: `cargo test` never returned, so
every run was killed before printing results.

### Why the parity constructors were inert (fixed, M-12)

Path B was made the default: the agent spine ran only when the process-global `AELIO_AGENT_LEGACY=1`
was set. But parity is a **per-AppState** property, chosen by `new_for_legacy_parity` /
`new_with_scoped_sdk_bridge_for_legacy_parity` and already recorded on the world. Gating it on a
global env var made those constructors do nothing: fixtures that asked for the parity interpreter got
the closed "I could not load a harness" fallback. Fixed by reading the AppState's own flag, and by
skipping the Sol cutovers when it is set — a parity fixture that still lost its turns to a cutover
would not be parity.

### Why admitted artifacts must outrank cutover (fixed, M-13)

`new_with_artifact_runtime` sets `legacy_parity: false`, so the kernel's tool-harness cutover was
preempting **materialized, admitted, pinned** flow artifacts by matching on utterance text. That
violates the Blueprint's pinning invariant — *"runtime semantic lookup cannot silently change a
running instance."* The cutover exists to replace the cold ProposePath, not an admitted program.
An installed artifact runtime is now treated as an authority in its own right.

### The three that remain

| Test | Root cause | Decision needed |
|---|---|---|
| `admitted_bound_procedure_executes_through_unified_runtime_and_replays` | **F-024.** Drives 5 × `"hi"`, expects `tier2` + a promoted procedure. Conductor now answers greetings, so no procedure is ever learned. The fixture artifact is literally `procedure.greeting`. | Retarget the fixture off greetings (artifact + utterance together), **or** add a parity switch that disables Conductor starters for pre-Conductor learning fixtures |
| `learning_control_plane_can_inspect_and_kill_a_promoted_procedure` | **F-024.** Same: 5 × `"hi"` then expects `/v1/admin/procedures` to be non-empty. | Same as above |
| `unmaterialized_public_flow_fails_closed_before_a_runtime_proxy_effect` | **F-022.** Expects a `FlowMaterialization` "demand recorded" step. That branch requires `flow.lowering.is_some()`; the demo tenant's login flow is `lowering: None` (`world.rs:371`), so it takes the *semantic-only skip* path instead and records nothing. | Declare a real `lowering` on the demo login flow — the fix F-022 itself prescribes ("author control: ship `aelio.lowering` to opt a flow into the install"), and what the demo needs anyway |

**Deliberately not done unilaterally.** Two of these need a third and fourth test retarget, and the
third changes demo-tenant catalog shape with blast radius across the 19 now-passing tests. Reshaping
more tests to match the code is exactly the failure mode this log exists to prevent — these are
product decisions, not cleanups.

### Correction to the recommended order (measured 2026-08-05, after the push)

An earlier version of this file called the F-022 fix "one product change". **That was wrong** — I
checked the blast radius before attempting it:

- Declaring `lowering` makes a flow an *executable OS app* that **must pin before the catalog is
  ready** (`tenant.rs:228-233`).
- `process_turn` refuses every turn while `install.executable_pending > 0`
  (`aelio-agent-api/src/lib.rs:678`) with `503 Unavailable`.
- **16 files** build on `World::demo_tenant`.

So declaring a lowering on the demo login flow without also authoring its Sol program, capability
bindings and gate cases *and* making it materialize in every fixture would turn every demo-tenant
turn into a 503 — far past the 3 tests it fixes. There is currently no `lowering: Some(...)` anywhere
in the tree to copy from.

**Actual recommended order:**

1. **F-024 first** — it is the smaller, self-contained decision (fixture strategy for 2 tests).
2. **F-022 second, as a scoped piece of work**: author `FlowLoweringV1` for login (Sol program +
   `$cap:` bindings + gate cases), materialize/pin it in fixtures, *then* the fail-closed test passes.
   Budget this as real work, not a cleanup.

**Push checkpoint:** commit `f4970b30` on `aelio-final-wrap`, pushed to origin — 146 files,
22,903 insertions. Everything from here is revertible.
