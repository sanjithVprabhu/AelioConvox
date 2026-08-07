# Conductor–Harness OS — code map (who owns what)

**Purpose:** Fix-oriented map of live code. Not a status report.  
**Gold model:** Sol = code · harness = program · Conductor = shell · kernel = runner · DB = store.  
**Phase memory:** 0–2 done · next 3 ‖ early 6 · critical 4 → 5 · then finish 6 → 7.

---

## 1. One picture: what happens on a message

```
Widget / SDK / channel
        │
        ▼
server/src/conversation-turn.ts          ← TS ingress, builds agent turn request
        │
        ▼
POST /agent/v1/turns
aelio-agent-api/src/lib.rs::process_turn  ← HTTP edge into Rust
        │
        ├── (A) greeting cutover?     cutover.rs → conductor.root Sol (narrow)
        ├── (B) tool harness installed? tool_workflows.rs → run Sol contract
        └── (C) else agent spine
                aelio-agent/.../turn.rs
                  ├── waiting child / stack     harness/mod.rs (HarnessSession)
                  ├── Conductor keywords       harness/conductor.rs
                  ├── play HarnessStepV1       harness/program.rs   ← Layer B (to delete in Ph5)
                  └── escalate → ProposePath   cold LLM path
```

**Rule of thumb while fixing:**  
- Changing **how Sol programs run** → `aelio-kernel`  
- Changing **what most chat still does today** → `aelio-agent` turn + harness  
- Changing **HTTP / cutover order** → `aelio-agent-api`  
- Changing **channels / LLM host / tools host** → `server/` (TS)

---

## 2. Crates (responsibility)

| Crate | Job | Touch when… |
|-------|-----|-------------|
| `aelio-sol` | Values, paths, canonical hash (BLAKE3) | Value/hash/path bugs |
| `aelio-kernel` | Plan + exec + ledger + Sol library + OS helpers | Sol ops, Call ISA, seed library, Conductor Sol, promote |
| `aelio-store` | Store trait / DB backend | Persistence tables |
| `aelio-agent` | Conversation brain: abilities, turn loop, Layer-B harness | Live chat behavior today |
| `aelio-agent-api` | Axum routes `/v1/turns`, cutover wiring | API order, OS vs spine switch |
| `aelio-runtime` | Artifact/flow admission, actors | Pins, flows, sandbox (installed artifacts) |
| `server/` (TS) | Channels, widget, `aelio-host` LLM/tool proxy | Edge I/O, not Sol semantics |

---

## 3. Kernel file map (`aelio-kernel/src/`)

### Mother core (don’t casually reinvent)

| File | Owns |
|------|------|
| `instr.rs` | Closed 17 Sol ops AST |
| `plan.rs` | Static checks before run |
| `exec.rs` | Walker / frames |
| `driver.rs` | Instance start / Park / resume / turn outcomes |
| `continuation.rs` | Park image |
| `ledger.rs` | Hash-chained journal |
| `bag.rs` | Working memory bag |
| `registry.rs` | Call target registry + effects |
| `compute.rs` | T2 expr fns (`add`, `mul`, …) |
| `sugar.rs` | Authoring sugar → Sol (if used) |

### Phase 0–2 OS layer (done foundations)

| File | Owns | Fix here for… |
|------|------|----------------|
| `harness_contract.rs` | Phase 1: signature, budget, BLAKE3 `contract_identity`, admit | Identity / admission of a contract |
| `call_isa.rs` | Phase 2: frozen Call ids (`harness.*`, `llm.*`, `memory.*`, `tool.*`) | Adding/changing system calls |
| `stdlib_targets.rs` | Pure stdlib Calls (`math.*`, `collection.*`) | Pure compute targets |
| `sol_harness_lib.rs` | Seed Sol programs + store/load `sol_harness_contracts` | Adding starter/library harnesses (content) |
| `library_bundle.rs` | Install seed/manifest into store | Boot install / signed bundle |

### Conductor / process / growth (partial)

| File | Owns | Fix here for… |
|------|------|----------------|
| `harness_syscalls.rs` | `decide_deterministic`, `conductor_root_program_json`, `run_pure_sol`, average via process tree | Keyword Conductor + thin Sol router; average join demo |
| `cutover.rs` | Which utterances skip agent spine for greets | Expanding/narrowing greeting OS authority |
| `event_admission.rs` | Event dedupe + optional conductor admit | Event identity / duplicate webhooks |
| `process_tree.rs` | Parent/child instance tree, spawn/join | Structured concurrency |
| `process_store.rs` | Persist instance/join/wait/budget tables | Crash recovery of trees |
| `tree_replay.rs` | Replay recorded tree runs | Determinism of compound workflows |
| `tool_workflows.rs` | Map utterance → installed tool harness (`send_otp`, memory, …) | Tool cutover without ProposePath |
| `otp_journey.rs` | OTP park/resume helper journey | Login OTP multi-turn |
| `promote.rs` | Draft → sandbox → admin promote | Growth loop (Phase 7) |
| `shadow.rs` | Compare agent vs OS routes | Shadow metrics |
| `os_contract.rs` | Shared V1 types (events, pins, envelopes) | Wire schemas |

---

## 4. Agent Layer B (temporary — Phase 5 deletes)

`aelio-agent/src/harness/`

| File | Owns |
|------|------|
| `mod.rs` | `HarnessSession` stack, frames, context pages, ExitUp/Fresh |
| `conductor.rs` | `select_starter_harness` — **Rust keyword ladder** (live Conductor today) |
| `program.rs` | `HarnessProgramV1` / `HarnessStepV1` JSON IR + starter library |

`aelio-agent/src/blocks/turn.rs`

| Region | Owns |
|--------|------|
| Main turn loop / tiers | Memory recall, flow lookup, ability path, **Tier3 Conductor** |
| `run_conductor_starters` | Calls `select_starter_harness` → load program → `play_harness_program` |
| `play_harness_program` | Interprets Layer-B steps (prompt / memory / wait / return) |
| Escalate branch | Cold `ProposePath` over declared abilities |

**If chat “does the wrong thing” today, start in `turn.rs` + `harness/conductor.rs`, not in `sol_harness_lib.rs`.**  
Sol lib changes won’t show up in most turns until Phase 4 play path lands.

---

## 5. API edge (`aelio-agent-api`)

`src/lib.rs` — `process_turn` roughly:

1. Ensure vendor library installed (`library_bundle`)
2. If **not** greeting cutover → try `try_run_tool_intent`
3. If greeting cutover → `run_greeting_cutover` → return (no agent spine)
4. Else again tool cutover branch in some paths
5. Else hand to agent `TurnRuntime` (Layer B)

Also: `/v1/events`-style admission via `admit_event`.

**Fix cutover order / “why did OS vs agent win?” here.**

---

## 6. Two “Conductors” (don’t confuse them)

| Name in code | Where | What it actually does |
|--------------|-------|------------------------|
| Agent Conductor | `aelio-agent/.../conductor.rs` | Keywords → starter id → play Layer B |
| OS deterministic Conductor | `harness_syscalls.rs` `decide_deterministic` | Keywords → `route` string |
| `conductor.root` Sol | `conductor_root_program_json()` | Branch on **precomputed** `route`; mostly says a reply / “Spawning …” |
| Gold Conductor (Phase 4) | not done | Stored Sol that `list`/`describe`/`invoke` real children |

When you “fix Conductor,” say which one.

---

## 7. Storage tables (mental filesystem)

| Table / key idea | Module | Contents |
|------------------|--------|----------|
| `sol_harness_contracts` | `sol_harness_lib` | Sol program bodies (gold library home) |
| Draft / promote audit | `promote` | Growth pipeline |
| Event / dedupe | `event_admission` | Incoming events |
| Instance / join / wait / budget | `process_store` | Process tree durability |
| Agent `harness_sessions` | agent durable path | Stack + pages (Layer B control plane) |
| `TenantDecl.harness_programs` | agent catalog | Old Layer-B program map (to retire in Ph5) |

**Phase 5 goal:** one library home (`sol_harness_contracts`), delete dual IR.

---

## 8. “I want to fix X” cheat sheet

| Symptom / goal | First files |
|----------------|-------------|
| Wrong harness picked for a phrase | `harness/conductor.rs` **and/or** `harness_syscalls.rs::decide_deterministic` |
| Greets still go through agent | `cutover.rs` + `agent-api` cutover branch |
| Sol program wrong math/logic | that contract in `sol_harness_lib.rs` + `compute` / `stdlib_targets` |
| Nested child doesn’t wait | `sol_harness_lib` invoke registry + `harness_syscalls` / `call_isa` |
| Average workflow | `workflow_average_*` in sol lib + `harness_syscalls::workflow_average_via_join` |
| Tool/OTP path | `tool_workflows.rs`, `otp_journey.rs` |
| Can’t promote new harness | `promote.rs` (+ later admin API) |
| Park/resume broken | `driver.rs`, `continuation.rs`, agent turn suspend path |
| Replay mismatch | `ledger.rs`, `driver::replay`, `tree_replay.rs` |
| New Call id needed | `call_isa.rs` (frozen — amend deliberately) |
| Contract hash / admit reject | `harness_contract.rs` |
| Widget/channel only broken | `server/src/conversation-turn.ts`, widget routes, `aelio-host.ts` |
| Delete Layer B / Sol play path | Phase 4: replace `play_harness_program` in `turn.rs` with kernel load→compile→exec |

---

## 9. Phase → code ownership

| Phase | Status | Primary code |
|-------|--------|--------------|
| 0 baseline | Done | build, scan limits, commit slice |
| 1 contract | Done | `harness_contract.rs` |
| 2 Call ISA | Done | `call_isa.rs`, `stdlib_targets.rs` |
| **3 library** | Next content | `sol_harness_lib.rs` + vectors under `docs/` |
| **4 Conductor Sol** | Critical | replace play path in `turn.rs`; real `conductor.root`; shrink keyword ladders to fast-path only |
| **5 one store / delete B** | Critical | kill `HarnessStepV1` / `play_harness_program`; single `sol_harness_contracts` |
| 6 harden | Parallel | budgets, soak, proxy wiring, replay coverage |
| 7 promote | Last | `promote.rs` + live draft writer from cold paths |

---

## 10. Safe mental model for edits

1. **Content (new programs)** → seed Sol in kernel library (Phase 3). Won’t drive chat until Phase 4.  
2. **Live chat behavior today** → agent conductor + turn + optional API cutover.  
3. **Don’t grow Layer B** — Path B decision: Sol-only bodies; sugar dies at Phase 5.  
4. **Don’t add Call ids casually** — ISA is frozen in `call_isa.rs`.  
5. **Two systems until 4/5** — expect dual behavior; shadow/cutover exist to bridge, not to stay forever.
