# Harness + Conductor Architecture (Vision Spec)

Status: draft instruction source for validating the “programmable conversation factory” hypothesis.  
Authority for product intent in this thread; Mother-doc / existing pin-lowering remain the install format until an amendment says otherwise.

---

## 1. One-sentence thesis

Conversation is a **stack of named programs (harnesses)** with clear input→output steps. The LLM mostly **selects and navigates** those programs; it does not reinvent operational procedures every turn. Cold ProposePath is a **backup**, not the factory.

---

## 2. Vocabulary

| Term | Meaning |
|------|---------|
| **Harness** | A named conversation/work program: goal, steps, exit rules, what it may call. Playable “level.” |
| **Lowering / pin** | Install packaging + sealed admitted artifact so a harness can be loaded and run by the brain. (Implementation substrate; not a different product idea.) |
| **Building block / reaction / op** | Named unit with permitted input and output (search, shorten, tool call, prompt, compute, memory). Harnesses compose these. |
| **Library / collection** | Group of related harnesses (e.g. calculation, memory, tool-select). |
| **Conductor** | Top harness. Ceiling of the stack. Chooses next harness or final reply. |
| **Context page** | Durable per-harness working context (session/user scoped) that feeds the next decision. |
| **Brain** | Fixed runtime that always hosts the turn: stack, load harness, run ops, call LLM inside bounds. Does **not** author harnesses. |

Harness ≈ what you want to run.  
Lowering/pin ≈ how that program gets installed so the brain can load it.  
Creation of new harnesses is allowed via a gated pipeline (see §7), not unchecked live invention.

---

## 3. Why this exists

- Avoid LLM re-deriving long procedures every turn (hallucination, loops, flaky math/tools).
- Prefer: **name + description → decide to run → get typed result → compose reply text**.
- Example: “add X, 10 000 times” is a named compute reaction, not mental arithmetic in the model.
- Same for tools: choose `send_login_otp`, don’t invent an OTP protocol in prose.

---

## 4. Stack navigation (critical)

### 4.1 Layers

```
Conductor          ← ceiling (top)
   └─ Child A
         └─ Child B   ← deeper allowed
```

- Depth may increase when a harness **decides it needs** another harness mid-flight.
- Upward moves are **one layer at a time** when the user (or harness) expresses clear **exit / go up** intent.
- Ceiling is always **Conductor** — nothing above it.

### 4.2 Waiting child (Park / inline resume)

When a child harness is **waiting** on the user (e.g. “enter the OTP”):

- The next user message is handled **inline by that child**, not re-arbitrated by Conductor first.
- Exception: **clear intent to go one layer up or exit** that harness → pop one level (parent, or Conductor if parent was Conductor’s direct child).
- Think: one active conversational frame until explicitly left.

### 4.3 Return choices

A harness (or user intent interpreted inside it) may:

1. **Return to parent** — pop one layer; parent resumes with child’s output/context.
2. **Fresh / top** — abandon stack context for a clean Conductor loop (“forget and restart at top”).
3. **Finish with reply** — produce user-facing output; Conductor may still wrap or accept as final.

### 4.4 Mid-harness calls

A harness may call another harness in the middle of its own steps. That pushes depth. Return still obeys one-layer-up / fresh / ceiling rules.

---

## 5. Conductor

- Always the **outer program** when no child is waiting.
- Has its **own durable context page**.
- Job: make the next informed decision:
  - reply immediately (e.g. via quick-reply), or
  - delegate to a harness in a library, or
  - escalate / safe stop.
- After a child returns, Conductor chooses again (continue, another delegate, or final reply).

Conductor is **preinstalled**. It is not invented by the LLM at runtime.

---

## 6. Libraries and hierarchy

Harnesses live in collections, for example:

| Library | Role |
|---------|------|
| `conductor` | Top routing only (usually singleton) |
| `intent` / understand | Clarify goal, memory hints, route suggestions |
| `reply` | Quick-reply, tone policies |
| `memory` | Recall / store / shorten-into-context |
| `calculation` / compute | Deterministic or library math/transform ops |
| `tools` | Tool-select and tool-sequence harnesses |
| `meta` | Create-harness, admin promote (later) |

Each harness declares:

- name, description (for selection)
- permitted inputs / outputs
- steps (pseudo-code / diagrammatic flow of ops)
- when to return parent / fresh / finish

---

## 7. Creating new harnesses

Desired path:

1. Search whether an equivalent harness already exists (DB + vector index).
2. If not: construct (steps, I/O, description).
3. Test / sandbox.
4. **v1: admin promote only** → then it joins a library and becomes callable.
5. Later: consider auto-promote; not now.

Rationale: primary goal is **test the hypothesis** with small correct steps, not leap to self-modifying OS.

Live chat must not treat an untested draft as installed law.

---

## 8. Durable context pages

- Per harness, durable for the session (and optionally longer per product rules).
- Lets a harness pick up “where we were” and feed Conductor/child decisions.
- Search (vector/DB) enriches pages; pages are not only ephemeral prompt stuffing.

---

## 9. Default feel vs cold path

**Preinstalled programs** are required for:

1. Default product feel (factory, not improv).
2. Verification that the LLM can make the right **named** calls and ops compose smoothly.

**Cold ProposePath** remains emergency backup when nothing matches — not the main architecture under test.

---

## 10. Building blocks inside a harness (example)

Goal: “get supporting fact into context, then answer briefly.”

Pseudo-flow:

1. `memory.search(query)` → hits or miss  
2. If hit and too large → `context.shorten(hit)`  
3. `context.attach(page, snippet)`  
4. `prompt.quick_reply(page + user)` → short text  
5. `return.finish(text)` or `return.parent(text)`

Each step is a named op with bounded I/O. The harness is the sequence. The LLM may choose *this harness* or fill a slot inside a step — it should not re-derive the sequence from scratch every turn.

---

## 11. Locked decisions (from product discussion)

| Topic | Decision |
|-------|----------|
| Waiting child | User message stays inline with child unless clear up/exit intent |
| Upward move | One layer at a time; ceiling = Conductor |
| Fresh | Allowed: clear stack and restart at Conductor |
| Context pages | Durable per harness |
| New harness publish | Admin promote in first edition |
| Cold path | Backup only for this architecture test |
| v1 create-harness | Deferred until Conductor + starter library prove selection works |

---

## 12. Restatement check

> Every turn runs inside a harness stack capped by Conductor. Waiting children own the next utterance unless the user clearly exits up. Harnesses are named programs composed of named ops, stored in libraries, with durable context pages. New harnesses can be drafted but only join the live library after test + admin promote. The LLM selects and navigates; it does not replace the program library.

---

## 13. Critical assessment (unbiased)

### Will it work?

**Yes, if scoped.** A Conductor + a handful of real harnesses + named ops + stack resume/pop **can** work and will feel more stable than cold ProposePath-as-default.

**No, if unbounded.** “Every possible conversation methodology as a harness library + auto-creation + perfect routing” will stall. Conductor selection is still an LLM (or rules+LLM) bottleneck; bad descriptions = wrong calls; deep stacks without strict exit discipline become spaghetti.

### Does it solve real problems?

**Yes, the right class of problems:**
- tool/protocol hallucination
- math/process loops in the model
- no reuse of known good sequences
- no clear place to put “how we do X here”

**It does not magically solve:**
- bad tool APIs
- ambiguous user intent
- product policy (“should we even do this?”)
- needing humans to author good harnesses/descriptions

### Will people build agentic systems like this?

**Already do, under other names:** workflows, skills, tools, graphs (LangGraph etc.), Temporal, “agents with SOPs.” Your differentiator is treating them as an **OS-ish loadable program library** with admission, stack, durable context pages, and gated creation — not a chat wrapper.

People will adopt this **if** (a) authoring harnesses is tolerable, (b) Conductor routing is reliable enough, (c) the SDK story is simpler than rolling their own graph. They will not adopt it if every integration requires hand-written Sol epics with no templates.

### Is it remotely like an OS?

**In metaphor, yes enough to be useful:**
- programs (harnesses) you install
- syscalls-ish (named ops / tools / memory)
- scheduler/shell (Conductor + stack)
- process resume (waiting child / Park)

**It is not a general-purpose OS.** It is a **conversation/process runtime**. Calling it “agent OS” is fair as product language if the install+run+isolate story stays true; overclaiming (“replaces all agent frameworks / is Linux for AI”) would be hype.

### Bottom line opinion

The architecture is **sound and worth proving**. It is not unique in the abstract, but it is the correct *shape* for operational agents. The bet worth testing: **LLM as navigator over a sealed op/harness library beats LLM as improviser.** If Phase D chat demo fails Conductor selection badly, the idea still isn’t dead — descriptions, rules, or smaller libraries need work — but don’t expand to create-harness until selection works.

---

## 14. Implementation instruction set (do this next)

Goal of this slice: **prove the architecture**, not ship create-harness or HireBoard completeness.

### 14.0 Non-goals (this slice)

- Full HireBoard lowering for every journey  
- Auto-promote / create-harness live  
- Replacing Mother semantics casually  
- Boiling the ocean of libraries  

### 14.1 Implementation choice (v1)

Map harnesses onto **executable flow pins (lowering)** where possible so existing admit/gate/kernel paths stay. Add a thin **stack + context-page + waiting-child resume** control plane in the agent turn path so Conductor/child rules from §4 are enforced in code (not only in prompt hopes).

If a harness cannot yet express something in Sol, keep that step as a **named op** (prompt/tool/compute) invoked from the pin — do not fall back to free ProposePath for the demo paths.

### 14.2 Starter preinstall set (minimum)

| Id | Library | Job |
|----|---------|-----|
| `conductor` | conductor | Choose: quick_reply \| understand_intent \| (demo) login_or_task \| fresh \| finish |
| `understand_intent` | intent | Classify user goal; may search memory; return structured intent + suggested next harness |
| `quick_reply` | reply | Short answer from current context page; finish |
| `memory_attach` | memory | Search → shorten if huge → attach to caller’s context page → return parent |
| `wait_for_user` | control | Ask one question; Park; on resume continue or detect exit-up/fresh |
| (optional demo) `login` or one tool sequence | tools | One real effectful path already drafted in aelio-test-2 |

Each must have: name, description, I/O schema, steps, exit rules (parent / fresh / finish).

### 14.2b Exhaustive library catalog (populate after starter works)

Not all of these ship in Phase B–E. This is the **target inventory** to grow into once Conductor selection is proven. Grouped by library; each entry is a harness (or named op that may later graduate to a harness).

#### `conductor`
- `conductor` — top router (singleton)
- `conductor.escalate` — safe stop / hand to human policy
- `conductor.fresh` — explicit clear-stack restart (may be op inside conductor)

#### `intent`
- `understand_intent` — classify goal + suggest next harness
- `clarify_slot` — ask for one missing field; Park
- `detect_stack_control` — stay | exit_up | fresh (rules + optional LLM)
- `split_multi_intent` — detect multiple asks; queue vs sequential
- `confirm_effect` — yes/no before write/external

#### `reply`
- `quick_reply` — shortest useful answer
- `full_reply` — longer grounded answer
- `apologize_closed` — fail-closed user message
- `summarize_thread` — compress recent page into brief
- `tone_rewrite` — rewrite draft under voice policy

#### `memory`
- `memory_search` — vector/DB search
- `memory_attach` — search → shorten → attach to page
- `memory_store_explicit` — user said “remember …”
- `memory_forget_explicit` — user said “forget …”
- `memory_list_open_loops` — deferred intents
- `context_shorten` — shrink blob for page budget
- `context_pin_fact` — pin a fact onto Conductor page

#### `control` (stack / wait)
- `wait_for_user` — ask + Park
- `return_parent` — pop one layer with payload
- `return_fresh` — clear to Conductor
- `delegate_harness` — push named child
- `timeout_escape` — TTL / max_attempts exit

#### `tools`
- `select_tool` — choose one tool from catalog by intent
- `run_tool_once` — single invocation + normalize output
- `tool_sequence` — ordered multi-tool path
- `login` / `auth.otp` — OTP send/verify program
- `crud_*` — domain CRUD sequences (app-supplied; HireBoard jobs, etc.)

#### `calculation` / `compute`
- `calc_express` — evaluate declared expression
- `calc_aggregate` — sum/count/avg over list
- `calc_repeat_add` — deterministic repeat (your 10k example)
- `format_number` / `format_date` / `format_phone`
- `parse_slots` — extract typed slots from utterance (phone, otp, ids)
- `time_of_day_context` — attach local/business-hours context

#### `policy` / `safety`
- `policy_check` — hard/soft policy before effect
- `redact_pii` — strip before prompt/log
- `rate_limit_notice` — user-facing throttle message

#### `meta` (after Phase E)
- `create_harness_draft` — search existing → draft steps/I/O/description
- `create_harness_test` — sandbox cases
- `create_harness_promote` — **admin only** in v1
- `list_library` — what harnesses exist (for Conductor grounding)

#### App-specific (not OS defaults; registered by SDK)
- HireBoard: login, list_jobs, create_job, applicant pipeline, candidate tracker, …
- Any tenant: their own library entries with lowering

**Rule:** OS ships `conductor` + cross-cutting libraries; tenants ship domain libraries. Exhaustive does **not** mean implement all before the live trial — it means we know where new programs go when the starter works.

### 14.3 Work phases (execute in order)

**Phase A — Spec lock**  
- [x] This document  
- [x] Product owner confirmed direction; exhaustive catalog added (§14.2b)  
- [x] FLAGS F-023 opened for harness stack vs Mother session shape  

**Phase B — Runtime spine**  
- [x] Session **harness stack** types + `detect_stack_control` (`aelio-agent/src/harness`)  
- [x] Persist `LogicalTable::HarnessSessions` hydrate/persist  
- [x] **Waiting-child** gate: exit_up/fresh abandon resume before subject wake  
- [x] **Durable context page** map on `HarnessSession`  
- [x] Wire Conductor as default when stack empty (preinstall + select)  
- [x] Keep cold ProposePath only if Conductor explicitly escalates  

**Phase C — Preinstall artifacts**  
- [x] Author starter harnesses as admitted pins/ops *(v1: `HarnessProgramV1` §10 IR in catalog `harness_programs`; hardcoded `exec_*` kept as benchmark control only)*  
- [x] Seed/embed descriptions for selection *(rule-first `select_starter_harness` + seeded program descriptions)*  
- [x] System prompts / program bodies stored as saveable artifacts *(starter library JSON in `TenantDecl.harness_programs`; PromptArtifact mint still deferred for richer slots)*  

**Phase D — Automated tests**  
- [x] Stack: push child → user message goes to child  
- [x] Exit-up: one layer only  
- [x] Fresh: clears to Conductor  
- [x] Context page survives Park/resume  
- [x] Conductor selects `quick_reply` vs `understand_intent` on scripted utterances (provider fixture or live)  
- [x] Stored vs hardcoded benchmark (`tests/harness_program_benchmark.rs`)  
- [x] Restart retrieve: `quick_reply` content hash survives catalog reload  

**Phase E — Live trial**  
- [x] Local stack: Rust + TS + chat UI *(API trial on rebuilt release; widget WS hung only on wait_for_user delivery — API path clean)*  
- [x] Script: greeting → short Q (quick_reply) → task needing clarify (understand_intent) → wait_for_user → resume → fresh  
- [x] Record traces: which harness ran each turn; confirm cold ProposePath not used on those paths  
- [x] Judgment: selection quality good enough to continue? **Yes** — proceed to Phase F carefully; refine rules so short factual Qs prefer `quick_reply` over `understand_intent`  

### 14.3a Live trial notes (2026-08-04)

User id `conductor-phase-e-*` via `POST /agent/v1/turns`:

| Utterance | Harness | ProposePath? | Notes |
|---|---|---|---|
| hello | quick_reply | no | 34ms template |
| what is 2+2 in one sentence? | understand_intent | no | `?` rule over-triggered; still no cold path |
| help me … hiring pipeline | understand_intent | no | correct |
| please ask me what you need | wait_for_user | no | suspended + waiting child |
| I need a short summary… | wait_for_user.resume | no | context page notes retained |
| start over | fresh → quick_reply | no | stack cleared then Conductor |

Verdict: architecture proves out. Widget path needs a follow-up for suspended wait replies; not a Conductor spine failure.

### 14.3b Stored program proof (2026-08-04)

Format: `HarnessProgramV1` (vision §10 named ops), persisted on `TenantDecl.harness_programs` with active catalog.

| Check | Result |
|---|---|
| Seeded starters (`quick_reply`, `understand_intent`, `wait_for_user`, `memory_attach`) | yes |
| Default play mode | **Stored** (`Harness.Load` + `Harness.Op`) |
| Hardcoded lane | `HarnessPlayMode::Hardcoded` for A/B only |
| Benchmark control-plane parity | pass (`stored_vs_hardcoded_control_plane_parity`) |
| Restart retrieve same content hash | pass (`stored_quick_reply_survives_catalog_restart`) |

Pseudo-language is now **saveable → retrievable → playable**, not only hardcoded Rust.

**Phase F — Only after E passes**  
- [ ] Create-harness draft + test + **admin promote**  
- [ ] Expand libraries  
- [ ] HireBoard login as installed child under Conductor  

### 14.4 Definition of done for “does this arch work?”

All must be true:

1. Empty stack → Conductor runs.  
2. Waiting child owns next message until exit-up/fresh.  
3. At least two non-conductor harnesses selectable by description.  
4. One Park/resume path uses durable context page.  
5. Demo chat paths do **not** depend on Tier3 ProposePath.  
6. Written live-trial notes: what worked / what failed / whether to proceed.

### 14.5 Immediate next coding actions

1. Confirm this file (§12–§14).  
2. Implement stack + waiting-child + context pages against DurableRuntime turn entry.  
3. Preinstall Conductor + quick_reply + understand_intent (+ wait_for_user).  
4. Tests in Phase D.  
5. Live trial Phase E; stop and review before create-harness.

---

## 15. Open questions (non-blocking)

Resolved enough to build. Remaining optional:

- Exact exit-up classifier (rules vs small LLM op) — implement as named op `intent.detect_stack_control`.  
- Session TTL for context pages.  
- Whether Conductor itself is Sol pin or privileged runtime harness with prompt — prefer pin for uniformity if feasible.
