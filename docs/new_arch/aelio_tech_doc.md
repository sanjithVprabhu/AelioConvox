# Aelio — Technical Document (Pitch)

**Document:** `aelio_tech_doc.md`
**Audience:** technical founders, investors, partners, and engineering leads
**Version:** 0.1
**Product thesis:** An open-source, production-grade **agent operating system** — not another chatbot wrapper.

---

## Executive summary

Most “AI agents” today are **prompt loops with tools**: every turn the model improvises, cost balloons, behavior drifts, and production failures are hard to replay or audit. Aelio takes a different approach.

**Aelio is a typed agent OS.** It runs agents as **executable pipelines** (flows) over a small, closed instruction set. Stages communicate through a shared JSON-shaped language (**Sol Contracts**). Durable knowledge — state, memory, learned converters, promoted procedures — lives in **Aelio DB**, a multimodal store. The **LLM is a privileged author and cold-path helper**: it proposes routines and translations when the system does not yet know how; after typecheck and promotion, the same work runs **without** calling the model again.

The result is an agent runtime that can:

- **Follow flows reliably** (login, checkout, support, analytics Q&A)
- **Learn and accumulate** procedures and meaning mappings in the database
- **Stay tenant-safe** with policy around every real-world effect
- **Scale economically** by making the hot path deterministic and cheap
- **Ship open-source** with a verifiable contract: glossary → DSL → runtime

One line:

> **Retrieve a known way to finish the job; if missing, ask the LLM once, prove it, store it, reuse it.**

---

## 1. The problem we solve

### 1.1 What breaks in production agent stacks

| Failure mode | Why it happens |
|---|---|
| Amnesia mid-flow | Model re-interprets “ok” / “1234” without pending OTP context |
| Double charges / double SMS | No at-most-once effect primitive; retries are ad hoc |
| Unauditable decisions | Freeform chain-of-thought is not a ledger |
| Cost explosion | Every greeting and every tool path burns an LLM call |
| Silent corruption | Cached JSON extractors applied to the wrong response shape |
| Tenant leakage risk | Shared stores without structural isolation |
| Unmaintainable “prompt soup” | Business logic lives only in prose |

### 1.2 What buyers actually need

- Agents that **run product logic**, not endless chat
- **Deterministic cores** with AI at the edges that invent
- **Replay, policy, and promotion** as first-class concerns
- A **language** engineers and models can both write and verify

Aelio is built for that bar — especially as an **open-source production runtime**.

---

## 2. What we are building

### 2.1 Product definition

**Aelio** = Agent Operating System + DSL + runtime + Aelio DB-backed memory/learning.

| Layer | What it is |
|---|---|
| **Kernel (L0)** | Closed, domain-agnostic operations: control, pure compute, effects |
| **Services (L1)** | Named abilities: Sense, Understand, Recall, Bind, Invoke, Learn, Express, … |
| **Userland** | Flows & procedures — programs stored and retrieved per tenant |
| **Filesystem** | Aelio DB / Aelio DB engine — multimodal rows (columnar, text, vector, graph) |
| **Installer** | Propose → TypeCheck → Promote / Demote for paths and Sol converters |
| **Ingress** | Channels (web, WhatsApp, SDK) — transport only |

### 2.2 What we are *not* building

- A single-vendor chat UI as the core product
- “LLM decides everything every turn”
- Domain syscalls in the kernel (`SendOtp` as a CPU instruction)
- Blocking sleep that holds workers for human wait times

---

## 3. Architecture overview

```text
┌─────────────────────────────────────────────────────────────┐
│  Channels (web / WhatsApp / SDK)                            │
├─────────────────────────────────────────────────────────────┤
│  Turn runtime — hydrate → Sense → execute flow → persist    │
├─────────────────────────────────────────────────────────────┤
│  Userland     Flows / Procedures (DSL / Op trees)           │
├─────────────────────────────────────────────────────────────┤
│  Services     L1 abilities via Call{id}                     │
├─────────────────────────────────────────────────────────────┤
│  Kernel       Control · Compute · Effects (L0)              │
├─────────────────────────────────────────────────────────────┤
│  Sol language Contracts (JSON KV bags) between every stage  │
├─────────────────────────────────────────────────────────────┤
│  Aelio DB       State · memory · messages · converters · procs│
├─────────────────────────────────────────────────────────────┤
│  LLM          Cold author / repair / synthesize (gated)     │
└─────────────────────────────────────────────────────────────┘
```

### 3.1 Agent OS mapping

| OS concept | Aelio |
|---|---|
| ISA | Kernel ops |
| libc / daemons | L1 abilities |
| Processes | Flow instances (pending step, slots, TTL) |
| Files | Memories, documents, messages |
| PATH / packages | Promoted procedures & conversion edges |
| init | Tenant catalog: tools, states, policies, personalities |
| auditd | Decision ledger + LedgerAppend |

---

## 4. How a turn functions (runtime spine)

One user message (or proactive tick) = **one turn**.

```text
1. Ingress          accept message, user_id, channel
2. Hydrate          load states, flow_instances, session, loops from Aelio DB
3. Sense            build awareness Sol (cockpit snapshot)
4. Flow gate        if pending_step → resume flow (outranks new intent)
5. Program          control tree: compute / convert / I/O / model / tool
6. Express          user-visible reply
7. Persist          write state, flow, messages, memories, ledger
8. Park or complete wait for next event without holding a worker
```

**Critical ordering rules:**

1. **Sense before classify** — you cannot interpret “ok” without knowing mid-OTP.
2. **Flow context outranks utterance semantics** — pending step wins.
3. **Policy wraps effects** — composers cannot forget authorization.
4. **Park ≠ Sleep** — human wait ends the turn; resume re-checks policy/state.

---

## 5. Sol Contracts — the shared data language

### 5.1 The insight

Pipelines are seamless when every stage speaks one language. We call that language **Sol Contracts**: JSON-shaped key–value bags with optional schema labels (**imprints**).

```json
{
  "sol": "1",
  "imprint": "user.state.v1",
  "body": {
    "user_id": { "k": "data", "v": "u_42" },
    "state_id": { "k": "data", "v": "unauthenticated" }
  }
}
```

Values may be: **data**, **variable** (path), **function**, **flow**, **nested Sol**, **list**.

### 5.2 How stages talk

```text
Op₁ → Sol → [Convert?] → Op₂ → Sol → …
```

Each op:

1. **Interprets** the incoming Sol into its working view
2. **Executes** its abstract machine
3. **Emits** the next Sol (or Err / Park)

Ops stay **modular** — you can remove or swap a stage; converters glue mismatched shapes.

### 5.3 Two data intents in the DSL

| Intent | Meaning | Example |
|---|---|---|
| **Fixed emit** | Author decides output | `Const({ age: 34 })` |
| **Transform** | Operate on input via pointers | `Add(by: 2, target: years_lived, scope: this)` |

---

## 6. Instruction DSL — how logic is written

### 6.1 Shared instruction envelope

Every op uses a **predefined instruction schema** (not free prose):

```text
{
  op, args,
  target?,          // pointer into Sol body
  scope?,           // this | cell | row | column | all
  at?, on_missing?
}
```

This is how `Add` knows **how much**, **to which key**, and **how wide** (one cell vs whole column vs all rows).

### 6.2 Five classes of operations

| Class | Job | Examples |
|---|---|---|
| **Control** | Structure the program | Seq, Branch, Loop, Try, Guard, Once, Park, Budget |
| **Compute** | Pure data transforms | Add, Equal, Pull/path, length, merge, validate |
| **I/O** | Durable read/write | Sense, State, Recall, Remember, Convert lookup |
| **Model** | LLM-facing | prompt, parse closed JSON, synthesize, repair |
| **Tool** | External APIs | Invoke, Sig match/extract, classify errors |

**Build sequence:** lock Control → Compute → I/O → Model → Tool.

### 6.3 Control catalog (kernel of flow programming)

| Group | Ops |
|---|---|
| Plumbing | `Seq`, `Const`, `Identity`, `Let`, `Tee` |
| Branching | `Branch`, `Guard` (+ `Switch` later) |
| Iteration | `Loop` (mandatory max_iter), `Map`, `Filter` |
| Recovery | `Try` (by ReasonCode), `Fallback` |
| Safety | `Once`, `Budget`, `Timeout` |
| Time | `Park` (Event / TTL / Instant) |

`Call{id, args}` bridges into other classes without polluting the kernel with domain nouns.

---

## 7. Learning and the LLM — where intelligence sits

### 7.1 Hot path vs cold path

| Path | Behavior | LLM? |
|---|---|---|
| **Hot** | Retrieve promoted procedure / conversion; execute Kernel | No (or only Express synthesize when required) |
| **Cold** | Miss → propose → typecheck → supervised run → promote | Yes |

### 7.2 What the LLM is allowed to do

- Propose **Sol→Sol converters** when keys/shapes mismatch (`active` vs `loggedin`)
- Propose **procedure paths** over **declared** abilities only
- Author **pathway prototypes** for conversational direction (stored embeddings)
- Produce **closed JSON** for Understand/parse steps
- **Synthesize** user-facing copy under personality constraints
- Help triage whether a failure is a true error vs a mapping mistake

### 7.3 What the LLM must not do

- Own the turn scheduler
- Bypass Policy on tools/state transitions
- Invent warm-path converters without promotion
- Expand the Kernel opcode set at runtime
- Apply the wrong response extractor on signature mismatch

### 7.4 Conversion graph (meaning accumulation)

When op A emits Sol shape α and op B needs shape β:

```text
lookup graph(α→β)
  HIT  → apply pure rules
  MISS → LLM proposes rules → typecheck → store edge → apply
```

Rules are closed and typed (`rename`, `cast`, `map_enum`, …) over fundamentals: null, bool, int, float, str, list, map.
Each edge stores rich **metadata** (id, version, tenant, type_map, evidence, sensitivity) so the system can **find translations without the LLM** next time — including graph walk (e.g. `age` → `years_lived`).

This is how “the system gets life”: **meanings and glues accumulate in Aelio DB**.

---

## 8. Sense and durable state — awareness without a special DB

### 8.1 Sense

After hydrate, the first job is **become aware**:

- Budget remaining
- Turn index / new vs continuing thread
- Timings (since last message/turn, message timestamp)
- Prior turn focus / summaries
- Deployer state (e.g. `unauthenticated`) and reachable tools/policies

Sense is a **Sol snapshot**, not a Aelio DB file format. Persistence is in tables; Sense **reads**.

### 8.2 Physical storage

aelio-os:

- Catalog + WAL + memtable + **`.vss` segments**
- Typed columns: utf8, i64, text, vector, edge, …
- Logical tables: `states`, `flow_instances`, `sessions`, `memories`, `messages`, `procedures`, conversion edges, …

Admin browser (ops): `/admin/db` on the Aelio server.

### 8.3 Flow instances (process control blocks)

Mid-login persistence includes: `flow_id`, `pending_step`, `slots`, `attempts`, TTL, pinned tool/prompt versions.
Resume always **re-validates policy and state**.

---

## 9. Worked example — Login (pitch narrative)

### 9.1 User says `Hi`

1. **Aware** — Sense + State + reachable capabilities → Sol
2. **Authorised?** — Compute pull/equal on state, with **Convert** if field names differ (`active` ↔ `loggedin`)
3. If **unauthorised** — choose **conversational direction** among declared pathways:
   - Start login immediately
   - Suggest anon actions
   - Entertain + nudge login
   - Divert topic
4. Pathway choice uses **signals** (urgency, turn gaps) + **conversation context Sol** (day / hour / 10m / 5m / last turn) + **vector search** over pathway prototypes authored once with the LLM and stored in Aelio DB

### 9.2 If pathway = start login

```text
Ask phone → Park
→ validate/normalize phone
→ Policy + Once + SendOtp tool
→ Ask OTP → Park
→ sanitise OTP (Convert if needed)
→ VerifyOtp tool
→ State → authenticated
→ logged-in message
→ next nudge / flow
```

Entire path is a **DSL program**: Control wires, Compute validates, Convert glues, Model asks, Tool executes, I/O commits.

### 9.3 Why this wins in a pitch

- Business logic is **inspectable and versionable**
- OTP wait does not block workers
- Double-send prevented by `Once`
- Wrong JSON shape never silently extracts
- Next similar situation can hit a **promoted** path with near-zero LLM cost

---

## 10. Safety, tenancy, and production posture

| Mechanism | Function |
|---|---|
| Closed Kernel | Finite compositional search; tenants cannot invent syscalls |
| ReasonCode totality | No ambiguous exceptions |
| Nested Budget (min) | Learned code cannot grant itself more headroom |
| Once + ledger | At-most-once effects + audit |
| Policy invariant | Effects always authorized |
| Signature registry | Extractors match response shape; mismatch → re-interpret, never guess |
| Promote slow / Demote fast | Asymmetric learning gates |
| Per-tenant storage discipline | Isolation as a structural goal (see data-model backlog) |

Open-source production bar: **glossary is the contract**; implementations cite ids; gaps are labeled (`normative` / `partial` / `aspirational`).

---

## 11. Competitive positioning

| Approach | Limitation | Aelio |
|---|---|---|
| Prompt + tools loop | Non-deterministic product logic | Flows as programs |
| RPA / brittle scripts | No language understanding | Model class + learning |
| Pure orchestration (Temporal-only) | No agent semantics | Sense/Understand/Recall/Learn |
| Hosted agent clouds | Lock-in, opaque prompts | Open contract + self-host Aelio DB |

**Category:** Agent OS / typed agent runtime
**Moat direction:** closed Kernel + Sol conversion graph + promote/demote learning loop on multimodal storage

---

## 12. Implementation status (honest)

| Area | Status |
|---|---|
| Architecture & glossaries | Strong — design locked in docs |
| Rust Kernel combinators | Substantial — Timeout/Budget ms still hardening |
| L1 turn spine | Working spine — Sense/Recall/Learn partial vs full dictionary |
| Sol Convert graph | Design + theory tests — production wiring ongoing |
| Aelio DB | Real engine; tenant/CAS/retention gaps documented |
| Pitch-ready story | This document |

We do not claim “every dictionary verb is shipped.” We claim a **coherent OS thesis** and a **clear path** from Control DSL → full production spine.

---

## 13. Roadmap (how we will build)

1. **Freeze Control DSL** — schemas for Seq/Branch/Loop/Park/…
2. **Compute instruction set** — pointed Add/Equal/Pull with scope
3. **Sol.Convert production** — Aelio DB graph + cold propose
4. **I/O + Sense durability** — real hydrate fields
5. **Tool golden path** — Bind → Policy → Once → Invoke → Sig
6. **Direction pathways** — registry + vector pick for login-style products
7. **Open-source release** — contracts, runtime, simulations, admin DB view

Verification method: **assignment flows** (login, rank-query, save-state) reverse-engineered into DSL and executed in simulations/golden traces.

---

## 14. Why this matters now

Enterprises want agents that **operate their business**, not demos that chat. Regulators and customers want **audit**. Platforms want **unit economics** that do not require a frontier model call for “Hi.”

Aelio’s bet:

> **Make agent behavior a programming and learning problem on a real OS substrate — with the LLM as the compiler that fills gaps, not the CPU that runs forever.**

---

## 15. Closing pitch lines

- **What:** Open-source agent operating system with Sol DSL and Aelio DB memory/learning.
- **How:** Typed pipelines; converters and procedures accumulate in DB; LLM on cold path only.
- **Why now:** Production agents need determinism, policy, and cost control without giving up language intelligence.
- **Ask:** Build and harden the Control→Convert→Tool spine; partner on flagship flows (auth, commerce, support).

---

## Appendix A — Glossary of core terms

| Term | Definition |
|---|---|
| **Sol Contract** | JSON KV bag + optional imprint; stage I/O language |
| **Imprint** | Schema/kind id for a Sol bag |
| **Kernel** | Closed L0 ops |
| **Call** | Bridge to named compute/I/O/model/tool |
| **Convert** | Sol→Sol adapter; graph-stored; LLM-proposed when missing |
| **Park** | End turn and wait; not Sleep |
| **Promote** | Install a procedure or converter after gates |
| **Sense** | Per-turn awareness Sol |
| **Flow instance** | Durable mid-conversation process state |
| **Pathway** | Declared conversational direction for triage |

## Appendix B — Companion documentation

See `docs/new_arch/` for normative glossaries, conversion type rules, production backlog, and this synopsis’s sibling `aelio-dsl-summary-synopsis.md`.

---

*End of technical pitch document.*
