# Gold-standard audit: Conductor–Harness OS

**Date:** 2026-08-05  
**Verdict:** **NOT production-ready as the OS you described.** Lab Sol library + partial cutover exist; live conversation authority is still mostly the agent spine.  
**Gold standard locked by product owner:** Sol = code · harness = saved executable program · Conductor = shell · preloaded multi-harness library from DB · children compose · promote grows the library · cold ProposePath is backup only.

**Authority stack for this audit:**  
`AELIO_DSL_MOTHER.md` > `HARNESS_CONDUCTOR_VISION.md` > `HARNESS_OS_EXECUTION_PLAN.md` DoD D1–D8 > Blueprint production plan > code.

---

## 0. Gold standard (one paragraph)

A message arrives. **Conductor** — itself a **stored Sol program** loaded from Aelio DB — decides: quick reply, spawn a named harness, exit one layer, or fresh. Spawning loads another Sol contract from DB, compiles it, runs it on the kernel. That program may compute, branch, loop, use pages, call tools via admitted proxies, invoke child harnesses, and return typed output/error or Park. Boot installs a large preloaded library. Successful cold paths can be **promoted** into new harnesses. Sugar may author; **only Sol is stored and executed**.

---

## 1. DoD scorecard (execution plan §0)

| ID | Criterion | Status | Evidence |
|----|-----------|--------|----------|
| **D1** | No `HarnessStepV1` on production turn path — Sol only | **FAIL** | `turn.rs` still calls `select_starter_harness` → `play_harness_program` (`HarnessStepV1`). Only a **greeting/ack cutover** and some **tool-intent** shortcuts bypass that. |
| **D2** | Conductor selection is a stored program, not Rust `match` | **FAIL** | Selection is `decide_deterministic` / `select_starter_harness` (keyword ladders in Rust). `conductor.root` Sol body only **switches on a precomputed `route` string** — it does not select from the catalog. |
| **D3** | ≥120 harnesses installed from DB at boot, with vectors | **FAIL** | Seed library ≈ **37** Sol contracts in `sol_harness_library()`. Example JSON library ~37. Target catalog in `PREINSTALLED_HARNESS_EXHAUSTIVE.md` still has Missing/Rust-only gaps. No `docs/vectors/harness_*` suite at scale. |
| **D4** | Every effectful Call via runtime-owned proxy | **PARTIAL** | Kernel registry + agent host exist; `tool.invoke@1` as the frozen ISA target is not the universal production path. Agent tools still escalate to ProposePath often. |
| **D5** | Replay: execute → ledger → replay → same `bag_hash` for every library harness | **PARTIAL** | Kernel lab tests green for many contracts (`sol_harness_store_replay`, `exhaustive_harness_system`). Not proven for live Conductor-driven turns / full ≥120 set. |
| **D6** | Mother §4.3 identity + audited promotion | **PARTIAL** | `promote.rs` has draft → sandbox → `admin_promote` (good shape). `SolHarnessContract` is still bare `{id, summary, program_json}` — **no version/library/signature/effect/budget/BLAKE3 identity** on the seed type (Phase 1 incomplete). Agent IR still uses weaker hashing. |
| **D7** | Budget / timeout / depth at admission | **PARTIAL** | Kernel has Budget/Timeout ops and some demo contracts; not enforced as universal admission for every installed harness. |
| **D8** | `cargo test --workspace` green `-D warnings` | **UNVERIFIED this pass** | Focused kernel harness suites previously green; full workspace not re-run as part of this audit write-up. |

**Score:** 0/8 fully met · ~4/8 partial · hard fails on D1–D3 (the product spine).

---

## 2. Architecture fidelity (your model vs code)

| Your requirement | Required shape | Today | Gap |
|------------------|----------------|-------|-----|
| Sol is the code | Closed ops + Calls | Kernel Sol works | Live turns mostly don’t play it |
| Harness = program | Named Sol in DB | 37 seed Sol contracts + store helpers | Not the live body language for chat |
| Conductor = shell | Stored program picks harnesses | Rust keywords + thin Sol router | Installing a harness does **not** auto-make it selectable |
| Preloaded many programs | Boot install from DB | Rust-embedded seed; bundle helpers exist | Not ≥120; Conductor still Rust-only for T0 ceiling |
| Series / “parallel” children | Deterministic invoke_seq / spawn+join | `harness.invoke@1`, `workflow_average_via_join` lab | Not Conductor-driven on live turns |
| Pages / bag memory | Scoped working memory | Kernel bag + agent context pages | Dual models; not unified under Sol Conductor |
| Prompts in DB | Versioned prompt artifacts | Mostly inline / agent synthesis | Gate H4 open |
| Tools as programs | Installed tool harnesses | Partial tool cutover + promote prototype | No factory growth on live path |
| Cold path is backup | ProposePath only when no fit | Still primary for many intents | Escalate → ProposePath common |

---

## 3. Dual-system risk (highest priority finding)

Two harness implementations still exist. **Most user turns still run the weaker one.**

| | Layer A — Sol (gold path) | Layer B — `HarnessStepV1` (legacy) |
|--|---------------------------|-------------------------------------|
| Home | `sol_harness_lib.rs`, kernel tests | `aelio-agent/src/harness/program.rs` |
| Ops | Full Sol + compute + nested invoke | ~7 sugar ops |
| Runs real `/agent/v1/turns`? | **Only cutovers** (greet/ack; some tool intents) | **Yes — default spine** |
| Plan decision | Path B: Sol-only bodies; delete Layer B | Still default |

Until D1 is true, claims like “we have a Sol harness OS” are **lab-true, product-false**.

---

## 4. Conductor honesty check

What exists that looks like Conductor:

1. **Agent** `select_starter_harness` — keyword ladder → play `HarnessProgramV1`.
2. **Kernel** `decide_deterministic` — similar keyword ladder → feeds `route` into Sol.
3. **`conductor.root` Sol** — Branch on `route`; mostly `express.say` acknowledgements (“Spawning workflow.average”), **not** actually loading/running child Sol contracts for those routes in the cutover path shown.
4. **API cutover** — pure greets/acks return from `run_greeting_cutover` without agent spine.
5. **Tool intent cutover** — if installed harness matches, run it; else fall through.

**What gold standard needs:** Conductor Sol that calls `harness.list` / `describe` / `invoke` (plus optional model classify), so **new DB installs become selectable without Rust edits**.

**Today’s failure mode:** new Sol contracts in the seed library are invisible to Conductor selection unless someone adds another `if t.contains(...)`.

---

## 5. Preloaded library audit

| Metric | Target | Observed |
|--------|--------|----------|
| Installed Sol contracts in seed | ≥120 | **37** |
| Example JSON mirror | all seed | ~37 files under `docs/examples/sol_harness_essence/library/` |
| `workflow.average` | present | **Present** in seed |
| T0 `conductor` as Sol body that selects | MUST | **Rust only** (doc + code agree) |
| T0 `escalate` as Sol | MUST | **Rust enum**, not Sol |
| Production identity fields | version, library, signature, effect, budget, BLAKE3 | **Missing** on `SolHarnessContract` |
| Boot from durable DB as authority | yes | Seed often still code-path; H1 persistence checklist still open |

Preload direction is correct. **Quantity and Conductor wiring are not.**

---

## 6. What is genuinely good (keep)

- Kernel Sol compile / park / replay / nested `harness.invoke@1` — proven in tests.
- Seed + store/load contract table pattern (`sol_harness_contracts`).
- `workflow.average` as compose-of-children example (lab).
- Process-tree spawn/join helpers (deterministic).
- Promote pipeline sketch: draft → sandbox → admin promote + audit table.
- Greeting cutover proves API *can* hand authority to OS Sol for a narrow class.
- Execution plan correctly chooses **Path B (Sol-only)** and forbids true parallelism without amendment.

---

## 7. What is not the gold standard (do not ship as “done”)

- Keyword Conductor presented as the OS shell.
- `HarnessStepV1` interpreter as production body language.
- “conductor.root ran” when selection happened in Rust and Sol only said hi / “Spawning …”.
- Claiming ≥120 / full catalog when seed is ~37.
- Dual catalogs (`TenantDecl.harness_programs` vs `sol_harness_contracts`) drifting.
- Auto-growth / create-harness without admin promote on the live turn path (H5 still open).

---

## 8. Ordered fix path to gold (matches execution plan)

1. **Phase 0** — build green; raise `list_contract_ids` scan limit; stop silent truncation.
2. **Phase 1** — harden `SolHarnessContract` (version, library, signature, effect, budget, BLAKE3 identity).
3. **Phase 2** — freeze Call ISA (`spawn`, `exit_up`, `fresh`, `list`, `describe`, `invoke_seq`, real `tool.invoke`, `llm.*`, memory/page).
4. **Phase 3** — grow library toward ≥120 with vectors (builder DSL, not hand JSON thrash).
5. **Phase 4** — replace `play_harness_program` with kernel Sol play; Conductor becomes real stored selector (rules only as fast-path front).
6. **Phase 5** — one storage path; seed-before-persist; admin API.
7. **Phase 6+** — delete Layer B; promotion factory; prompts as artifacts.

---

## 9. Monitoring watchlist (ongoing)

Flag immediately if Grok/agents:

- add more Rust keyword routes and call that “Conductor complete”
- expand `HarnessStepV1` instead of lowering/deleting it
- grow `sol_harness_lib.rs` without wiring Conductor selection to `harness.list`
- claim D1–D3 without `/agent/v1/turns` evidence
- promote without sandbox/admin gate

---

## 10. Bottom line

**You and the docs agree on the gold standard.**  
**The codebase is early mid-build toward it:** Sol programs exist and can run in the lab; a thin cutover proves the door; **the house is not yet the OS.**

Until D1–D3 pass, the honest status line is:

> *Sol harness library prototype + partial OS cutover; agent Layer-B Conductor still owns most turns.*
