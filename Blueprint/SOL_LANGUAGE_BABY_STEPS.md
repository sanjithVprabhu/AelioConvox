# Sol language — baby step 0 (learn before Conductor)

**Purpose:** Understand Sol as the instruction language. Write small programs.  
**Authority:** `AELIO_DSL_MOTHER.md` §8 (17 control ops) + App E JSON shape.  
**Code mirror:** `aelio-os/crates/aelio-kernel/src/instr.rs`

---

## 1. What Sol is (one breath)

Sol is a **closed** language. You do not invent new keywords.

- **17 control ops** — structure of the program (if, loop, call, park, …)
- **Exprs** — read bag / literal / pure compute (`add`, `concat`, …)
- **Call targets** — registered “system calls” (`express.say@1`, `harness.invoke@1`, …)

A harness body **is** a Sol program (JSON tree of ops).

Working memory = the **bag** (a map of paths → values).

---

## 2. The 17 control ops (cheat sheet)

| Op | Plain English |
|----|----------------|
| `Const` | Replace whole bag with a fixed value (seed / fixture) |
| `Identity` | No-op (pass bag through) |
| `Seq` | Do steps in order |
| `Let` | Bind local names from exprs, then run body |
| `Branch` | if pred then … else … |
| `Loop` | while pred, body, **max_iter** required |
| `Try` | try / catch by reason / finally |
| `Fallback` | try alternatives until one works |
| `Guard` | run body under an invariant |
| `Budget` | cap calls / tokens / ms around body |
| `Timeout` | deadline around body |
| `Once` | at-most-once effectful body (idempotency) |
| `Park` | suspend until event/ttl (human wait — not Loop) |
| `Tee` | main body + side branch (sequential in practice) |
| `Map` | for each list element, run body, collect |
| `Filter` | keep list elements matching pred |
| `Call` | invoke registered target with **args** → write **into** path |

Forbidden by design: Sleep; Loop-as-human-wait (use Park).

---

## 3. Expressions (how you read/compute)

Inside args / predicates:

```json
{ "lit": 5 }                 // literal
{ "pull": "sum" }            // read bag path
{ "fn": "add", "args": [ { "pull": "a" }, { "lit": 1 } ] }
{ "fn": "gt",  "args": [ { "pull": "sum" }, { "lit": 10 } ] }
{ "fn": "concat", "args": [ { "lit": "Hi " }, { "pull": "name" } ] }
```

Pure math/text lives in **fn** (Compute). Side effects live in **Call** targets.

---

## 4. Every node needs `nid`

```json
{ "nid": "add_ab", "op": "Call", ... }
```

`nid` is stable identity for the step (tracing, pins, replay). Mint once; don’t recycle casually.

---

## 5. Call shape (most important “command”)

```json
{
  "nid": "say_hi",
  "op": "Call",
  "id": "express.say@1",
  "args": {
    "text": { "lit": "Hey!" }
  },
  "into": "reply"
}
```

Rules of thumb:

- `id` = registered target (versioned, e.g. `@1`)
- `args` = **only** what the callee sees (not the whole bag)
- `into` = where result lands in **your** bag (mandatory; not root)

---

## 6. Tiny programs to read / copy

### A. Say hello (Seq + Call)

See `docs/examples/sol_harness_essence/library/quick_reply.json`:

```text
Seq[
  Call express.say@1(
    text = concat("Quick reply: ", pull utterance)
  ) → into out
]
```

### B. Add then branch

See `calc_sum_gate.json`:

```text
Seq[
  Const { a:7, b:5, c:3 }          // seed bag
  Call compute.hold(add(a,b)) → partial
  Call compute.hold(add(partial,c)) → sum
  Branch sum > 10
    then say "sum is large"
    else say "sum is small"
]
```

That’s Sol: **seed → transform → decide → emit**.

---

## 7. How a program actually runs

1. JSON → parse (`instr`) → plan checks (`plan`)  
2. `Instance::start(bag)` walks the tree (`exec` / `driver`)  
3. Writes go into the bag; Calls hit the registry  
4. End: Completed (bag + bag_hash) or Parked (continuation)

Try compile in tests/CLI: `aelio_kernel::compile(json_text)`.

---

## 8. Mental model vs Conductor

| Layer | What |
|-------|------|
| Sol ops | Language keywords (`Seq`, `Branch`, `Call`) |
| Call targets | System calls (`express.say`, later `llm.classify`) |
| Harness | Named Sol program stored in DB |
| Conductor | One special harness — **later** |

Don’t invent Conductor steps in Rust. Write them as Sol once we know the language.

---

## 11. Deeper: how the machine actually runs Sol

### 11.1 Three different “languages” people mix up

| Thing | Is it Sol? | Who runs it |
|-------|------------|-------------|
| JSON with `"op": "Seq"` / `"Call"` | **Yes — Sol** | Kernel parser → executor |
| English in a chat message | No | Never parsed as Sol. May become bag field `utterance` |
| LLM output | No | Only usable if a **Call** target validates it into bag data |

The kernel does not “read” English. It only walks a Sol tree.

### 11.2 Phase A — Parse (shape)

Input: JSON text.  
Module: `instr.rs` → `parse_node`.

- Every node must have `nid` + known `op`.
- Unknown op / unknown field → **reject** (closed schema).
- Output: Rust AST `Node { nid, kind: Kind::Seq(...) | Call(...) | ... }`.

This is like a compiler frontend: text → tree. No execution yet.

### 11.3 Phase B — Plan (static legality)

Module: `plan.rs` → `plan(&node)`.

Before any bag exists, reject programs that break Mother rules, e.g.:

- bad Call shapes (missing `into`, etc.)
- illegal writes under `sense` (runtime-owned)
- other §29 static checks

`compile(json)` = parse + plan. Passing compile ⇒ “this tree is allowed to run.”

### 11.4 Phase C — Instance (the process)

Module: `driver.rs` → `Instance`.

Think of one **process**:

- owns the **program tree** (immutable for this run)
- owns the **bag** (mutable memory)
- owns the **ledger** (journal of what happened)
- owns a handle to the **registry** (Call implementations)
- can **start**, **park**, **resume** across turns

`Instance::start(initial_bag)` begins a turn.  
`Instance::resume(parked, wake)` continues after Park.

### 11.5 Phase D — Executor (the CPU)

Module: `exec.rs` → `Executor`.

One recursive walker: `exec(node, cursor)`.

- **bag** = current memory
- **cursor / frame stack** = “where am I if I suspend?”

Not a FIFO job queue. It’s a **call stack of control frames**, like a language VM.

### 11.6 How `Seq` really works (your example)

Program:

```text
Seq [ Const{x,y}, Call add→sum, Branch … ]
```

Executor hits `Seq`:

1. No saved frame → start at index `0`
2. Run child 0 (`Const`) to completion → bag becomes `{x:2,y:3}`
3. Run child 1 (`Call`) → registry runs target → writes `sum`
4. Run child 2 (`Branch`) → eval pred → run then or else
5. Seq done → Completed

If child 1 were `Park` instead:

- Executor returns `Suspend`
- Pushes `Frame::Seq(1)` meaning “resume this Seq at step index 1”
- Driver saves bag + frames to store
- Next turn: restore frames, continue from that index (don’t re-run Const)

That’s the “queue”: **an index on a Seq frame**, nested if you’re inside Branch/Loop/etc.

### 11.7 How `Branch` works

1. Evaluate `pred` expr against bag → bool (unless resuming with saved `Frame::Branch(arm)`)
2. Run `then` or `else` subtree (missing else = Identity / no-op)
3. If that subtree Parks → save which arm you chose so resume doesn’t re-flip the coin

### 11.8 How `Call` works (system call)

1. Evaluate each `args` expr (lit / pull / fn) in the **caller** bag
2. Build a small args map — **callee does not see the whole bag** (least privilege)
3. Look up `id` in **Registry** (e.g. `express.say@1`)
4. Live: run the target function → result  
   Replay: **do not re-run**; take result from ledger inject
5. Write result to caller bag at `into` path

So “say ok” is not a Sol keyword — it’s a **registered Call**. Sol only says *when* to call it and *where* to put the result.

### 11.9 Exprs (the calculator inside)

`{ "fn": "add", "args": [ {"pull":"x"}, {"pull":"y"} ] }`  
is evaluated by the executor’s pure compute path — no registry, no effects.  
That’s why add can appear inside Call args or Branch preds.

### 11.10 Park / resume (multi-turn)

```text
… → Park(until: Event) → turn ends
user bag + frames durable
user replies → wake value
Instance::resume → Executor continues from saved frames
```

Human wait = **Park**, never unbounded Loop.

### 11.11 Ledger + replay (why this design)

Every meaningful step (especially Calls) is journaled with a hash chain.

Replay = **same Executor walk**, but Call results come from the ledger.  
Final `bag_hash` must match. Divergence → hard refuse.

That’s why true wall-clock parallelism is avoided: order must be total for replay.

### 11.12 Where Conductor fits (later)

Conductor does **not** parse Sol or walk Seq.  
It *is* (will be) a Sol program the same Instance/Executor runs.  
Choosing “spin jobs.list” is just another Call (`harness.invoke`) inside that program.

## 12. How Sol becomes a harness (building block)

### 12.1 One sentence

**A harness = a named, stored Sol program with an I/O contract.**  
Sol is the *body*. Harness is the *packaged program* you can find, spin, and compose.

### 12.2 Layers (don’t mix them)

| Tier | What | Example |
|------|------|---------|
| T1 Control ops | Language keywords | `Seq`, `Branch`, `Call`, `Park` |
| T2 Expr fns | Pure helpers | `add`, `eq`, `concat` |
| T3 Call targets | System calls | `express.say@1`, `math.sum@1`, `harness.invoke@1` |
| **T4 Harness** | **Named Sol program** | `calc.sum_gate`, `quick_reply`, `jobs.list` |

Your sum exercise is a **Sol body**.  
Wrap it with id + summary + inputs/outputs + store it → it becomes a **harness**.

### 12.3 What’s on a harness (conceptually)

```text
Harness {
  id:          "demo.sum_ok"          // name Conductor / parents use
  summary:     "2+3 check; say ok/nope"  // for selection
  inputs:      in.x, in.y  (or seeded Const)
  outputs:     out.reply / out.sum
  program:     <Sol JSON tree>        // the Seq/Call/Branch you wrote
  identity:    BLAKE3 of canonical Sol
}
```

Stored in DB table like `sol_harness_contracts`.  
Boot/install loads many of these → **library of building blocks**.

### 12.4 How one harness uses another

Inside Sol, composition is a **Call**:

```text
Call harness.invoke@1 { id: "demo.sum_ok", ...args } → into child_out
```

Parent harness waits for child Sol to complete (or park).  
Child’s `out.*` becomes a **value** in the parent bag → parent continues.

That’s the building-block rule:

- small harnesses = reusable recipes  
- larger harnesses = Seq of Calls to smaller ones (+ Branch/Park)  
- Conductor = top harness that mostly decides which block to spin

### 12.5 What is *not* a harness

| Thing | Why not |
|-------|---------|
| `add` | T2 expr — too small; always available |
| `express.say@1` | T3 Call — a syscall, not a named program |
| Keyword Rust `if contains("otp")` | Not Sol; not a harness |

Harness exists when it’s **reusable by name**, has **I/O**, and is worth **selecting/composing**.

### 12.6 Path from your sum program → building block

```text
1. Write Sol body (Seq Const Call Branch)     ← you are here
2. Give it id + summary + in/out contract
3. compile() must pass
4. store in sol_harness_contracts
5. Another program Call harness.invoke@1 id=...
6. Later: Conductor can spin it by name from catalog
```

## 13. The ladder in detail (one example end-to-end)

**Example name:** `demo.sum_ok`  
**Job:** given numbers like 2 and 3, compute sum; if sum == 5 say `"ok"`, else `"nope"`.

---

### Rung 1 — Write the Sol body (raw program)

This is only the instruction tree. No id yet. Not in the DB. Not selectable.

Pseudocode:

```text
Seq [
  // assume bag already has x, y  OR use Const to seed
  Call compute.hold( add(pull x, pull y) ) → into sum
  Branch eq(pull sum, lit 5)
    then Call express.say( lit "ok" )   → into reply
    else Call express.say( lit "nope" ) → into reply
]
```

JSON sketch (body only):

```json
{
  "nid": "root",
  "op": "Seq",
  "steps": [
    {
      "nid": "add_xy",
      "op": "Call",
      "id": "compute.hold@1",
      "args": {
        "v": {
          "fn": "add",
          "args": [ { "pull": "x" }, { "pull": "y" } ]
        }
      },
      "into": "sum"
    },
    {
      "nid": "gate",
      "op": "Branch",
      "pred": {
        "fn": "eq",
        "args": [ { "pull": "sum" }, { "lit": 5 } ]
      },
      "then": {
        "nid": "say_ok",
        "op": "Call",
        "id": "express.say@1",
        "args": { "text": { "lit": "ok" } },
        "into": "reply"
      },
      "else": {
        "nid": "say_nope",
        "op": "Call",
        "id": "express.say@1",
        "args": { "text": { "lit": "nope" } },
        "into": "reply"
      }
    }
  ]
}
```

**Check:** `compile(json)` must succeed.  
**Run test:** start Instance with bag `{ "x": 2, "y": 3 }` → expect `reply.text == "ok"`, stable `bag_hash`.

At this rung you have **code**, not yet a **building block**.

---

### Rung 2 — Wrap as a harness contract (name + I/O + body)

Package the body so the OS can store and invoke it by name.

```text
HarnessContract {
  id:       "demo.sum_ok"
  version:  1
  summary:  "Add x+y; reply ok if sum is 5, else nope"
  library:  "demo"
  inputs:   [ in.x:int required, in.y:int required ]
  outputs:  [ out via reply / sum ]
  program:  <JSON from Rung 1>
  identity: BLAKE3(canonical Sol)   // Mother hash
}
```

Convention: caller writes `x`,`y` (or `in.x`/`in.y` once IO convention is fixed) before invoke; harness writes `sum` and `reply`.

**Still not** in the live catalog until Rung 3.

---

### Rung 3 — Store in the library (DB)

```text
store → table sol_harness_contracts
        key: tenant + "demo.sum_ok"
        value: contract (id, summary, program_json, …)
```

Boot / `install_seed_sol_library` can preload many contracts.  
Now it is a **building block**: loadable by id.

```text
load_contract(store, tenant, "demo.sum_ok")
  → program_json → compile → ready to run
```

---

### Rung 4 — Compose: another harness calls it

Build a slightly bigger harness `demo.two_checks` that:

1. Runs `demo.sum_ok` with (2,3)  
2. Runs `demo.sum_ok` with (2,2)  
3. Says a combined message

Parent Sol (sketch):

```text
Seq [
  Call harness.invoke@1 {
      id: "demo.sum_ok",
      x: lit 2,
      y: lit 3
  } → into first

  Call harness.invoke@1 {
      id: "demo.sum_ok",
      x: lit 2,
      y: lit 2
  } → into second

  Call express.say(
      concat("first=", pull first.reply…, " second=", …)
  ) → into report
]
```

What happens under the hood for each invoke:

```text
Parent Executor hits Call harness.invoke@1
  → registry loads child contract "demo.sum_ok"
  → compile child Sol
  → new/nested run with args-only bag {x:2,y:3}
  → child Completes { sum:5, reply: ok }
  → parent bag gets that value at `into first`
  → parent continues to next Seq step
```

**Same kernel.** Different named trees. Child is a building block; parent is a larger block built from smaller ones.

---

### Rung 5 — Conductor spins a block by name (later)

Conductor is itself a harness. Its decide step returns something like:

```text
decision: spin_harness
harness_id: demo.sum_ok
args: { x: 2, y: 3 }
```

Then Conductor Sol does the same thing Rung 4 did:

```text
Call harness.invoke@1 { id: demo.sum_ok, … } → value
→ then quick_reply / rough_chat using that value
```

User message is **not** parsed as Sol.  
Conductor turns intent into **which stored harness id** to run.

```text
User: "check if 2 and 3 make 5"
  → Conductor (LLM + catalog descriptions)
  → spin demo.sum_ok
  → value "ok"
  → reply to user
```

---

### Full ladder picture (this example)

```text
Rung 1  Sol body          Seq / Call / Branch          (anonymous code)
          │
Rung 2  Harness wrap      id=demo.sum_ok + I/O         (named program)
          │
Rung 3  Library store     sol_harness_contracts        (retrievable)
          │
Rung 4  Compose           demo.two_checks invoke×2     (blocks on blocks)
          │
Rung 5  Conductor         decide → spin demo.sum_ok    (shell selects blocks)
```

### What you practice at each rung

| Rung | Practice |
|------|----------|
| 1 | Write/compile/run Sol |
| 2 | Name it + declare inputs/outputs |
| 3 | Persist + reload + same bag_hash |
| 4 | Parent invoke child; use returned value |
| 5 | Catalog selection (no keywords as authority) |

**You are between Rung 1 and 2.** Next concrete act: freeze `demo.sum_ok` id + body JSON; then store/run once.

### 13.1 Done once (2026-08-06) — proof run

| Artifact | Path |
|----------|------|
| Named JSON | `docs/examples/sol_harness_essence/library/demo_sum_ok.json` |
| Kernel test | `aelio-os/crates/aelio-kernel/tests/demo_sum_ok.rs` |

**Verified:**
- `compile(program)` passes
- bag `{x:2,y:3}` → `sum=5`, `reply.text="ok"`
- bag `{x:2,y:2}` → `sum=4`, `reply.text="nope"`
- `cargo test -p aelio-kernel --test demo_sum_ok` → 2 passed

Rung 1–2 body is real. Next baby rung when ready: store into `sol_harness_contracts` + `load_contract` + same result (Rung 3).

### 13.2 Done once (2026-08-06) — Rung 3 store/load

**Verified** (`demo_sum_ok_store_load_run`):

1. Wrap JSON as `SolHarnessContract::seed("demo.sum_ok", …)` (identity sealed)
2. `put_if_absent` into table `sol_harness_contracts`
3. `load_contract(tenant, "demo.sum_ok")` → same `program_json` + `identity`
4. Run loaded body: `{2,3}→ok`, `{2,2}→nope`
5. Reload + rerun happy path → **same `bag_hash`**

`cargo test -p aelio-kernel --test demo_sum_ok` → **3 passed**

Next baby rung when ready: **Rung 4** — parent harness `Call harness.invoke@1` on `demo.sum_ok` and use the returned value.

### 13.3 Done once (2026-08-06) — Rung 4 compose

| Artifact | Path |
|----------|------|
| Parent JSON | `docs/examples/sol_harness_essence/library/demo_two_checks.json` |
| Test | `demo_two_checks_invokes_sum_ok` |

**Verified:**
1. Store both `demo.sum_ok` and `demo.two_checks` in `sol_harness_contracts`
2. Parent `Call harness.invoke@1` twice with `(2,3)` and `(2,2)`
3. Child bags land at `first` / `second` (sums 5 and 4)
4. Parent reports `out.text = "first=ok second=nope"`

`cargo test -p aelio-kernel --test demo_sum_ok` → **4 passed**

Next baby rung when ready: **Rung 5** — Conductor-style decide that spins `demo.sum_ok` by catalog id (still no keyword authority as the product end-state).

### 13.4 Done once (2026-08-06) — Rung 5 Conductor-style

| Artifact | Path |
|----------|------|
| Conductor JSON | `docs/examples/sol_harness_essence/library/demo_conductor_baby.json` |
| Test | `demo_conductor_baby_spins_sum_ok_from_catalog` |

**Flow proven:**
```text
utterance + catalog
  → Call conductor.decide@1  (stub stand-in for future LLM classify)
  → decision { kind:spawn, harness_id:demo.sum_ok, x:2, y:3 }
  → Call harness.invoke@1 by that id
  → out: "conductor spun demo.sum_ok → ok"
```

Also proves rough_chat branch when decide returns non-spawn.

**Honesty:** Sol Conductor has **no** keyword ladder. Decide logic is inside the Call target (stub today → real `llm.classify` later). Sol only Branches on `decision.kind` and invokes by id.

`cargo test -p aelio-kernel --test demo_sum_ok` → **5 passed** (Rungs 1–5).

Ladder complete for this baby example. Next work is productionizing decide (real LLM + catalog) and wiring live turns — not more demo rungs.

### 14. Productionize decide — slice 1 done (2026-08-06)

**Objective of this slice:** shared `conductor.decide@1` Call with a locked in/out contract; scripted backend first (same shape as future model); catalog validation on spawn; Sol Conductor unchanged.

| Piece | Location |
|-------|----------|
| Module | `aelio-kernel/src/conductor_decide.rs` |
| ISA | `conductor.decide@1` in `call_isa` PROC family + frozen registry |
| Prompt pin reserved | `prompt.conductor.decide` |

**Contract:**
```text
in:  utterance, catalog[{id,summary}], prompt_id?
out: kind ∈ spawn|rough_chat|quick_reply
     harness_id? (required if spawn; must be in catalog)
     x?, y? (demo args)
     confidence?, source ∈ scripted|model
```

**Verified:** module unit tests + `demo_conductor_baby_spins_sum_ok_from_catalog` uses `register_conductor_decide` (no inline stub).

### 15. Slice 2 done (2026-08-06) — model backend + live baby cutover

**Objective:** same decide contract with `source=model`; inject helper for replay; run Conductor Sol from API for demo phrases.

| Piece | Location |
|-------|----------|
| Model verdict + `model_decide` | `conductor_decide.rs` |
| `decide_injected` (replay) | `conductor_decide.rs` |
| `register_conductor_decide_model` | injectable classifier |
| `run_conductor_baby_turn` | `conductor_sol_turn.rs` |
| Live cutover | `aelio-agent-api` `/v1/turns` after greeting cutover |

**How to hit live path:**
- Path B default: Conductor Sol on `/v1/turns` (after greeting cutover)
- Decide backend: host LLM (`source=model`) when the provider supports language intelligence; else scripted heuristics (`source=scripted`)
- Opt out Sol cutover: `AELIO_CONDUCTOR_SOL=0` (or legacy spine `AELIO_AGENT_LEGACY=1`)

Trace steps: `Conductor.Sol` with `authority=demo.conductor_baby` and `source=model|scripted`.

**Verified:** `cargo test -p aelio-kernel --lib -- conductor_decide conductor_sol_turn` + `demo_sum_ok`; `cargo test -p aelio-agent --lib -- llm_decide`; `cargo check -p aelio-agent-api`.

**Still open:**
1. Ledger inject of decide on replay in Instance path
2. Catalog loaded from store for live turns (beyond demo ladder)
3. Delete Layer B keyword spine entirely once Phase 5 lands
4. Live-LLM simulation over the 10-harness `sim.*` catalog (oracle path is green)

### §16 — Ten-harness catalog simulation (oracle)

**Artifacts**
- `docs/examples/sol_harness_essence/library/sim/sim_*.json` (10 tag harnesses)
- `demo_conductor_catalog_sim.json` — Conductor Sol over that catalog
- `aelio-kernel/src/conductor_catalog_sim.rs`

**What it proves**
1. Store 10 contracts + Conductor in `sol_harness_contracts`
2. Simulation chat (12 turns) via oracle `ModelVerdict` → decide → spawn/reply
3. Every harness is selected at least once; quick_reply + rough_chat covered

**Run:** `cargo test -p aelio-kernel --lib -- conductor_catalog_sim`

**Honest limit:** oracle is keyword-ish on purpose so the *pipeline* is testable without a gateway. Live LLM path is also green over the **diverse** script (`sim_chat_script_diverse`): choice + **executability** (`ran:<id>` child tag, or non-empty reply; phantom harnesses fail closed).

**Run (scripted LLM wire):** `cargo test -p aelio-agent --lib -- llm_decide`  
**Run (oracle + exec):** `cargo test -p aelio-kernel --lib -- conductor_catalog_sim`  
**Run (live diverse):** `cargo test -p aelio-agent --features direct-provider-tests --test conductor_catalog_sim_live -- --ignored --nocapture`
