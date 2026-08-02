# Unified completion claim — audit findings (no code changed)

**Audit date:** 2026-08-02  
**Auditor posture:** verify claims only; **do not implement** in this pass.  
**Branch context:** `sanjith-2` with a large **uncommitted** change set (~80 paths) claiming local production audit completion.

---

## Executive verdict

| Claim class | Verdict |
|---|---|
| Direction of the unified architecture | **Mostly correct** — Rust-owned artifacts, FlowLoweringV1, subject continuations, TS authority scan, and fail-closed semantic-only flows are real in code + targeted tests. |
| “Everything is complete / production audit passed” | **Overstated as a ship declaration.** Strong local evidence exists, but mother-locked invariant gaps remain, §14.1 ToolHost removal is incomplete, docs disagree with themselves, and the auditor did **not** independently re-run the full claimed gate (`cargo test --workspace`, `clippy -D warnings`, `pnpm test:all`). |
| External limitation (`PRODUCTION-LIVE-001`) | **Correctly stated** as external (credentials / load / soak / managed crash-window). |

**Bottom line:** Treat the machine’s report as *“local unified cutover scaffolding + scenario tests largely landed”*, not as *“mother-complete + ship-ready without follow-ups.”* Use the change list below as the remediation backlog before trusting the green ledger.

---

## What was independently confirmed (spot checks)

These claims held under local verification on this machine:

1. **Requirements ledger shape:** `docs/requirements/aelio.json` reports **28 implemented / 0 partial / 0 missing / 1 external** (`PRODUCTION-LIVE-001`). Validator: `node scripts/validate-aelio-requirements.mjs` → OK.
2. **Production authority scan:** `node scripts/check-production-authority.mjs` → Rust is the only turn decision executor (28 reachable TS modules).
3. **Spec convergence script:** `node scripts/check-aelio-spec-convergence.mjs` → OK.
4. **FlowLoweringV1 rejects `escape.kind = fallback`:** code in `aelio-os/crates/aelio-agent-api/src/flow_lowering.rs` (`apply_escape`) + test `unified_catalog_requires_exact_effect_and_refuses_unsupported_fallback_handoff` **passed**.
5. **Unified world does not hydrate/persist legacy flow instances when legacy is disabled:** test `unified_world_never_hydrates_or_persists_legacy_flow_instances` **passed**.
6. **Production constructor path:** `AppState::new_with_artifact_runtime` calls `disable_legacy_flow_execution()`, installs `RuntimeArtifactToolHost` + `RuntimeAdaptiveArtifactHost`. `aelio-server` uses this constructor.
7. **Public-boundary evidence index exists** and names tests that exist in-tree (`api.rs`, `unified_prism.rs`, `public_drain.rs`, etc.).
8. **Honest limitation on fallback handoff** matches code comments: escalate / free_range supported; fallback intentionally rejected pending reactor-owned handoff format.

---

## What was *not* independently re-verified here

Do **not** treat these as auditor-confirmed on this pass (they may have passed on the other machine):

| Claimed gate | Status in this audit |
|---|---|
| `cargo test --workspace --all-features` | Not fully re-run (hours-long). Only targeted tests above. |
| Multi-seed DB oracle simulation | Not re-run. |
| Full Prism adversarial / lifecycle / recovery suites | Not fully re-run. |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Not re-run. |
| Rust fmt + `git diff --check` | Not re-run; working tree is dirty. |
| `pnpm typecheck` 16/16 | Not re-run. |
| `pnpm test:all` | Not re-run. |

**Remediation:** before accepting the completion claim as release evidence, re-run the full gate on a clean tree and paste logs into CI artifacts.

---

## Required changes (robust backlog)

Priority: **P0** = mother / authority integrity; **P1** = cutover completeness; **P2** = docs / hygiene / ops.

Each item: **what**, **why**, **where to implement**, **acceptance**.

---

### P0-1 — Clamp pathway prototypes to mother §18 (cap 8)

- **What:** Change `MAX_PROTOTYPES` from `256` to `8` (or introduce a Decision Log amendment + FLAGS entry if 256 is intentional).
- **Why:** Mother `AELIO_DSL_MOTHER.md` §18 locks **“Cap: 8 pathways per decision point (v0)”**. `256` silently weakens a locked invariant.
- **Where:**
  - `aelio-os/crates/aelio-runtime/src/artifact/views.rs` (`MAX_PROTOTYPES`, validation)
  - `aelio-os/crates/aelio-runtime/tests/artifact_views.rs` (update fixtures/assertions)
  - If keeping 256: `FLAGS.md` new entry + mother Decision Log amendment (never silent)
- **Acceptance:** validation rejects >8 prototypes; tests cover boundary 8 and 9; FLAGS/mother aligned.

---

### P0-2 — Adaptive decision sealing must use mother-canonical hashing (or be flagged)

- **What:** Replace SHA-256-over-`serde_json` decision hashing with mother §4.3 canonical Sol serialization + BLAKE3, **or** open an explicit FLAGS entry documenting provisional SHA-256 with migration plan.
- **Why:** Mother authority for content-addressed identity is BLAKE3 + canonical bytes. Adaptive seals currently use `sha2::Sha256` over JSON bytes in `adaptive.rs` (`compute_hash`). That invents a second hash regime.
- **Where:**
  - `aelio-os/crates/aelio-agent/src/adaptive.rs` (`compute_hash`, envelope sealing/verify)
  - Callers / tests in `aelio-agent-api/tests/api.rs`, `aelio-agent/tests/golden_traces.rs`
  - Optionally bridge verify path in `aelio-agent-api/src/adaptive_bridge.rs`
  - `FLAGS.md` if deferred
- **Acceptance:** same logical decision ⇒ stable mother-canonical hash across platforms; unknown fields still rejected; FLAGS either closed or explicitly PROVISIONAL.

---

### P0-3 — Flag or remove other invented constants

- **What:** Inventory and either (a) cite mother sections, (b) move into §33/App constants, or (c) FLAGS as PROVISIONAL with rationale.
- **Why:** Handoff rule: absent constants → flag, don’t invent silently.
- **Where (known sites to review):**
  - `aelio-os/crates/aelio-agent/src/adaptive.rs` — envelope size/depth/score bounds, candidate cap (8 is good if tied to §18)
  - `aelio-os/crates/aelio-runtime/src/builder.rs` — RRF `k` / ranking constants
  - `aelio-os/crates/aelio-runtime/src/learning.rs` / gate thresholds — miner eligibility vs shadow gate (20 / 0.95)
  - `aelio-os/crates/aelio-runtime/src/gate.rs` — agreement thresholds
- **Acceptance:** every magic number has a mother cite, App constant, or FLAGS id.

---

### P1-1 — Finish master-plan §14.1 “agent without ToolHost” acceptance

- **What:** Make it possible to compile / construct the production turn path **without** a legacy-capable `ToolHost` execution adapter inside `aelio-agent` (effects only via runtime-owned proxies / adaptive host). Keep `ToolHost` only behind `cfg(test)` / parity modules if needed.
- **Why:** Master plan §14.1 acceptance still says compiling without legacy ToolHost adapter must be possible. Today `TurnRuntime` still requires `tool_host: &mut dyn ToolHost` (`blocks/turn.rs`), and production still installs a `ToolHost` (even if it is `RuntimeArtifactToolHost`).
- **Where:**
  - `aelio-os/crates/aelio-agent/src/blocks/turn.rs`
  - `aelio-os/crates/aelio-agent/src/abilities/invoke.rs`
  - `aelio-os/crates/aelio-agent/src/runtime/world.rs`
  - `aelio-os/crates/aelio-agent-api/src/runtime_tool_host.rs`
  - `aelio-os/crates/aelio-agent-api/src/lib.rs` (constructor surface)
  - Update §14.1 checklist text only after code proves it
- **Acceptance:** production binary path has no agent-local effect dispatch; feature/cfg or type split prevents accidental ToolHost effect execution; tests fail if agent dispatches effects directly.

---

### P1-2 — Harden all production constructors against legacy flow execution

- **What:** Every production `AppState` constructor must call `disable_legacy_flow_execution()` (or start from a World that defaults false and cannot be flipped in prod). Deprecate or gate `new_with_sdk_bridge` so it cannot silently leave a dual rail.
- **Why:** `new_with_artifact_runtime` disables legacy; `new_with_sdk_bridge` (`install_host: true`) does **not**. Footgun if any host still uses the SDK-host constructor without the artifact runtime path.
- **Where:**
  - `aelio-os/crates/aelio-agent-api/src/lib.rs` (`build`, `new_with_sdk_bridge`, `new`)
  - `aelio-os/crates/aelio-server/src/main.rs` / `lib.rs` (assert which constructor is used)
  - Tests proving constructors cannot hydrate `LogicalTable::FlowInstances` in prod mode
- **Acceptance:** grep/source scan: no production constructor leaves `legacy_flow_execution_enabled == true`; negative test included.

---

### P1-3 — Reconcile evidence doc vs requirements status

- **What:** Update `docs/new_arch/AELIO_PUBLIC_BOUNDARY_SCENARIO_EVIDENCE.md` “Release interpretation” so it no longer says `E2E-PUBLIC-001` *may move from partial* while `aelio.json` already marks it `implemented`. Either demote the requirement back to `partial` until the gate is re-run on a clean tree, or update the evidence doc to point at the completed gate artifacts.
- **Why:** Internal contradiction undermines the audit trail the machine cites.
- **Where:**
  - `docs/new_arch/AELIO_PUBLIC_BOUNDARY_SCENARIO_EVIDENCE.md` (§ Release interpretation)
  - `docs/requirements/aelio.json` (`E2E-PUBLIC-001`)
  - `docs/claude_context/AELIO_UNIFIED_IMPLEMENTATION_MASTER_PLAN.md` §14 / §14.4
- **Acceptance:** one status, one evidence pointer, no “may move from partial” after `implemented`.

---

### P1-4 — Record independent full-gate evidence on a clean tree

- **What:** Commit or stash cleanly, then re-run and archive:
  - `cargo test --workspace --all-features`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `cargo fmt --check` / `git diff --check`
  - `pnpm typecheck`
  - `pnpm test:all`
  - Prism adversarial + multi-seed oracle (whatever scripts the claim referenced)
- **Why:** Completion was asserted on a dirty uncommitted tree; this audit only spot-checked. Release claims need reproducible artifacts.
- **Where:**
  - CI: `.github/workflows/ci.yml`
  - Optional: `docs/operations/` gate log index or CI run URLs linked from §14.4
- **Acceptance:** green CI on the exact commit hash that declares completion; no “passed locally on dirty tree” as sole evidence.

---

### P2-1 — Render Protocol F5 (frame ledgering) completeness

- **What:** Ensure every `RenderFrame` / `EventFrame` / Hello/Welcome is ledgered as a reaction per `AELIO_RENDER_PROTOCOL.md` F5, or keep F-020 explicitly partial for ledgering.
- **Why:** FLAGS F-020 is marked ratified v1, but protocol F5 (replayable screens) is stronger than “widget got a text@1 frame.”
- **Where:**
  - `aelio-os/crates/aelio-agent/src/abilities/express.rs`
  - Durable turn / decision log writers under `aelio-agent/src/runtime/durable.rs` / decision log
  - `packages/chat-sdk` + widget only as consumers
  - `FLAGS.md` F-020 status text
- **Acceptance:** resume/replay reconstructs frames; or F-020 explicitly lists ledgering as deferred.

---

### P2-2 — Clarify dual ladders: mother P0 vs unified master DoD

- **What:** Add a short ops note that `CLAUDE.md` / mother §30 P0 (Sol → vectors → login golden via kernel/cli) is a **different ladder** from unified master Phases 0–9 / AGENT-UNIFICATION. Completing unified local cutover ≠ closing every mother P0 annex/vector claim.
- **Why:** Prevents false confidence when reading “architecture complete.”
- **Where:**
  - `docs/operations/AELIO_PRODUCTION_RUNBOOK.md` or new `docs/operations/AUTHORITY_LADDERS.md`
  - Cross-link from master plan §13–§14
- **Acceptance:** onboarding docs state both ladders and how they relate.

---

### P2-3 — Commit hygiene for the completion claim

- **What:** Land the uncommitted unified cutover as reviewed commit(s) with messages citing sections (Phase 6/7/9, FlowLoweringV1, public scenarios). Do not leave “completed” only as working-tree state.
- **Why:** ~80 dirty paths; completion is not reviewable/reproducible until committed.
- **Where:** git history on `sanjith-2` (or release branch); split commits by concern (kernel, runtime artifacts, agent cutover, server public tests, docs/FLAGS).
- **Acceptance:** `git status` clean on the release commit; CI green on that SHA.

---

### P2-4 — Document remaining deliberate non-goals

- **What:** Keep a short “known unfinished” list next to the completion claim:
  - `FlowLoweringV1` has no safe `fallback` escape (by design)
  - Atomic adaptive hot path remains Pure / non-Park (suspendable flows use subject continuations)
  - `PRODUCTION-LIVE-001` external
  - Legacy interpreter retained for parity/local worlds
- **Why:** The machine’s note on fallback is good; other caveats should be equally visible so “complete” isn’t misread.
- **Where:** `docs/operations/AUTHORED_FLOW_LOWERING.md` + master plan §14.4
- **Acceptance:** each caveat has a code pointer and a future format / phase id.

---

## Claim-by-claim scorecard

| Machine claim | Audit |
|---|---|
| Rust sole production decision/execution authority | **Mostly true** for `new_with_artifact_runtime` + authority scan; ToolHost still present as adapter type (P1-1). |
| TS limited to SDK/transport/channels/providers/tools | **Supported** by authority scan; keep monitoring. |
| Authored workflows → immutable hash-pinned artifacts | **Supported** by FlowLoweringV1 + gate path. |
| Capability bindings → exact tool versions/effects | **Supported** by compiler + tests (spot-checked fallback refusal). |
| Park/repair/TTL/restart/resume/exactly-once runtime-owned | **Plausibly supported** by subject-continuation design + named public tests; full suite not re-run here. |
| Semantic-only flows fail closed + demand | **Supported** by design + named tests. |
| Same-version drift rejected; prior catalog remains | **Claimed with tests**; not re-run here → keep as P1-4 evidence item. |
| Failed Rust catalog admission cannot replace active SDK host | **Claimed**; verify in sdk admission ordering tests during full gate. |
| Legacy flow-instance not hydrated/mutated/persisted in unified production | **Supported** when legacy disabled; constructor footgun remains (P1-2). |
| Compiler escalation for unavailable pinned targets | **Plausibly supported**; escalate/free_range yes; fallback correctly refused. |
| Prism/DB/WAL/etc passed | **Not re-verified** in this audit. |
| Full local gate (cargo/pnpm/clippy/fmt) passed | **Not re-verified** here; dirty tree weakens the claim. |
| Spec convergence 28/0/0/1 | **Confirmed** for the JSON ledger. |
| Only external = live provider/load/soak | **Correct framing**. |

---

## Recommended reading order for remediation implementers

1. Mother §18 (pathway cap) + `artifact/views.rs`
2. Mother §4.3 (canonical hash) + `adaptive.rs`
3. Master plan §14.1–§14.4 vs `turn.rs` / `AppState::build`
4. `flow_lowering.rs` + `AUTHORED_FLOW_LOWERING.md`
5. `AELIO_PUBLIC_BOUNDARY_SCENARIO_EVIDENCE.md` vs `aelio.json`
6. Re-run full gate; attach logs to §14.4

---

## Explicit non-actions from this audit

- No code was modified.
- No FLAGS were rewritten.
- No requirements statuses were flipped.
- This file is the remediation backlog only.
