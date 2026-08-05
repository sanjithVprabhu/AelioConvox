# Conductor–Harness OS: Monitoring Notes

**Created:** 2026-08-05  
**Role:** Observer notes while converging on [`Blueprint/conductor-harness-os-production-plan.md`](conductor-harness-os-production-plan.md)  
**Owner intent (product):** Open-source agent OS — Sol is code; harnesses are programs; Conductor is the shell.

---

## 1. Product intent (locked understanding)

How Sanjith wants it to work:

1. **Harness = program.** Readable steps (sugar) that **lower to Sol** and become an **executable**, sealed artifact.
2. **Each harness** has typed I/O (or explicit `unit`), runs a fixed algorithm (loop / branch / call children), returns output or typed error.
3. **Children:** a harness may **spawn other harnesses in series or parallel** (e.g. average → sum ‖ count → divide), then join.
4. **Escape / return:** exit one layer up; stack of processes, not free ChatGPT.
5. **Pages / bag:** step-local and flow-scoped memory (quick context store).
6. **Ops surface:** compute, control, memory, text, **tools**, **prompts/LLM** as admitted `Call` targets — prompts stored in Aelio DB.
7. **Conductor = root shell / controller.** On every event (user message or system push): decide **quick reply | spawn harness | continue | exit | fresh | ignore**, then spin programs. Heavy Conductor prompt when needed, but **decision is validated** against catalog/policy.
8. **Library grows:** preinstalled programs at boot; good tool paths become **new promoted harnesses**; Conductor reuses them next time.
9. **Not** one-shot cold ProposePath forever; **not** hardcoded agent loop as final authority.

One sentence: *stored Conductor selects and runs stored Sol programs; programs compose programs; state survives park/restart; effects stay kernel-gated.*

---

## 2. Plan document alignment

The Blueprint plan matches that intent:

| User language | Plan concept |
|---|---|
| Sol as code | Closed 17 ops + sugar → canonical Sol |
| Harness as executable | Immutable Flow/Harness artifact |
| Conductor as shell | `conductor.root` pinned Sol program |
| Spin series/parallel | `Spawn` / `SpawnMany` + join policies |
| Average example | `workflow.average` = sum ‖ count → divide |
| DB storage | Artifact tables + signed library bundle |
| Tools → new harness | Learning + promote (Phase 7 / backlog #13–14) |
| Rust kernel / TS edge | Authority boundary preserved |

---

## 3. Snapshot: what is already real (2026-08-05)

### 3.1 Assets to keep (plan §3)

- Kernel Sol AST / plan / exec / park / replay / ledger — present.
- Runtime `HarnessBody` composition type — present (thin).
- Agent turn path `/agent/v1/turns` — still the live conversation authority.

### 3.2 Grok / recent kernel work (observe)

| Artifact | Status | Notes |
|---|---|---|
| `aelio-kernel/src/sol_harness_lib.rs` (~2k LOC) | **Present** | Seed Sol contracts: quick_reply, understand, wait, memory, calc, nested stacks, pipelines |
| `store_library` / `load_contract` | **Present** | Store table `sol_harness_contracts` |
| Tests: `exhaustive_harness_system`, `sol_harness_store_replay`, `proper_sol_nested_stack` | **Green** (72+6+7) | Store → compile → wait → replay proven in kernel lab |
| Nested Call via `harness.invoke@1` and proper registered-flow Calls | **Lab-proven** | Not yet the live `/agent/v1/turns` path |

### 3.3 Earlier agent-layer work (this thread)

| Artifact | Status | Notes |
|---|---|---|
| `aelio-agent` `HarnessProgramV1` JSON IR | **Present** | Named ops (`prompt.quick_reply`, …) — **not** canonical Sol |
| Conductor rule selector | **Present** | Keyword ladder, not `harness.select` pipeline |
| Stored vs hardcoded benchmark | **Pass** | Same job for 4 starters |
| Tool path → new harness write | **Absent** | Escalate → ProposePath; no `HarnessProgramV1` writer |
| `aelio-os/library/` filesystem bundle | **Absent** | Plan §7.2 not started |
| `conductor.root` as pinned Sol on live turns | **Absent** | Baseline conversation flow still model+Park |

---

## 4. Critical observation (flag for fix pass)

**Two parallel “harness” representations exist:**

1. **Kernel Sol contracts** (`sol_harness_lib`) — plan-aligned direction (executable Sol, store/replay).
2. **Agent `HarnessProgramV1`** — conversation IR with Rust step dispatcher — useful proof of “data programs,” **but not** Sol-lowering.

**Risk if Grok continues only on (1) while live chat stays on (2):** dual authority, drift, false “done.”

**Required convergence (plan Stage F):** live turns must eventually invoke/resume **tenant-pinned Conductor Sol**, not rule+JSON-op dispatcher as final authority. JSON sugar is allowed **only if it lowers to Sol before admission**.

---

## 5. Gap matrix vs plan DoD (honest)

| DoD / claim | Status |
|---|---|
| Every event enters pinned Conductor harness | **No** — agent turn spine + rules |
| Conductor decisions stored / policy-checked / replayable | **Partial** — traces only; not ConductorActionV1 |
| Nested + parallel durable trees with joins/cancel | **Lab only** — kernel tests; not production scheduler |
| All executable behavior → Sol or admitted Call | **No** — agent IR + ProposePath remain |
| Signed default library install | **No** — Rust source seed only |
| Exact pins for prompts/tools | **Partial** — flows/pins exist; Conductor not pin |
| Parked trees survive restart + library upgrade | **Kernel lab yes / live Conductor no** |
| Effects intent-ledgered + confirmable | **Existing agent/runtime partial** |
| Old agent loop removed as authority | **No** |
| `workflow.average` through harnesses only | **Not verified as production path** |
| Create/promote harness from tool path | **No** (Phase F / learning) |

---

## 6. Monitoring checklist (while Grok writes)

Watch for these **good** signals:

- [ ] Schemas: `HarnessContractV1`, `NormalizedEventV1`, `ConductorActionV1`, `InstanceRecordV1`
- [ ] `aelio-os/library/` layout + bundle builder
- [ ] Migration of `sol_harness_lib` into versioned artifacts (not only a giant `.rs`)
- [ ] Durable spawn/join/cancel + crash tests
- [ ] `conductor.root` deterministic routes
- [ ] Shadow mode vs `/agent/v1/turns`
- [ ] Single authority: Sol path wins; agent JSON IR either lowers or is marked provisional/delete

Watch for these **bad** signals (note + block “done” claims):

- [ ] New hardcoded Conductor rules presented as final product
- [ ] “Harness” that cannot compile through `aelio_kernel::compile`
- [ ] Parallel writes without merge plan
- [ ] LLM spawn of harness id without catalog/policy validation
- [ ] Effectful auto-promote without admin gate
- [ ] Overwriting immutable artifact versions
- [ ] Live chat still only `HarnessProgramV1` while docs claim Sol OS complete

---

## 7. Recommended next actions (after Grok coding pass)

1. **Diff** Grok’s tree against Blueprint §18 backlog items 1–10.
2. **Wire or refuse:** either lower agent starters to Sol and call them from Conductor, or quarantine agent IR as temporary.
3. **E2E acceptance** from plan §13 (greeting, average parallel, park/restart, duplicate webhook, pin across upgrade).
4. **Do not** claim production OSS readiness until §17 DoD checklist is green.

---

## 8. Verdict for Sanjith

**Yes — the structure you described is understood and matches the Blueprint.**

**Today:** kernel **can** store/retrieve/replay Sol harness programs (including nested calls) in the lab; conversation path is still a **thin Conductor (rules) + separate JSON IR / ProposePath**. The **factory growth loop** (tool path → promote → reusable harness) is **not** live.

Monitoring continues against this file. Fixes and full intended-outcome testing start after the current coding pass lands, unless a P0 deviation appears sooner.

---

## 9. Gold-standard lock + full audit (2026-08-05)

Product owner confirmed the dumbed-down model as the gold standard (Sol code · harness programs · Conductor shell · preloaded library · promote growth).

**Full audit:** [`GOLD_STANDARD_HARNESS_OS_AUDIT.md`](GOLD_STANDARD_HARNESS_OS_AUDIT.md)

**Headline:** DoD D1–D3 **FAIL** (live path still Layer B; Conductor still Rust keywords; ~37 harnesses not ≥120). Lab Sol + greeting/tool cutover = progress, not the OS.
