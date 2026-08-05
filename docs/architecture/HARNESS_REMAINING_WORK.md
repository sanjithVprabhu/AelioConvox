# Harness + Conductor — remaining work

**Status:** active working checklist  
**This is the one file to keep open** for reverse-engineering + execute + change.  
**Authority:** `AELIO_DSL_MOTHER.md` > `docs/architecture/HARNESS_CONDUCTOR_VISION.md` > this file > code comments  
**Scope:** everything between the current verified state and "Conductor grows its own library"  
**Opened:** 2026-08-04, from a verification pass over the uncommitted harness slice

**Rule:** do not mark an item complete from memory. Mark complete only when the acceptance column is
proven on a clean tree. Prefer linking the commit. Where an item contradicts a claim already written
into the vision doc, fixing the doc is part of the item.

**On line numbers:** this slice was under active edit when the doc was written, and citations drifted
within the same session. Line numbers are a hint; the symbol name or quoted snippet is the real
reference. Grep the snippet if a line does not match.

Legend: `[ ]` open · `[~]` in progress · `[x]` done · `[!]` blocked / needs decision

---

## Mental model (did you understand it?)

Yes — there **is** a coded pseudo-language of operations. There are **two layers**. Mixing them
is the main source of confusion.

```
User message
    │
    ▼
Conductor (top harness)          ← chooses which named program to run
    │
    ▼
HarnessProgramV1 steps           ← LAYER B: harness "pseudo-ops" (JSON IR)
  prompt.quick_reply
  memory.search
  ask_and_wait
  return.finish
    │
    ▼
Agent runtime (aelio-agent)      ← plays those steps; may Call tools / Park / LLM
    │
    ▼  (when lowered flows / Sol trees run)
aelio-kernel                     ← LAYER A: Mother Sol control ops
  Seq / Loop / Tee / Once / Park / Call / Const / …
    │
    ▼
bag + ledger + store             ← durable state / replay
```

| Layer | What it is | Where | Who “runs” it |
|-------|------------|--------|----------------|
| **A — Sol / kernel** | Normative op tree from `AELIO_DSL_MOTHER.md` (§8) | `aelio-os/crates/aelio-kernel` | Planner validates → Executor walks tree |
| **B — Harness IR** | Named conversation programs (`HarnessStepV1`) | `aelio-agent/src/harness/program.rs` | Conductor loads → turn path dispatches steps |

**Correct intuition:** write a program of ops → runtime executes them in order → effects / parks / replies happen.

**Important correction:** today the thing you are shaping to “work like you want” is mostly **Layer B**
(Conductor + harness library). Layer A is the Mother kernel underneath installed Sol pins / flows.
Harness steps are *not* Sol `Kind` nodes yet — they are a higher-level IR that the agent runtime
interprets. Vision goal: programs stay data; brain stays fixed.

**P1 (2026-08-04): harness bodies must become Sol. Status: INCOMPLETE.** See
`docs/architecture/LAYER_BREAKDOWN.md` §P1 and Gate **HS** below. `HarnessStepV1` is temporary sugar
until lowered/stored as Sol.

**How we reverse-engineer without thrashing**

1. Keep **this file** as the only checklist. Vision doc = intent. Mother = law. Code = truth.
2. One gate at a time: **H0 → H1 → H2 → H3 → H4 → H5** (do not skip to create-harness before H0/H1).
3. Each item: read the instructions → change code → run the **Acceptance** command/test → mark `[x]` only with evidence.
4. If behavior ≠ what you want: write the desired behavior under the gate as a new row (or fix the vision doc), then implement — don’t silent-edit without a checkbox.

**Quick “prove you get it” questions**

1. What runs when stack is empty? → Conductor.
2. Where do starter programs live *in code*? → `starter_harness_library()` / `HarnessProgramV1`.
3. Why is “saveable” half-true today? → seeded in memory at boot; live catalog often still empty on disk (H1).
4. What is broken first? → H0: `cargo test -p aelio-agent` fails on missing `contract` re-exports.

---

## 0. Verified state (2026-08-04)

Established by direct inspection, not by reading the checkboxes. Keep this section honest — it is the
baseline every item below is measured against.

**Proven working:**

- `HarnessProgramV1` is a real tagged-JSON step IR with 7 named ops — `harness/program.rs`
- Dispatch is genuinely data-driven. Editing a stored program's `question_template` changed the live
  reply; injecting `memory.search` + `context.attach` into `quick_reply`'s step list caused those ops
  to actually execute; deleting a program produced a clean refusal (`Harness.Load | missing program
  id=quick_reply`), not a panic
- Stored/hardcoded control-plane parity holds; per-suite tests green across 5 consecutive rounds
  (`lib` 112 · `golden_traces` 24 · `harness_conductor` 6 · `harness_program_benchmark` 2)
- `harness_sessions` is a live table with real rows — stack frames and context pages persist
- Turn traces persist per step (`step_attempt` rows carrying `Harness.Load` / `Harness.Op`)

**Proven NOT working / not built:**

- `cargo test -p aelio-agent` does not build (see H0)
- The live DB's active catalog carries `harness_programs: {}` — all 16 persisted versions empty.
  Programs reach the runtime from Rust code at boot, never from storage (see H1)
- Zero runtime writes to `harness_programs` anywhere in the workspace. No create-harness path (see H5)
- Conductor's "is this a tool call" test is a hardcoded phrase list that never sees registered
  tools (see H3)

---

## Gate H0 — Unblock the build (do this first)

| ID | Item | Acceptance | Status |
|---|---|---|---|
| H0-1 | Restore the deleted `contract` re-export | `cargo test -p aelio-agent` compiles all targets | [x] 2026-08-05 — see below |

### H0-1 instructions

The harness `pub use` block at `aelio-os/crates/aelio-agent/src/lib.rs:36` **replaced** rather than
joined the previous line. `git show HEAD:aelio-os/crates/aelio-agent/src/lib.rs` confirms the original
at line 35. Restore it alongside the harness exports:

```rust
pub use contract::{AbilityContract, AbilityPath, FieldSchema, Predicate, TypeSchema};
```

Current failure:

```
error[E0432]: unresolved import `aelio_agent::Predicate`
error[E0433]: failed to resolve: could not find `AbilityContract` in `aelio_agent`
error: could not compile `aelio-agent` (test "durable_workers") due to 3 previous errors
```

**Why this is P0:** every "tests pass" claim about the harness so far was produced with per-suite
filters (`--test harness_conductor`), which route around the broken target. `durable_workers` has not
compiled — let alone run — since the harness slice landed. Until H0-1 is closed, the crate's real test
posture is unknown.

**Completion check:** `cargo test -p aelio-agent 2>&1 | grep -c "^error"` returns `0`, and the full
run's per-suite results are recorded in this file's Evidence column.

**CLOSED 2026-08-05.** Restoring the line did exactly what this gate predicted: `durable_workers`
compiled for the first time since the harness slice landed, and **2 of 22 tests failed immediately**.
The Conductor had silently narrowed the procedure-learning surface — see **FLAGS F-024** and
`HARNESS_OS_CHANGE_LOG.md` §2. Both are now fixed and the suite is 23/23. This gate paid for itself;
sequencing it first was correct.

*Note:* a concurrent session applied the identical fix at the same moment, briefly duplicating the
import and breaking the build. See change-log §3 before assuming ownership of edits in this slice.

---

## Gate H1 — Make persistence real

The single most load-bearing gap. The vision doc's §14.3b table asserts programs are "persisted on
`TenantDecl.harness_programs` with active catalog." The type and the write path support that, but in
the running system **nothing has ever written one**.

| ID | Item | Acceptance | Status |
|---|---|---|---|
| H1-1 | Seed starters before the catalog is persisted | A fresh boot writes a catalog whose `harness_programs` contains all 4 starters | [ ] |
| H1-2 | Prove persistence against a real DB, not a temp store | Live `agent/wal.log` contains `harness_programs":{"` with 4 ids | [ ] |
| H1-3 | Admin write path for a single program | `POST` a program; it survives restart and plays | [ ] |
| H1-4 | Correct the vision doc's persistence claim | §14.3b states what is actually stored | [ ] |

### H1-1 instructions

Root cause is ordering in `DurableRuntime::register_catalog` (`runtime/durable.rs:2697-2708`):

```rust
self.upsert(LogicalTable::Catalogs, envelope("active", …, tenant.clone()))?;  // persists FIRST
self.world.replace_tenant(tenant)?;                                           // seeds AFTER
```

`ensure_starter_harness_programs` is called inside `replace_tenant` (`runtime/world.rs:72`), so the
seeded copy only ever exists in memory. The posted catalog — empty — is what reaches disk.

Fix by seeding before the upsert in `register_catalog`:

```rust
validate_catalog(&tenant)?;
let mut tenant = tenant;
crate::runtime::world::ensure_starter_harness_programs(&mut tenant);
// … then upsert, then replace_tenant
```

Two constraints that must not be broken:

- `ensure_starter_harness_programs` uses `.entry().or_insert()` (`world.rs:855`). Keep that — a
  tenant-authored program of the same id must win over the OS seed. Add a test for it.
- `validate_catalog` runs before the seed today. After moving the seed, re-validate after seeding so a
  malformed seed cannot reach storage.

**Completion check:** a test that registers a catalog with `harness_programs: {}`, reads back
`LogicalTable::Catalogs / "active"` from the store, and asserts all 4 ids present with matching
content hashes.

### H1-2 instructions

The existing `stored_quick_reply_survives_catalog_restart` proves the mechanism but **assigns the
library by hand first** (`harness_program_benchmark.rs:151`):

```rust
world.tenant.harness_programs = starter_harness_library();   // the live server never does this
```

That line is why the test passes and production does not. After H1-1, delete it — the test should
prove the seed arrives on its own. Then verify against the actual running instance:

```bash
cd data/aelio-os-demo
grep -aob 'harness_programs":{"' agent/wal.log | wc -l    # must be > 0
```

Note `-a`. Without it grep suppresses output on binary files and silently reports nothing — this
produced a false negative during the original verification pass.

### H1-3 instructions

Until an author can write a program without a rebuild, "saveable" is only half true. Add an admin
route beside the existing catalog route in `AgentApi::router` (`aelio-agent-api/src/lib.rs:230`):

```
POST /v1/admin/harness/:id     → validate → upsert into active catalog → replace_tenant
GET  /v1/admin/harness         → list installed programs with content hashes
```

Reuse `HarnessProgramV1::validate()`. Reject unknown ops via the existing
`#[serde(deny_unknown_fields)]` + tagged enum — an unrecognised `op` must fail the request, never
silently no-op at play time.

**Completion check:** POST a modified `quick_reply`, restart the process, run a turn, and assert the
trace's `Harness.Load … hash=` matches the POSTed program's hash and **not** the code seed's
`e1b94ce8de2b8941074e32203da8f212696c0274609d65e7be3e17b6cdc9bdae`.

### H1-4 instructions

§14.3b currently overstates. Replace the "Seeded starters | yes" row with the distinction: programs
are *code-seeded at boot* and *not yet author-writable* until H1-3. Add a Decision Log entry if the
claim is being amended rather than corrected.

---

## Gate H2 — Mother conformance of the program hash

| ID | Item | Acceptance | Status |
|---|---|---|---|
| H2-1 | `content_hash` uses canonical Sol + BLAKE3 | Same logical program ⇒ stable BLAKE3; no `serde_json` + SHA-256 | [ ] |
| H2-2 | Serialization failure cannot collapse hashes | No `unwrap_or_default()` on the hash input | [ ] |

### H2-1 instructions

`harness/program.rs:137-141` hashes `serde_json::to_vec` output with SHA-256. This is exactly the
pattern already remediated elsewhere as a Mother §4.3 violation — see checklist item **G1-2**
("Adaptive sealing … no longer hashes `serde_json` with SHA-256"). The rest of `aelio-agent` uses
SHA-256 broadly, so this is *consistent with its neighbours* but inconsistent with the Mother rule that
G1-2 established for sealed identities.

Decide explicitly, and record it:

- **(a)** Treat `content_hash` as a sealed artifact identity → convert to canonical Sol + BLAKE3,
  matching G1-2.
- **(b)** Treat it as a non-normative cache/debug fingerprint → keep SHA-256, rename to
  `fingerprint()` so it is never mistaken for a Mother identity, and note it in FLAGS.

**Recommendation → (a).** The hash already appears in persisted turn traces (`Harness.Load … hash=`)
and is asserted across a restart boundary, which makes it an identity in practice regardless of intent.
Deferring means a second migration later against stored trace data.

This needs a FLAGS entry either way — extend **F-023** rather than opening a new flag, since it is the
same slice.

### H2-2 instructions

```rust
let bytes = serde_json::to_vec(self).unwrap_or_default();   // program.rs:138
```

On failure this yields an empty vec, so any two failing programs hash identically. Return
`AelioResult<String>` and propagate, or use a serializer that cannot fail for this type.

---

## Gate H3 — Conductor selection quality

| ID | Item | Acceptance | Status |
|---|---|---|---|
| H3-1 | Tool-call detection derives from the registry, not a phrase list | Registering a tool makes its verbs escalate, with no code edit | [ ] |
| H3-2 | Short factual questions prefer `quick_reply` | `"what is 2+2 in one sentence?"` → `quick_reply` | [ ] |

### H3-1 instructions

`harness/conductor.rs:99` hardcodes:

```rust
const EFFECT_HINTS: &[&str] = &["send otp","login","log in","verify otp",
                                "create job","list jobs","delete","cancel","pay","book"];
```

The live tenant has **31 registered tools**. None of their names participate in selection. A tool named
`refund_invoice` matches no hint and routes to `understand_intent` or `quick_reply` instead of
escalating — the Conductor cannot see the OS's own capabilities.

Replace with derivation over `self.registry` / `tenant.tools`: match against tool ids, names, and
`capability_tags`, preferring effectful-tagged tools. Keep a small hardcoded list only as a fallback
for tenants with an empty registry, and mark it `PROVISIONAL`.

**Completion check:** a test that registers a tool with a novel verb, asserts `Escalate`, then removes
it and asserts the selection changes — with no edit to `conductor.rs`.

### H3-2 instructions

Recorded in §14.3a as the one selection miss from the live trial. `conductor.rs:115-126` triggers
`UnderstandIntent` on `'?' && ("how"|"why"|"which"|"should")` before the short-utterance
`quick_reply` rule is reached. Reorder so a short question with no ambiguity markers takes
`quick_reply`, or require a length floor on the `?` rule.

---

## Gate H4 — Phase C leftovers

| ID | Item | Acceptance | Status |
|---|---|---|---|
| H4-1 | Conductor / `quick_reply` / `understand_intent` system prompts as prompt artifacts | Prompts load from artifacts, not inline format strings | [ ] |

Carried from the vision doc's Phase C, explicitly deferred with *"v0: synthesize evidence strings"*.
Today the `understand_intent` prompt is built inline as a format string, and as of this writing it is
duplicated across **two** sites in `blocks/turn.rs` (~2969 hardcoded lane, ~3207 stored lane) — grep
`"Classify the user's goal"`. Until these are artifacts they cannot be versioned, pinned, or A/B'd —
which the create-harness work in H5 will need. Deduplicate as part of extracting them.

Do H4-1 before H5. Authoring a harness that references an unpinnable prompt just moves the hardcoding.

---

## Gate H5 — Phase F: let Conductor grow its library

The headline feature, and the one most likely to be assumed done. **Nothing here exists.** Vision doc
Phase F is entirely unchecked, and the workspace contains zero runtime writes to `harness_programs`.

| ID | Item | Acceptance | Status |
|---|---|---|---|
| H5-1 | Draft a `HarnessProgramV1` from a successful cold path | Escalate → success → a draft program exists | [ ] |
| H5-2 | Admin promote gate | Drafts never auto-install; promotion is explicit | [ ] |
| H5-3 | Promoted harness is selectable | Next matching utterance loads it instead of `ProposePath` | [ ] |
| H5-4 | HireBoard login as installed child under Conductor | Login runs as a harness, not a cold proposal | [ ] |

### What exists today, and why it is not this

There is a learning loop, but it is a **different mechanism at a different level**:

- `Learn.Observe` accumulates *procedure* proposals; promotion needs 3 observations at ≥0.8 success
  (`abilities/learn.rs:626-627`)
- It produces `PromotedProcedureVersion` in `LogicalTable::Procedures` — not a harness. Not playable
  by Conductor, no stack frame, no context page
- **Effectful paths never auto-promote.** Per the code comment: *"they require tenant approval,
  decision P5, so only deterministic read/express paths warm up on their own"* — which excludes
  precisely the OTP / login / payment cases this gate is about
- In the live DB nothing has promoted at all; the one proposal reads
  `"observations":2 … "state":"accumulating","promoted_version":null`

So H5 is not "wire up the existing loop." It is a new writer.

### H5-1 instructions

On a successful escalate turn, lower the accepted path into a `HarnessProgramV1` draft:

- Source: the `ProposePath` result after `TypeCheck` passes, in `blocks/turn.rs` near the escalate arm
- The current 7 named ops cannot express a tool call. **A `tool.invoke` step op must be added first** —
  design it against Mother §12.4's intent/dispatch/result protocol so a played program dispatches
  effects through the runtime-owned proxy, never directly. This is the real design work in H5; the
  drafting glue is small by comparison
- Store drafts under a distinct status (`"draft"`), never in the active catalog
- Derive a stable id from the situation key, so repeat situations update one draft rather than
  accumulating near-duplicates

**Flag first.** Adding an effectful step op touches locked execution semantics — open a FLAGS entry
with your recommendation before writing it, per the repo's amendment rule.

### H5-2 instructions

Mirror the P5 posture that already governs procedures: an effectful draft requires explicit tenant
approval. Drafts move to the active catalog only via the H1-3 admin route. Auto-install of an effectful
harness is a Mother-level weakening — do not add a config flag that permits it.

### H5-3 instructions

Once promoted, `select_starter_harness` must be able to choose it. This depends on H3-1 — selection
has to consult installed programs and the registry rather than a fixed enum. Note `StarterHarness` is
a closed enum (`conductor.rs:17`); a tenant-installed harness has no variant. Selection will need to
return an id, with the enum reduced to seed defaults.

**Completion check:** run the same effectful utterance twice across a restart. First turn shows
`ProposePath`; after promotion the second shows `Harness.Load` with the promoted id and no
`ProposePath`.

---

## Gate H6 — Delivery path

| ID | Item | Acceptance | Status |
|---|---|---|---|
| H6-1 | Widget WS delivers suspended `wait_for_user` replies | Park → resume completes over the widget, not only the API | [ ] |

From §14.3a: *"widget WS hung only on `wait_for_user` delivery — API path clean."* The Conductor spine
is not implicated; this is the socket delivery path for a suspended turn. Reproduce over
`/v1/sdk` with a park-then-resume script before changing code.

---

## Gate H7 — Housekeeping

| ID | Item | Acceptance | Status |
|---|---|---|---|
| H7-1 | Commit the slice | `harness/`, `harness_conductor.rs`, `harness_program_benchmark.rs`, `docs/architecture/` tracked | [ ] |
| H7-2 | Shut down or document the long-running trial stack | No stale processes, or a documented dev-stack recipe | [ ] |

### H7-1 instructions

The entire harness slice is **untracked**. Per the repo's commit convention, cite sections:

```
feat(agent): harness stack + Conductor per HARNESS_CONDUCTOR_VISION §4/§5, programs §10
```

Land H0-1 in the same commit or before it — do not commit a tree whose test target does not compile.

### H7-2 instructions

Left running from the trials: TS server pid `112324` (:3010, ~53 min CPU) and `aelio-server` on :8090
(restarted mid-verification, so the pid moves). Either stop them or record the launch recipe in
`docs/operations/` so the next session does not re-derive it from `ps`.

---

## Gate HS — Harness bodies → Sol (INCOMPLETE — must complete)

**Authority cross-link:** `docs/architecture/LAYER_BREAKDOWN.md` §P1  
**Status:** open. Owner locked the rule; **implementation not done.**

| ID | Item | Acceptance | Status |
|---|---|---|---|
| HS-1 | Every `HarnessStepV1` has a Sol lowering (or FLAG why not) | Written table in LAYER_BREAKDOWN or annex; no silent orphans | [ ] |
| HS-2 | Choose Path A (sugar+lower) vs Path B (Sol-only authoring) | Decision logged here + FLAGS if Mother touch | [~] Path A preferred (sugar = names); not ratified |
| HS-3 | Persist/play harness body as Sol (or sealed pin) | Starter runs via lowered Sol; Mother-hash identity | [~] **kernel essence demo green** (`sol_harness_store_replay`); Conductor play path still open |
| HS-4 | Migrate four starters | `quick_reply` / `understand_intent` / `wait_for_user` / `memory_attach` green on Sol path | [ ] |

**HS-3 interim evidence (2026-08-04):**  
- Library module: `aelio-kernel::sol_harness_library` — **18 Sol contracts** seeded via `store_library`.  
- Exhaustive ops + harness list: `docs/examples/sol_harness_essence/EXHAUSTIVE_OPS_AND_HARNESSES.md`  
- JSON mirrors: `docs/examples/sol_harness_essence/library/`  
- `cargo test -p aelio-kernel sol_harness` green.  
Conductor play path still open.

Do **not** mark harness “done” in vision prose while HS is open. Current `HarnessStepV1` = temporary sugar.

---

## Suggested order

H0-1 → H1-1 → H1-2 → H7-1 → H2-2 → H2-1 → H1-3 → H1-4 → H3-2 → H3-1 → H4-1 → H6-1 → **HS-1 → HS-2 → HS-3 → HS-4** → H5-*

Rationale: unblock the build, make the "stored" claim true, then get the slice committed before more
churn. H2-2 is a one-line correctness fix worth taking early. **HS** (Sol bodies) before H5 create-harness
so new programs are not authored into a dead-end IR. H5 last — it depends on H1-3 (a write
path), H3-1 (open-ended selection), H4-1 (pinnable prompts), and HS (Sol destination), and it needs a
FLAGS decision on the `tool.invoke` step op before any code.

---

## Verification commands

```bash
# Build posture (must be clean before trusting any suite result)
cargo test -p aelio-agent 2>&1 | grep -E "^(error|test result)"

# Harness suites
cargo test -p aelio-agent --lib --test harness_conductor \
  --test harness_program_benchmark --test golden_traces 2>&1 | grep "^test result"

# What the live server thinks it has (in-memory view — can differ from disk)
curl -sS http://127.0.0.1:8090/agent/v1/catalog \
  -H "Authorization: Bearer $AELIO_RUNTIME_TOKEN" | python3 -c \
  "import json,sys; print(list(json.load(sys.stdin)['harness_programs'].keys()))"

# What is actually on disk — note -a, or binary files report nothing
cd data/aelio-os-demo
grep -aob 'harness_programs":{"' agent/wal.log | wc -l   # populated
grep -aob 'harness_programs":{}'  agent/wal.log | wc -l  # empty
```
