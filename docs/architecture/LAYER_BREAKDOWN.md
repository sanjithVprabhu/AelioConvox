# Aelio layer breakdown (confusion killer)

**Status:** working conversation doc — keep this open with the vision doc  
**Purpose:** name every layer once, say what it does, say what it does *not* do  
**Rule:** if two terms sound the same, they are different until this file says they are the same  
**Opened:** 2026-08-04 — reverse-engineering / Q&A session

Legend for each entry:
- **Is** — one-sentence definition
- **Does** — concrete job
- **Does not** — common confusion
- **Lives in** — code / doc home
- **Talks to** — neighbors

---

## 0. The one picture (lock this)

```text
USER MESSAGE
     │
     ▼
┌────────────────────────────────────────────────────────────┐
│ 1. BRAIN = Turn spine                                      │
│    Hosts the turn. Picks WHICH world runs.                 │
│    File: aelio-agent/src/blocks/turn.rs                    │
└────────────────────────────────────────────────────────────┘
     │
     ├──► 2a. HARNESS WORLD (conversation programs)
     │         Conductor chooses a child program
     │         Child = ordered named steps (HarnessProgramV1)
     │         Steps run inside aelio-agent (not Sol IR yet)
     │
     ├──► 2b. PIN / FLOW WORLD (sealed operational graphs)
     │         Lowered Sol op trees
     │         Executed by aelio-kernel
     │
     └──► 2c. COLD PATH (backup)
               ProposePath over abilities/tools
               Only when Conductor escalates (or triage says so)
     │
     ▼
┌────────────────────────────────────────────────────────────┐
│ 3. STORE                                                     │
│    Sessions, catalogs, traces, memory, WAL / DB              │
└────────────────────────────────────────────────────────────┘
```

**Same philosophy everywhere:** named blocks → assemble into a program → fixed runtime runs it.  
**Two assembly languages today:** Harness steps (conversation) and Sol ops (kernel graphs). Not merged yet.

---

## P1 — Harness bodies must become Sol (NOT DONE)

**Status: INCOMPLETE — must be completed.** Do not treat as finished.

**Locked product rule (2026-08-04, owner session):**

1. Sol is the real operations language (Mother + `aelio-kernel`).
2. Every harness step **must be writable / lowerable to Sol**.
3. Durable law for a harness body should be Sol (or a sealed pin that *is* Sol), not a permanent second language.
4. Until that exists, `HarnessProgramV1` / `HarnessStepV1` is **temporary sugar** (see below).

### What “sugar” means (plain English)

**Sugar** = a nicer / shorter way to write something that the machine should eventually understand as the **real** language.

Example:
- Sugar you type today: `{ "op": "memory.search", ... }`
- Real language we want under it: Sol node(s), e.g. `Call` to a registered Memory ability, inside a `Seq`

So sugar is **not** “fake forever.” It is “friendly front” that **must** map 1:1 (or clearly) into Sol. If it cannot map → gap / FLAG, not a second OS.

### Two ways to finish P1 (choose later; destination same)

| Path | Meaning | Hardness |
|------|---------|----------|
| **A — Soft / dual** | Keep authoring `HarnessStepV1` for humans; **every** step has a defined **lowering → Sol**; store/run the Sol (or pin). Sugar stays as author UX. | Easier migration |
| **B — Harder version** | **Stop authoring harness step JSON as law.** Authors write **Sol only** from now on (or tools that emit Sol only). No parallel `HarnessStepV1` runtime forever. | Harder: rewrite starters, Conductor play path, tests |

**“Author Sol only from now on”** = Path B: new harnesses are written as Sol op trees (like `docs/vectors/*.json`), not as `{ "op": "prompt.quick_reply" }` IR. The agent still has Conductor/stack, but **bodies** are Sol programs the kernel (or a thin host) executes.

**We have not chosen A vs B yet.** We **have** locked: destination = Sol storage/execution for harness bodies; current state = incomplete.

### Current reality (evidence)

| Item | Today |
|------|--------|
| Starters stored as | `HarnessProgramV1` in `harness/program.rs` (+ catalog field) |
| Played by | `play_harness_program` in `turn.rs` (agent), **not** kernel |
| Sol lowering of each step | **Missing** |
| Conductor as Sol program | **Missing** (Rust `select_starter_harness`) |

### Completion checklist (track here + remaining-work gate HS)

- [ ] Inventory every `HarnessStepV1` variant → target Sol fragment / Call id
- [ ] Define lowering table (step → Sol); FLAG any step that cannot lower yet
- [ ] Decide Path A (sugar+lower) vs Path B (Sol-only authoring)
- [ ] Persist harness body as Sol (or sealed pin); content hash per Mother §4.3
- [ ] Play path: run lowered Sol via kernel (or documented interim host)
- [ ] Migrate 4 starters (`quick_reply`, `understand_intent`, `wait_for_user`, `memory_attach`)
- [ ] Conductor selection: keep Rust interim **or** lower Conductor itself to Sol later
- [ ] Update vision §14 / remaining-work when acceptance tests pass
- [ ] Mark this section **DONE** only with linked proof (tests + one live turn trace)

**Until the boxes above are checked, anyone reading the repo must assume: harness ≠ Sol yet.**

---

## P1 stress test — Can Sol express a 12-step harness? (2026-08-04)

**Question:** If sugar is only **names** for Sol instructions, is Sol’s vocabulary strong enough?

**Scenario:** “Login assist” — user wants OTP login; may need memory, clarify phone, send OTP, wait, verify, reply.

**Sugar names (12 steps)** → try each in Sol:

| # | Sugar name (harness-ish) | Sol form | Verdict |
|--:|--------------------------|----------|---------|
| 1 | `sense.session` | `Call` id → Sense/Session ability | **OK if Call registered** |
| 2 | `memory.search` | `Call` → memory/DB query ability | **OK if Call registered** |
| 3 | `context.shorten` | pure compute: `slice` / string ops on bag text **or** `Call` model “summarize” | **OK** (pure or Call) |
| 4 | `context.attach` | bag write via `Let`/`Const`/`merge` **or** `Call` state write | **OK** |
| 5 | `understand.extract_phone` | `Call` Understand.Extract **or** `matches_format` + Branch | **OK if Call/validator** |
| 6 | `branch.phone_missing` | `Branch` + `exists`/`eq` Expr | **OK** (Sol native) |
| 7 | `express.ask_phone` | `Call` Express.Ask / model template | **OK if Call registered** |
| 8 | `park.wait_user` | `Park` `{until:{kind:event}, into:…}` | **OK** (Sol native) |
| 9 | `once.send_otp` | `Once` { body: `Call` send_otp tool, idem_key: … } | **OK** (Sol native + tool Call) |
| 10 | `park.wait_otp` | `Park` again | **OK** |
| 11 | `invoke.verify_otp` | `Call` verify tool | **OK if tool registered** |
| 12 | `express.reply` | `Call` Express.Synthesize / Template | **OK if Call registered** |

**Optional 13–15 (still Sol):**

| # | Sugar | Sol | Verdict |
|--:|-------|-----|---------|
| 13 | `budget.cap` | `Budget` { calls, tokens, ms } | **OK** |
| 14 | `try.tool_errors` | `Try` / `Fallback` | **OK** |
| 15 | `guard.policy` | `Guard` + policy via Call/Policy wrap | **OK pattern** (policy often executor wrap) |

### Reading-aid Sol skeleton (not admitted JSON — shape only)

```text
Seq [
  Call Sense.Session            → sense
  Call Memory.Search            → hits
  // shorten: Let + slice/concat OR Call Summarize
  merge/attach into context page bag paths
  Call Understand.ExtractPhone  → slots
  Branch exists(slots.phone)
    then: Seq [
      Once { Call send_otp },
      Park until event → otp_msg,
      Call verify_otp,
      Call Express.Reply
    ]
    else: Seq [
      Call Express.AskPhone,
      Park until event → phone_msg,
      … (loop back or continue)
    ]
]
```

### Verdict

| Layer | Strong enough? |
|-------|----------------|
| **Sol control ops** (Seq, Branch, Loop, Park, Once, Call, Budget, Try, Guard, Map, Filter, …) | **Yes** — enough to structure this flow |
| **Sol pure compute** (merge, slice, exists, matches_format, …) | **Yes** — enough for attach/shorten/validate *data shape* |
| **Named effects as Call targets** (Memory, Express, tools) | **Yes in principle** — Sol does this via `Call` + registry |
| **Gap today** | Not missing Sol *syntax* — missing **registered Call targets + lowering table** from sugar names → exact `id`s, and stack/Conductor glue outside the Sol tree |

**Conclusion:** Sol vocabulary is **strong enough** for this class of 12–15 step harness.  
Sugar-as-names works. What is incomplete is **wiring** (HS gate), not inventing new Sol control ops for this scenario.

**Open gaps to FLAG if we lower for real:**
- Conductor *selection* itself is not a Sol step yet (router in Rust).
- `return.parent` / stack pop may stay **brain/stack** ops unless we model stack in bag + Calls.
- LLM “quick_reply” needs a pinned model/template Call, not a free prompt string in Sol.

### Essence demo shipped (2026-08-04) — store / retrieve / replay

**Proof (kernel):** `aelio-os/crates/aelio-kernel/tests/sol_harness_store_replay.rs` — **3/3 green**

```bash
cd aelio-os && cargo test -p aelio-kernel --test sol_harness_store_replay
```

| Program | Proves |
|---------|--------|
| `harness.calc_sum_gate` | mock-devise → store → retrieve → run (7+5+3→15, Branch) → replay hash == live → reuse |
| `harness.semantic_ack` | classify + Branch + Park/resume + replay; clear path without park |
| catalog scan | both contracts listed under `sol_harness_contracts` |

Fixtures: `docs/examples/sol_harness_essence/`  

**Still incomplete for product:** Conductor does not load these yet (Gate HS-3/HS-4). Drafter is mock; live LLM = `aelio forge`.

**Nested stack (done in kernel library):** a Sol harness can wait on a child that waits on a grandchild.

1. **Demo sugar:** `Call harness.invoke@1` with runtime `args.id` (`stack_top_a` → `stack_mid_b` → `stack_leaf_c`).
2. **Proper Mother Sol:** pinned registered-flow Calls `flow.stack_*@1` (`proper_sol_stack_*`); acyclic flow graph required (§8.4 / §10). Same wait outcome.

Doc: `docs/examples/sol_harness_essence/NESTED_HARNESS_STACK.md`.  
**Exhaustive case matrix + tests:** `docs/examples/sol_harness_essence/EXHAUSTIVE_SYSTEM_TEST_MATRIX.md` ↔ `cargo test -p aelio-kernel --test exhaustive_harness_system` (72 cases).

---

## 1. Brain (Turn spine)

| | |
|--|--|
| **Is** | The fixed hot loop for one user message (or resume). |
| **Does** | Hydrate session → check waiting child / Park resume → triage installed flows → maybe Conductor → maybe ProposePath → produce reply / Park. |
| **Does not** | Author new harnesses. Is not “the kernel.” Is not Conductor itself. |
| **Lives in** | `aelio-os/crates/aelio-agent/src/blocks/turn.rs` |
| **Talks to** | Harness session, tenant catalog, adaptive pin host, provider/LLM, store |

**Remember:** Brain = host. Conductor = one decision inside the brain when chatty path is chosen.

---

## 2. Conductor

| | |
|--|--|
| **Is** | Top harness / stack ceiling / router for starter conversation programs. |
| **Does** | If stack empty, sit on top. Look at utterance. Pick: `quick_reply` \| `understand_intent` \| `wait_for_user` \| `memory_attach` \| `escalate`. |
| **Does not** | Have a saved `HarnessProgramV1` step list today. Does not run Sol. Does not call tools itself. |
| **Lives in** | `aelio-agent/src/harness/conductor.rs` (`select_starter_harness`) + call site in `turn.rs` (`run_conductor_starters`) |
| **Talks to** | `HarnessSession`, starter library / play mode |

**Today’s Conductor “program”** = Rust if/else decision table, not JSON steps.

---

## 3. Harness (playable program)

| | |
|--|--|
| **Is** | A named conversation program: id + description + ordered steps + exits. |
| **Does** | When selected, steps run in order until finish / parent / ask_and_wait (Park). |
| **Does not** | Equal a Sol flow. Equal the kernel. Equal Conductor (Conductor is special). |
| **Lives in** | Type: `HarnessProgramV1` in `harness/program.rs`. Storage intent: `TenantDecl.harness_programs`. |
| **Talks to** | Turn dispatcher `play_harness_program` |

**Starter saved programs (steps exist):**
- `quick_reply`
- `understand_intent`
- `wait_for_user`
- `memory_attach`

**Not saved as steps:** `conductor`, `escalate`

---

## 4. Harness step / building block (conversation ops)

| | |
|--|--|
| **Is** | One named op inside a harness (`HarnessStepV1`). |
| **Does** | One bounded action: search memory, attach context, quick prompt, ask+wait, return. |
| **Does not** | Mean Sol `Kind::Call` / `Kind::Park` unless later lowered. |
| **Lives in** | `harness/program.rs` enum `HarnessStepV1` |
| **Talks to** | Memory, LLM provider, harness session pages |

**v0 ops:** `prompt.quick_reply` · `prompt.understand_intent` · `memory.search` · `context.attach` · `ask_and_wait` · `return.finish` · `return.parent`

---

## 5. Stack / waiting child / context page

| Term | Is | Does |
|------|-----|------|
| **Stack** | Ordered frames of harness ids | Depth = mid-flight children |
| **Frame** | One stack entry | May be `waiting` |
| **Waiting child** | Frame that owns next user msg | Inline resume; Conductor does not re-pick first |
| **Exit up** | Pop one layer | Ceiling = Conductor |
| **Fresh** | Clear stack | Restart at Conductor |
| **Context page** | Durable notes per harness id | Feeds next decisions |

**Lives in:** `harness/mod.rs` (`HarnessSession`, `StackControl`, `ContextPage`)

---

## 6. Sol / fundamental building blocks (Mother)

| | |
|--|--|
| **Is** | The normative op language: Seq, Loop, Park, Once, Call, Const, compute, … |
| **Does** | Express sealed operational graphs with bag + ledger + replay rules. |
| **Does not** | Equal harness step IR today. |
| **Lives in** | Spec: `AELIO_DSL_MOTHER.md`. Code: `aelio-os/crates/aelio-sol`, `aelio-kernel` |
| **Talks to** | Planner → Executor → store trait |

**This is the original “assemble blocks → get code” substrate.**

---

## 7. Kernel (`aelio-kernel`)

| | |
|--|--|
| **Is** | Executor of validated Sol op trees. |
| **Does** | Walk the tree; enforce Park/Once/Loop; write ledger; bag updates; replay. |
| **Does not** | Choose Conductor children. Host HTTP. Author harnesses. |
| **Lives in** | `aelio-os/crates/aelio-kernel` |
| **Talks to** | Sol values, store, drivers for Call targets |

---

## 8. Lowering / pin / adaptive artifact

| Term | Is | Does |
|------|-----|------|
| **Flow (semantic)** | Product description of a journey | May be guidance only |
| **Lowering** | How that journey becomes a closed Sol graph + pins | Makes it executable |
| **Pin / artifact** | Sealed admitted runnable | Turn can Invoke it |
| **Adaptive host** | Runtime that loads/runs sealed artifacts | Resume subjects, decision envelopes |

**Without lowering:** visible, not executable as an installed app.  
**With lowering:** brain can hand work to kernel world.

**Docs:** `docs/operations/AUTHORED_FLOW_LOWERING.md`

---

## 9. Abilities / tools / ProposePath

| Term | Is | Does |
|------|-----|------|
| **Ability** | Typed Call target (L1) | Sense, Express, Invoke, … |
| **Tool** | Tenant-registered effectful capability | OTP, jobs, … |
| **ProposePath** | LLM proposes an ability path | Cold backup when no harness/pin fits |
| **Escalate** | Conductor choice | Fall through to ProposePath |

**Does not:** mean harness steps. Escalate is how harness world admits “I don’t have a program for this.”

---

## 10. Store / catalog / session

| Term | Is |
|------|-----|
| **Tenant catalog** | Tools, flows, prompts, harness_programs, … |
| **Harness session** | Stack + context pages for a user/session |
| **Traces** | What ran (`Conductor.Select`, `Harness.Load`, `Harness.Op`, …) |
| **WAL / DB** | Durability for agent state |

---

## 11. What we have probably “made messy”

Honest list from this reverse-engineering pass:

1. **Two program IRs** (Harness steps vs Sol) without a single sentence that says “not unified yet.”
2. **Conductor named like a harness** but implemented as Rust router, not a step program.
3. **Vision says programs persist**; live path often seeds in memory only (see `HARNESS_REMAINING_WORK.md` H1).
4. **“Brain” used casually** for kernel, turn spine, and Conductor — three different things.
5. **170-atom vocabulary doc** is a third catalog; not the v0 harness IR.

This file exists so we stop collapsing (1)–(4).

---

## 12. Conversation protocol (how we continue)

We go **one object at a time**. For each:

1. Read its section above.
2. You restate in your own words (1–3 sentences).
3. I correct only if wrong.
4. Then we move to the next.

**Order for the session:**

| Round | Object |
|------:|--------|
| A | Brain (turn spine) |
| B | Conductor |
| C | One child harness (`quick_reply`) |
| D | Stack + waiting child |
| E | Sol blocks |
| F | Kernel |
| G | Lowering / pin |
| H | Escalate → ProposePath |
| I | How they should eventually unify (design, not code yet) |

Do **not** jump to code changes until Round I is agreed — unless fixing H0 build blocker.

---

## 13. Session log

| When | Note |
|------|------|
| 2026-08-04 | Doc created. User asked to explain each piece and enter write/reason mode to kill confusion. |
| 2026-08-04 | Locked P1: harness steps must be Sol-capable; storage/execution as Sol is **incomplete**. Explained sugar vs Path A/B (harder = Sol-only authoring). |
| 2026-08-04 | Stress test: 12–15 step login-assist flow → Sol control vocabulary **strong enough**; gap is Call registry + lowering, not new Sol ops. Sugar-as-names = Path A. |

*(Append short notes after each round.)*
