# Aelio Harness OS — end-to-end execution plan

**Status:** execution plan (target-state, not a status report)
**Authority:** `AELIO_DSL_MOTHER.md` > `HARNESS_CONDUCTOR_VISION.md` > `LAYER_BREAKDOWN.md` §P1 > this file > code
**Companions:** `HARNESS_REMAINING_WORK.md` (gate tracker) · `PREINSTALLED_HARNESS_EXHAUSTIVE.md` (catalog status)
**Opened:** 2026-08-05

**What this file is for.** `HARNESS_REMAINING_WORK.md` tracks *repairs to what exists*. This file
describes *how to get from here to the finished product* — a Conductor that boots, retrieves Sol
programs from the Aelio DB, and runs them as the agent's entire operational surface, with a standard
library large enough that most work is composition rather than invention.

**Reading rule.** Phases are ordered by dependency, not by appeal. Do not start Phase 3 (the library)
before Phase 2 (the Call ISA) is frozen — every harness written against a moving Call surface has to
be rewritten.

---

## 0. Target state

> A message arrives. The Conductor — itself a stored Sol program — decides: answer directly, spawn a
> harness, exit a layer, or start fresh. Spawning loads a Sol contract from the Aelio DB, compiles it,
> and executes it on the kernel. That program computes, branches, loops, reads and writes pages,
> calls tools through runtime-owned proxies, invokes child harnesses and waits for them, and returns a
> typed output or a typed error. Every program, every system prompt, and every trace lives in the DB.
> New programs are drafted from successful cold paths and installed by explicit promotion.

**Definition of done (all must hold):**

| # | Criterion |
|---|---|
| D1 | No `HarnessStepV1` in the production turn path — Sol is the only body language |
| D2 | Conductor selection is a stored program, not a Rust `match` |
| D3 | ≥120 harnesses installed from the DB at boot, all compiling, all with conformance vectors |
| D4 | Every effectful Call routes through the runtime-owned proxy; none dispatch from a program body |
| D5 | Replay determinism: execute → ledger → replay yields identical `bag_hash` for every library harness |
| D6 | Every program has a Mother §4.3 canonical-Sol + BLAKE3 identity; promotion is explicit and audited |
| D7 | Budget / timeout / depth ceilings enforced at admission, not discovered at runtime |
| D8 | `cargo test --workspace` green with `-D warnings` |

---

## 1. Verified baseline (2026-08-05)

Two independent "harness" implementations exist. **The Conductor plays the weaker one.**

| | Layer A — Sol contracts | Layer B — `HarnessStepV1` |
|---|---|---|
| Home | `aelio-kernel/src/sol_harness_lib.rs` | `aelio-agent/src/harness/program.rs` |
| Body language | Mother Sol op tree (App E JSON) | 7 named sugar ops |
| Ops available | 17 control + full Expr fn set | 7, no compute, no branch, no loop |
| Child harness | ✅ `harness.invoke@1`, nested A→B→C green | ❌ |
| Storage | `sol_harness_contracts` table, `store_library()` | `TenantDecl.harness_programs` (empty on disk) |
| **Runs a real turn** | ❌ | ✅ `play_harness_program` in `turn.rs` |

**Green today:** `every_library_program_compiles`, `parent_waits_on_child_invoke_works`,
`nested_stack_a_waits_on_b_waits_on_c`, `store_library_persists_all_contracts`,
`combination_harnesses_compile_and_help_router_runs`.

**The gap is wiring, not invention.** The ISA, the compute set, nested invoke, and durable storage all
exist and pass tests. Nothing in the agent turn path loads them.

### 1.1 The four tiers (fix this vocabulary before proceeding)

Confusion between these is the main source of thrash. `add` and `spawn` are *not* the same kind of
thing, and neither is a harness.

| Tier | What | Extensible? | Where |
|---|---|---|---|
| **T1 — Control ops** | `Seq` `Let` `Branch` `Loop` `Try` `Fallback` `Guard` `Budget` `Timeout` `Once` `Park` `Tee` `Map` `Filter` `Call` `Const` `Identity` | **No** — closed, Mother §8; changing needs an amendment | `aelio-sol` |
| **T2 — Expr fns** | `add` `mul` `concat` `count` `eq` `pull` `blake3` … | Closed-ish, Mother §9/F3 | `aelio-kernel/src/compute.rs` |
| **T3 — Call targets** | `express.say@1`, `memory.search@1`, `harness.invoke@1`, `tool.invoke@1` … | **Yes** — this is the driver/ability surface | `registry.rs` + host |
| **T4 — Harnesses** | Named Sol programs composed from T1–T3 | **Yes** — this is the library | `sol_harness_contracts` table |

**Design rule.** Pure math inside a step is a **T2 fn**, not a harness. A harness exists when a unit is
(a) reusable by name, (b) has an IO contract, and (c) is worth Conductor selecting. `num.add` as a
harness is ceremony; `calc.average` as a harness is real, because it composes, branches on empty
input, and returns a typed error.

---

## 2. Decision: Path B — Sol-only bodies

`LAYER_BREAKDOWN.md` §P1 leaves Path A (sugar + lowering) vs Path B (Sol-only) open. **Choose B.**

**Rationale.** Path A keeps two runtimes alive forever. Every new op must be implemented twice, and
every divergence becomes a silent behavior fork — the exact class of bug that produced the current
"stored programs are persisted" claim while the live catalog held `{}`. Path A's stated benefit is
authoring ergonomics, which is a *tooling* problem, solvable above the storage layer.

**The rule:** sugar may exist as an **authoring input**, but it is compiled to Sol at admission time
and only Sol is stored, hashed, and executed. There is no sugar interpreter at runtime.

**Consequences to accept up front:**

1. `HarnessStepV1` and `play_harness_program` are deleted, not maintained (Phase 5).
2. The four starters are rewritten as Sol contracts — three already exist in `sol_harness_lib.rs`.
3. `HarnessPlayMode::Hardcoded` loses its purpose once parity is proven; remove it with the A/B.
4. Authoring UX becomes a separate, optional concern (Phase 6.5), not a runtime dependency.

**Log this as a Decision Log entry** and a FLAGS amendment — it resolves an explicitly open question.

### 2.1 Parallelism: a correction to the mental model

Sol has **no parallelism**. `Tee` reads like fork-join but `exec.rs:902` runs body to completion, then
side, and reports side failure while main continues — sequential fire-and-record.

This is deliberate: replay determinism (execute → ledger → replay → identical `bag_hash`) is a locked
invariant, and true concurrency breaks it absent a total order.

**Therefore "run sum and count in parallel, then divide" is expressed as deterministic fan-out/join:**
invoke `calc.sum`, invoke `list.count`, then divide. Identical semantics, replayable, expressible
today. If wall-clock concurrency for I/O-bound children is ever needed, it is a Mother amendment with
a declared deterministic join order — flag it, never sneak it in.

---

## 3. Phase plan

### Phase 0 — Repair and baseline

**Goal:** a tree that compiles, with the current work committed, before any new construction.

| Step | Action | Acceptance |
|---|---|---|
| 0.1 | Restore `pub use contract::{AbilityContract, AbilityPath, FieldSchema, Predicate, TypeSchema};` in `aelio-agent/src/lib.rs` (Gate H0-1) | `cargo test -p aelio-agent` compiles all targets |
| 0.2 | Full workspace test run, record per-suite results | `cargo test --workspace` green |
| 0.3 | Commit the harness slice: `harness/`, `harness_conductor.rs`, `harness_program_benchmark.rs`, `docs/architecture/` | `git status` shows no untracked harness files |
| 0.4 | Raise `list_contract_ids` scan limit — currently `scan_prefix(…, 256)` in `sol_harness_lib.rs:141`, which silently truncates a 200+ library as it grows | Test with 300 contracts returns 300 |

**Do not skip 0.4.** A silently truncated library listing is the failure mode that will waste a day
in Phase 3 when a harness "isn't installed."

---

### Phase 1 — Contract model hardening

**Status:** **done (2026-08-05)** — see change log M-15–M-18. `SolHarnessContract` is first-class;
admission + BLAKE3 identity green for the full seed library.

**Goal:** make a stored harness a first-class, versioned, typed, identity-bearing artifact.

~~Today `SolHarnessContract { id, summary, program_json }` is a bare triple.~~ Implemented.

**1.1 — Extend the contract:**

```rust
pub struct SolHarnessContract {
    pub id: String,              // namespaced: "calc.average"
    pub version: u32,
    pub summary: String,         // selection-facing description
    pub library: String,         // "calc" | "control" | "memory" | …
    pub program_json: String,    // App E instruction tree
    pub signature: HarnessSignature,
    pub effect: EffectClass,     // pure | read | write | external
    pub budget: BudgetSpec,      // max calls / tokens / ms / depth
    pub identity: String,        // canonical Sol + BLAKE3, Mother §4.3
}

pub struct HarnessSignature {
    pub inputs:  Vec<SlotSpec>,  // bag path, type, required, default
    pub outputs: Vec<SlotSpec>,
    pub errors:  Vec<String>,    // reason codes it may return
}
```

**1.2 — Identity per Mother §4.3.** Canonical Sol serialization + BLAKE3. Do **not** repeat the
`serde_json` + SHA-256 pattern from `harness/program.rs:137` — that is the exact defect already
remediated as checklist item G1-2. The identity is what promotion, pinning, and audit hang on.

**1.3 — Validation at admission.** A contract is rejected unless: it compiles; every `Call` id is
registered; the call graph is acyclic (`registry.rs:285` `validate_call_graph`); declared effect
matches the effects of its calls (a `pure` harness cannot contain a `write` call); every input path is
either provided or defaulted; budget is present and within ceiling.

**1.4 — IO convention.** Fix it once, in writing:

```
in.*      inputs written by the caller before invoke
out.*     outputs the harness promises to write
err       set on typed failure: {reason, nid, detail}
page.*    working memory, harness-scoped, survives Park
tmp.*     scratch, dropped on return
```

Acceptance: a contract declaring `out.value` that returns without writing it fails its own test.

---

### Phase 2 — Freeze the Call ISA

**Goal:** the T3 surface every harness composes against. **Freeze before Phase 3.**

Current registered targets are demo stubs: `express.say@1`, `understand.classify@1`,
`memory.search@1`, `context.attach@1`, `compute.hold@1`, `tool.act_stub@1`, `harness.invoke@1`.

**2.1 — Process / spawn family** (the part your model needs most):

| Call id | Effect | Semantics |
|---|---|---|
| `harness.invoke@1` | inherits child | Run child to completion, merge its `out.*` into `into` path. **Exists.** |
| `harness.spawn@1` | inherits | Push child as a stack frame, transfer control; parent resumes on child return |
| `harness.invoke_seq@1` | inherits | Invoke N children in declared order, collect outputs into a list |
| `harness.return@1` | pure | Pop frame, return `out.*` to parent |
| `harness.exit_up@1` | pure | Pop one layer only; ceiling is Conductor |
| `harness.fresh@1` | write | Clear stack, restart at Conductor |
| `harness.describe@1` | read | Return a contract's signature + summary (selection support) |
| `harness.list@1` | read | List installed ids filtered by library / effect |

`invoke_seq` is the deterministic fan-out/join from §2.1 — the honest form of "parallel."

**2.2 — Tool family.** `tool.invoke@1` replaces `tool.act_stub@1`. **This is the highest-risk item in
the plan.** It must dispatch through the runtime-owned proxy per Mother §12.4 intent/dispatch/result,
never directly from the program body. Effectful invocations must be `Once`-wrapped for idempotency
and carry a ledgered intent. **Open a FLAGS entry with a recommendation before writing this** — it
touches locked execution semantics.

**2.3 — LLM family.** Model access as ordinary registered Calls, each with a stored prompt artifact id
rather than an inline string:

`llm.classify@1` · `llm.extract@1` · `llm.generate@1` · `llm.rewrite@1` · `llm.summarize@1` ·
`llm.judge@1` · `llm.sanitize@1` · `llm.embed@1` · `llm.rerank@1`

**2.4 — Memory family.** `memory.search@1` · `memory.write@1` · `memory.forget@1` ·
`page.read@1` · `page.write@1` · `page.append@1` · `page.compact@1` · `slot.set@1` · `slot.get@1`

**2.5 — Prompt artifacts.** Every `llm.*` call names a stored prompt, versioned and hashed like a
contract. This closes Gate H4 and is a prerequisite for Phase 7 — a drafted harness that references an
unpinnable prompt just relocates the hardcoding.

**Exit criterion:** the Call table is written down, every id has an effect class and a declared arg
schema, and `validate_call_graph` passes for the whole seed library. Only then start Phase 3.

---

### Phase 3 — The standard library

**Goal:** ≥120 installed harnesses, so that composition beats invention. Full catalog in §4.

**Method — do not hand-write 120 JSON trees.**

1. Write a **builder DSL in Rust test code** (`sol_harness_lib.rs` already does this) that emits
   contracts. Hand-authoring App E JSON at this volume guarantees drift.
2. For each harness, write a **conformance vector** in `docs/vectors/harness_<id>.json`: input bag,
   expected output bag, expected error. The vector is the spec; the contract satisfies it.
3. One test asserts **every** installed contract compiles, validates, and has ≥1 vector. A harness
   without a vector fails the build.
4. Group into libraries and land library-by-library, committing per group:
   `feat(kernel): calc library harnesses per §9, vectors calc_*`

**Per-harness checklist** — no exceptions:

- [ ] Namespaced id + version + library + summary written for *selection*, not for developers
- [ ] Signature: inputs, outputs, error reason codes
- [ ] Effect class declared and matching its calls
- [ ] Budget declared
- [ ] Conformance vector: happy path
- [ ] Conformance vector: each declared error path
- [ ] Replay determinism assertion (`bag_hash` stable)

---

### Phase 4 — Conductor on Sol

**Goal:** D1 and D2 — the Conductor becomes a stored program and plays Sol bodies.

**4.1 — Play path.** Replace `play_harness_program` in `turn.rs` with a kernel-backed executor:
load contract from `sol_harness_contracts` → compile → execute with the turn's bag → map `Flow::Suspend`
to the existing Park/resume machinery → merge `out.*` into the turn result.

The stack (`HarnessSession`, frames, waiting-child, context pages) **stays** — it is proven and
orthogonal. Only the *body language* changes.

**4.2 — Conductor as a program.** Today: `select_starter_harness`, a keyword ladder that cannot see
the 31 registered tools. Target: a Sol contract `conductor` whose body is

```
Seq[
  Call llm.classify@1  → in.utterance + installed catalog + page context
                       → out.decision ∈ {reply, spawn, exit_up, fresh}
  Branch on out.decision:
    reply   → Call harness.invoke@1 quick_reply
    spawn   → Call harness.invoke@1 (id from out.harness_id)
    exit_up → Call harness.exit_up@1
    fresh   → Call harness.fresh@1
]
```

Selection candidates come from `harness.list@1` + `harness.describe@1`, so **installing a harness makes
it selectable with no code change** — this is the property that makes it an OS.

Keep a rule-based fast path in front for greetings and stack-control phrases, so trivial turns stay
model-free (today: 0 LLM calls on `"hello"` — do not regress this).

**4.3 — Escalate stays.** Cold `ProposePath` remains the permanent fallback when nothing fits. The
library grows; it never becomes mandatory.

**4.4 — Selection quality.** Fix the recorded misses: short factual questions must prefer `quick_reply`
over `understand_intent`; tool detection must derive from the registry rather than the ten hardcoded
phrases in `conductor.rs:99`.

---

### Phase 5 — Persistence, admission, migration

**5.1 — One storage path.** `sol_harness_contracts` is the home. Retire
`TenantDecl.harness_programs`. Until it is deleted, do not let two libraries drift.

**5.2 — Seed before persist.** The current defect: `register_catalog` upserts, *then* seeds
(`durable.rs:2697-2708`), so the seed never reaches disk. Seed first, then validate, then persist.

**5.3 — Admin API:**

```
GET    /v1/admin/harness              list installed (id, version, identity, library, effect)
GET    /v1/admin/harness/:id          full contract
POST   /v1/admin/harness/:id          validate → compile → hash → upsert → reload
DELETE /v1/admin/harness/:id          refuse if referenced by an installed contract
POST   /v1/admin/harness/:id/verify   run its vectors against the installed body
```

**5.4 — Migration.** Rewrite the 4 starters as Sol (3 already exist), prove parity against the
`HarnessPlayMode` A/B lanes, then delete `HarnessStepV1`, `play_harness_program`, the `exec_*` bodies,
and `HarnessPlayMode` itself in one commit.

**5.5 — Boot.** Seed missing library contracts with `put_if_absent` (`store_library` already does
this), so tenant-authored overrides survive. Never overwrite an installed contract on boot.

---

### Phase 6 — Production hardening

| # | Item | Requirement |
|---|---|---|
| 6.1 | Budgets | Every harness declares max calls / tokens / ms; enforced by `Budget`; exceeding is a typed refusal, never a hang |
| 6.2 | Depth ceiling | `harness.invoke` nesting capped; cycles rejected at admission via `validate_call_graph` |
| 6.3 | Timeouts | Every external Call inside `Timeout`; expiry is a typed error with a defined recovery |
| 6.4 | Replay | Property test: every library harness replays to an identical `bag_hash` |
| 6.5 | Idempotency | Effectful calls `Once`-wrapped and keyed; replay must not re-fire effects |
| 6.6 | Failure taxonomy | Every harness declares its reason codes; no bare panics; missing input is `Missing`, not a crash |
| 6.7 | Observability | Trace shape `Harness.Load id=… identity=… → Harness.Op …` persisted per step (already works for Layer B — preserve it) |
| 6.8 | Secrets / redaction | `policy.redact` before any `llm.*` call that touches user data; prompts stored, never logged raw |
| 6.9 | Authorization | Effectful tool calls carry tenant scope; a harness cannot widen its own effect class at runtime |
| 6.10 | Load | Soak: 10k turns across the library, no leak in stack/pages, WAL growth bounded |

---

### Phase 7 — The growth loop

**Only after Phases 0–6.** This is Gate H5, currently zero lines of code.

```
utterance → nothing fits → Escalate → cold ProposePath → TypeCheck → execute → success
   ↓
lower the accepted path into a Sol contract draft (status="draft", never active)
   ↓
admin review → promote → upsert → reload
   ↓
next matching utterance → Conductor selects it → Harness.Load, no ProposePath
```

**Locked constraints:**

- Effectful drafts **never** auto-install. Same P5 posture that governs procedure promotion. No
  config flag may bypass it.
- Drafts live outside the active library.
- Draft ids derive from the situation key, so repeats update one draft instead of accumulating
  near-duplicates.

**Note:** the existing `Learn.Observe` loop is *not* this. It promotes **procedures** (3 observations,
≥0.8 success, `learn.rs:626`) and explicitly excludes effectful paths — so it will never crystallize
the OTP/login/payment cases. Phase 7 is a new writer.

---

## 4. The standard library catalog

Target ≥120 harnesses. `[S]` = Sol contract exists today · `[N]` = to be written.

**Naming:** `library.name`, lowercase, dot-separated. Inputs at `in.*`, outputs at `out.*`.

> Reminder from §1.1: entries below are **harnesses** (T4). Where an item is trivially a T2 fn, it is
> listed only because it earns its keep as a *named, selectable, individually testable* unit with
> declared errors — e.g. `num.divide` exists as a harness because it must return a typed
> `DivideByZero` rather than trapping.

### 4.1 `core` — identity and plumbing

| id | in → out | Notes |
|---|---|---|
| `core.identity` | `in.v` → `out.v` | `[N]` no-op passthrough |
| `core.const` | — → `out.v` | `[N]` emit a literal |
| `core.noop` | — → — | `[N]` |
| `core.fail` | `in.reason` → `err` | `[N]` deliberate typed failure |
| `core.assert` | `in.pred` → `out.ok` \| `err` | `[N]` `Guard` |
| `core.pipe` | `in.v`, `in.steps` → `out.v` | `[N]` sequential apply |
| `core.trace` | `in.label` → — | `[N]` emit trace step |
| `core.hash` | `in.v` → `out.hash` | `[N]` `blake3` |

### 4.2 `num` — arithmetic

| id | in → out | Notes |
|---|---|---|
| `num.add` `num.sub` `num.mul` | `in.a`,`in.b` → `out.value` | `[N]` |
| `num.divide` | `in.a`,`in.b` → `out.value` \| `err DivideByZero` | `[N]` `Branch` guard |
| `num.mod` `num.abs` `num.neg` | | `[N]` |
| `num.min` `num.max` `num.clamp` | | `[N]` |
| `num.round` `num.floor` `num.ceil` | | `[N]` |
| `num.pow` `num.sqrt` | | `[N]` `sqrt` of negative → typed error |
| `num.sum` | `in.values[]` → `out.value` | `[N]` `Map` + fold |
| `num.count` | `in.values[]` → `out.value` | `[N]` |
| `num.average` | `in.values[]` → `out.value` \| `err EmptyInput` | `[N]` **the canonical composite**: invoke `num.sum`, invoke `num.count`, branch on zero, divide |
| `num.median` `num.mode` `num.stddev` | | `[N]` |
| `num.percent` `num.ratio` | | `[N]` |
| `num.in_range` | | `[N]` |
| `calc.sum_gate` | | `[S]` `Const`,`Call`,`Branch`,add/gt |
| `calc.repeat_add` | | `[S]` `Loop` |
| `calc.mul_div` | | `[S]` `Branch`, mul/div/eq |

### 4.3 `cmp` / `logic`

| id | Notes |
|---|---|
| `cmp.eq` `cmp.ne` `cmp.lt` `cmp.le` `cmp.gt` `cmp.ge` | `[N]` |
| `cmp.between` `cmp.approx` | `[N]` |
| `logic.and` `logic.or` `logic.not` `logic.xor` | `[N]` |
| `logic.all` `logic.any` `logic.none` | `[N]` over a list |
| `logic.if_else` | `[N]` value-level `Branch` |
| `logic.switch` | `[N]` desugars via `sugar.rs` |
| `logic.coalesce` | `[N]` first non-null |

### 4.4 `str` — text

| id | Notes |
|---|---|
| `str.concat` `str.join` `str.split` | `[N]` |
| `str.length` `str.trim` `str.lower` `str.upper` `str.title` | `[N]` |
| `str.contains` `str.starts_with` `str.ends_with` | `[N]` |
| `str.replace` `str.slice` `str.pad` `str.truncate` | `[N]` |
| `str.template` | `[N]` `{slot}` interpolation from bag |
| `str.words` `str.sentences` `str.tokens` | `[N]` |
| `str.word_count` `str.char_count` | `[N]` |
| `str.matches_format` | `[N]` |
| `str.normalize` `str.strip_punct` | `[N]` |
| `str.contains_gate` | `[S]` |
| `str.parse_number` `str.parse_bool` | `[N]` typed error on failure |
| `str.to_json` `str.from_json` | `[N]` |

### 4.5 `list` — collections

| id | Notes |
|---|---|
| `list.map` | `[N]` `Map` over invoked child |
| `list.filter` | `[N]` `Filter` |
| `list.reduce` `list.fold` | `[N]` `Loop` |
| `list.select` | `[N]` **"select"** — pick by predicate, first match |
| `list.select_many` | `[N]` all matches |
| `list.find_index` | `[N]` |
| `list.first` `list.last` `list.nth` | `[N]` empty → typed error |
| `list.slice` `list.take` `list.drop` | `[N]` |
| `list.append` `list.prepend` `list.concat` | `[N]` |
| `list.count` `list.is_empty` | `[N]` |
| `list.contains` | `[N]` |
| `list.unique` `list.sort` `list.reverse` | `[N]` sort needs a declared key + stable order |
| `list.group_by` `list.partition` | `[N]` |
| `list.zip` `list.flatten` | `[N]` |
| `list.sum` `list.min` `list.max` | `[N]` |
| `list.filter_keep` | `[S]` |
| `list.chunk` `list.window` | `[N]` |

### 4.6 `obj` — structure and paths

| id | Notes |
|---|---|
| `obj.get` `obj.set` `obj.exists` | `[N]` |
| `obj.merge` `obj.drop` `obj.keep` `obj.pick` | `[N]` |
| `obj.rename` `obj.copy_path` | `[N]` |
| `obj.keys` `obj.values` `obj.entries` | `[N]` |
| `obj.from_entries` | `[N]` |
| `obj.is_type` `obj.validate_schema` | `[N]` |
| `obj.default` | `[N]` fill missing with default |

### 4.7 `ctrl` — control flow as harnesses

| id | Notes |
|---|---|
| `ctrl.if` `ctrl.switch` | `[N]` |
| `ctrl.loop_while` `ctrl.loop_times` `ctrl.foreach` | `[N]` `max_iter` mandatory |
| `ctrl.try` `ctrl.retry` `ctrl.fallback` | `[S]` `fallback_say` exists |
| `ctrl.guard` | `[S]` `guard_required_field` |
| `ctrl.budget` `ctrl.timeout` | `[N]` |
| `ctrl.once` | `[S]` `once_external_stub` |
| `ctrl.park` | `[S]` inside `wait_for_user` |
| `ctrl.detect_fresh` | `[S]` `detect_fresh_utterance` |

### 4.8 `proc` — process / spawn

**The heart of the model.**

| id | Notes |
|---|---|
| `proc.spawn` | `[N]` push child frame, transfer control |
| `proc.invoke` | `[S]` `parent_waits_on_child` proves it |
| `proc.invoke_seq` | `[N]` N children, deterministic order, collect outputs — the fan-out/join form |
| `proc.return` | `[N]` pop, return `out.*` |
| `proc.exit_up` | `[N]` pop one layer, ceiling Conductor |
| `proc.fresh` | `[N]` clear stack |
| `proc.select_harness` | `[N]` choose an id from catalog + context — used by Conductor |
| `proc.describe` `proc.list` | `[N]` catalog introspection |
| `proc.stack_depth` | `[N]` |
| `proc.stack_top_a` / `mid_b` / `leaf_c` | `[S]` nested demos, green |

### 4.9 `mem` — memory and pages

| id | Notes |
|---|---|
| `mem.search` | `[S]` `memory_search` |
| `mem.attach` | `[S]` `memory_attach` |
| `mem.write` `mem.forget` | `[N]` |
| `page.read` `page.write` `page.append` | `[N]` harness-scoped working memory |
| `page.slot_set` `page.slot_get` | `[N]` |
| `page.compact` `page.summarize` | `[N]` `llm.summarize` when over budget |
| `page.clear` | `[N]` |
| `mem.embed` `mem.similar` | `[N]` Aelio DB vector path |
| `mem.recall_recent` | `[N]` |

### 4.10 `llm` — model-backed

Each names a **stored prompt artifact**, never an inline string.

| id | Notes |
|---|---|
| `llm.classify` | `[S]` via `understand.classify@1` |
| `llm.extract` | `[N]` structured slots from text |
| `llm.generate` | `[N]` |
| `llm.rewrite` `llm.summarize` | `[N]` |
| `llm.judge` | `[N]` scored evaluation |
| `llm.sanitize` | `[N]` strip injection / PII before downstream use |
| `llm.decide` | `[N]` constrained choice from an enum — Conductor's core call |
| `llm.embed` `llm.rerank` | `[N]` |
| `llm.explain` | `[N]` render a trace as prose |

### 4.11 `tool` — external effects

| id | Notes |
|---|---|
| `tool.list` `tool.describe` | `[N]` registry introspection |
| `tool.select` | `[N]` pick a tool for an intent |
| `tool.invoke` | `[N]` **Mother §12.4 protocol, runtime proxy, `Once`-wrapped** |
| `tool.confirm_then_act` | `[S]` `confirm_then_act` — Park for confirmation, then act |
| `tool.dry_run` | `[N]` validate args without effect |
| `tool.act_stub` | `[S]` demo only; delete at Phase 5 |

### 4.12 `conv` — conversation

| id | Notes |
|---|---|
| `conv.quick_reply` | `[S]` |
| `conv.full_reply` | `[S]` |
| `conv.greet` | `[S]` `greet_then_offer_help` |
| `conv.understand_intent` | `[S]` |
| `conv.wait_for_user` | `[S]` `Park` |
| `conv.clarify_slot` | `[S]` |
| `conv.apologize_closed` | `[S]` |
| `conv.confirm` `conv.handoff` `conv.smalltalk` | `[N]` |
| `conv.semantic_ack` | `[S]` |
| `conv.help_router` | `[S]` |
| `conv.offer_options` | `[N]` |
| `conv.repeat_back` | `[N]` |

### 4.13 `policy` — safety and governance

| id | Notes |
|---|---|
| `policy.redact` | `[N]` PII strip before `llm.*` |
| `policy.consent_gate` | `[N]` Park until consent |
| `policy.approval_gate` | `[N]` human approval for effectful |
| `policy.rate_limit` | `[N]` |
| `policy.safety_check` | `[N]` |
| `policy.scope_check` | `[N]` tenant authorization |
| `policy.budget_guard` | `[N]` |

### 4.14 `time`

| id | Notes |
|---|---|
| `time.now` | `[N]` ledgered nondet |
| `time.parse` `time.format` `time.diff` `time.add` | `[N]` |
| `time.is_before` `time.is_after` | `[N]` |
| `time.schedule` | `[N]` |

### 4.15 `meta` — self-description and growth

| id | Notes |
|---|---|
| `meta.list_harnesses` `meta.describe_harness` | `[N]` |
| `meta.validate_harness` | `[N]` compile + vector check |
| `meta.draft_harness` | `[N]` Phase 7 |
| `meta.promote_harness` | `[N]` Phase 7, admin-gated |
| `meta.trace_last_turn` | `[N]` |
| `meta.explain_decision` | `[N]` |

### 4.16 Pipelines (composites already proven)

`greet_then_quick_reply` · `greet_memory_pipeline` · `memory_then_full_reply` ·
`understand_then_memory` · `intent_then_calc` · `calc_then_report_pipeline` ·
`fresh_then_greet` · `budgeted_express` — all `[S]`, all compiling.

**Count:** ~34 `[S]` today, ~150 `[N]` planned → **~184 target**, exceeding D3's floor of 120.

---

## 5. Testing strategy

| Layer | Test | Gate |
|---|---|---|
| Contract | Compiles; validates; call graph acyclic | Every contract, every build |
| Behavior | Conformance vector per happy path + per declared error | Phase 3 exit |
| Determinism | execute → ledger → replay → identical `bag_hash` | Phase 6.4 |
| Idempotency | Replay does not re-fire effects | Phase 6.5 |
| Selection | Scripted utterances → expected harness id | Phase 4 exit |
| Integration | Park/resume, nested invoke, exit_up, fresh across a restart | Phase 4 exit |
| Parity | Sol lane vs legacy lane identical control plane, then delete legacy | Phase 5.4 |
| Soak | 10k turns, bounded memory and WAL | Phase 6.10 |

**Vector layout:** `docs/vectors/harness_<library>_<name>.json` —
`{ id, in, expect_out, expect_err, expect_bag_hash }`.

---

## 6. Execution order

```
Phase 0  repair + commit + scan-limit fix
Phase 1  contract model: signature, identity (BLAKE3), validation, IO convention
Phase 2  FREEZE Call ISA — proc.*, tool.invoke (FLAG first), llm.*, mem.*, prompt artifacts
Phase 3  library, one namespace at a time, vector-first:
         core → num → cmp/logic → str → list → obj → ctrl → proc → mem → llm → tool → conv → policy → time → meta
Phase 4  Conductor on Sol: play path, selection as a program, quality fixes
Phase 5  single storage path, admin API, migrate starters, DELETE HarnessStepV1
Phase 6  hardening: budgets, depth, timeouts, replay, idempotency, redaction, soak
Phase 7  growth loop: draft → admin promote → selectable
```

**Critical path:** 0 → 1 → 2 → 4 → 5. Phase 3 is wide and parallelizable across people once Phase 2 is
frozen. Phase 6 runs continuously from Phase 3 onward, not as a final gate. Phase 7 last — it depends
on everything.

**Highest risks, in order:**

1. **`tool.invoke@1` semantics** (Phase 2.2) — touches locked Mother execution rules. FLAG before code.
2. **Call ISA churn** — if T3 moves during Phase 3, the library is rewritten. Freeze hard.
3. **Two libraries drifting** during migration — keep the window between 5.1 and 5.4 short.
4. **Selection quality at scale** — a 180-harness catalog is a harder selection problem than 4. Budget
   real work for `llm.decide` prompt design and expect to add a retrieval step over `harness.describe`.

---

## 7. Open decisions (resolve before the phase that needs them)

| # | Question | Needed by | Recommendation |
|---|---|---|---|
| O1 | Path A vs Path B | Phase 2 | **Resolved: B** — Sol-only bodies; sugar compiles at admission (§2); F-025 |
| O2 | Harness identity hash | Phase 1 | **Resolved** — Canonical Sol + BLAKE3 (`contract_identity`); M-16 |
| O3 | Does `tool.invoke` need a Mother amendment? | Phase 2 | Likely yes — FLAG with recommendation |
| O4 | True parallelism ever? | Phase 2 | Not now — deterministic fan-out/join (§2.1) |
| O5 | Conductor fully Sol, or Rust fast-path + Sol body? | Phase 4 | Hybrid: keep the 0-LLM greeting path, Sol for the rest |
| O6 | Tenant harness namespacing / override rules | Phase 5 | `put_if_absent` seeds; tenant ids win; never overwrite on boot |
| O7 | Draft storage location | Phase 7 | Separate status, outside the active library |
