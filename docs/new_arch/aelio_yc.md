# Aelio — YC Technical Brief

**File:** `aelio_yc.md`  
**Purpose:** Full technical explanation for YC partners, application, and diligence  
**Product:** Open-source **agent operating system** (typed runtime + DSL + learning store)  
**One-liner:** Agents that run as programs — LLM teaches the OS new routines; the OS runs them.

---

## 1. What is Aelio?

Aelio is infrastructure for **production AI agents**.

It is not a chatbot. It is not “ChatGPT with tools.” It is an **operating system for agents**:

- a **small kernel** of operations (control flow, compute, effects)
- a **DSL** so product logic is written as pipelines, not prompt soup
- **Sol Contracts** — a shared JSON language so every stage can talk
- **Sunjet** — multimodal durable storage (state, memory, vectors, graph)
- an **LLM** used to *author and repair* logic when something is unknown — then **saved and reused**

**Thesis:** The expensive, creative model should invent *once*. The runtime should execute *forever* after that — with policy, replay, and cost control.

---

## 2. Why this exists (YC problem framing)

### The market problem

Every company wants agents that **log users in, take orders, answer from their data, follow policy**. What they get is demos that:

- forget they were mid-OTP
- double-send SMS or charge twice on retry
- cannot explain why they did something
- cost a frontier call for “Hi”
- break when a JSON field is renamed

### The technical root cause

Today’s stacks make the **LLM the CPU**. Every turn is improvisation. Product logic lives in prose. There is no real:

- instruction set  
- process control (suspend/resume)  
- package install for learned behaviors  
- typed IPC between stages  

### Our answer

Make agent behavior a **programming + learning** problem on a real substrate — the same way databases and kernels made software production-grade.

---

## 3. How it works (full technical picture)

### 3.1 Layer cake

```text
Channels (web, WhatsApp, SDK)
        ↓
Turn runtime
        ↓
Flows / procedures          ← userland programs (DSL)
        ↓
L1 abilities (Call)         ← Sense, Understand, Recall, Invoke, Learn, Express…
        ↓
L0 Kernel                   ← Seq, Branch, Park, Add, Once, …
        ↓
Sol Contracts               ← JSON KV bags between every stage
        ↓
Sunjet                      ← durable truth + conversion graph + procedures
        ↓
LLM (cold path only)        ← propose converters, paths, closed JSON, copy
```

### 3.2 A turn (one user message)

1. **Accept** message + identity + channel  
2. **Hydrate** user state / active flow from Sunjet  
3. **Sense** — build awareness bag (budget, turn #, timings, prior focus, auth state, allowed tools)  
4. **Flow gate** — if waiting on OTP (etc.), **resume that step** (ignore “new intent”)  
5. **Run program** — control tree calling compute / convert / I/O / model / tools  
6. **Reply**  
7. **Persist**  
8. **Park** if waiting on human — free the worker; resume later with policy re-check  

### 3.3 Sol Contracts (IPC)

Every stage reads/writes a **Sol Contract**:

```json
{
  "sol": "1",
  "imprint": "schema.kind.v1",
  "body": { "key": { "k": "data", "v": "..." } }
}
```

- Same envelope everywhere → seamless pipelines  
- Different body shapes OK → **Convert** maps A→B  
- Ops use **pointers** into keys (`target: years_lived`) and **scope** (`this | cell | row | column | all`)

### 3.4 Instruction DSL

Logic is not free text. Every op has a **schema**:

```text
{ op, args, target?, scope?, … }
```

Examples:

```text
Const(v: { age: 34 })
Add(by: 2, target: years_lived, scope: this)
Seq([ … ])
Branch(pred, then, else)
Park(until: { event: "user_reply" })
Call(id: "Invoke.SendOtp", args: { … })
```

Five classes:

| Class | Purpose |
|---|---|
| Control | How the program runs |
| Compute | Pure transforms |
| I/O | DB read/write |
| Model | LLM prompt/parse/synthesize |
| Tool | External APIs + diagnosis |

### 3.5 Where the LLM sits (critical for YC)

| Situation | LLM? |
|---|---|
| Known procedure / converter | **No** — execute graph + kernel |
| Missing Sol→Sol mapping (`active` vs `loggedin`) | **Yes once** — propose rules → typecheck → **save in Sunjet** |
| Novel goal, no path | **Yes** — propose path over **declared** tools/abilities only |
| User-facing copy | Sometimes — Express |
| Policy / OTP send / charge | **Never** — deterministic gates |

**Warm path = cheap and deterministic. Cold path = creative and gated.**

### 3.6 Learning loop (“the system gets life”)

```text
miss → LLM proposes → typecheck → run supervised → Promote
                                              ↓
                                    Sunjet (procedures, converters)
                                              ↓
                         next similar situation → retrieve → run
```

Promote is **slow** (evidence). Demote is **fast** (tool change, bad outcomes). Wrong installed logic is worse than missing logic.

---

## 4. Concrete product story — Login

User: `Hi` while logged out.

1. Sense + load state `unauthenticated`  
2. Check authorised (Compute + Convert if field names don’t match)  
3. Pick **conversational direction** among declared pathways via signals + vector search over stored prototypes:
   - start login  
   - suggest anon actions  
   - entertain + nudge  
   - divert  
4. If start login:

```text
ask phone → wait (Park)
→ validate → SendOtp (Once + Policy)
→ ask OTP → wait
→ verify → set authenticated
→ confirm → next nudge
```

All of that is a **program**, not a hope. OTP wait does not block a server thread. Double SMS prevented by `Once`. Resume re-checks auth policy.

---

## 5. Storage (Sunjet)

Not “Sense is a file.” Sense is RAM. Disk is Sunjet/Astrolobe:

- WAL + `.vss` segments  
- Tables for states, flow instances, sessions, messages, memories  
- Vectors for recall + pathway matching  
- Graph/edges for **conversion glossary** and relationships  

Ops UI already exists to browse tables (`/admin/db`).

---

## 6. Safety model (diligence checklist)

- Closed kernel → finite search space for composition  
- Typed errors (`ReasonCode`) everywhere  
- Nested budgets cannot widen  
- Effects ledgered; `Once` for idempotency  
- Policy wraps tools and state transitions  
- Tool response **signatures** — mismatch never silent-guesses  
- Park + re-auth on resume  
- Tenant isolation as structural goal in the data model  

---

## 7. Why now

1. **Enterprises** need agents that operate workflows, not demos.  
2. **Unit economics** forbid frontier calls for every turn.  
3. **Models** are good enough to *author* typed JSON and converters — if the runtime can *store* them.  
4. **Open source** agent infra is still mostly LangChain-style glue; few ships a real OS contract.  

Timing: models got good at structure; runtimes did not get serious. We build the runtime.

---

## 8. Why us / what’s hard

Hard parts (our work):

- Closed control semantics that still feel expressive  
- Sol conversion graph that stays typed and demotable  
- Turn/Park/resume correctness under policy  
- Multimodal store + learning without silent corruption  
- Keeping LLM power **without** giving it the kernel  

Easy to copy: “we also have agents.” Hard to copy: **contracts + promote/demote + durable converters + Park.**

---

## 9. Status (honest)

| Piece | Reality |
|---|---|
| Architecture | Fully articulated (glossaries + this brief) |
| Rust/TS runtime | Working spine; gaps labeled |
| Sunjet | Real engine; isolation/CAS hardening in backlog |
| Sol Convert | Design proven; production wiring next |
| Pitch | Ready |

We are not claiming a finished Fortune-500 install tomorrow. We are claiming a **clear OS category**, a **build order**, and **technical depth** past prompt wrappers.

---

## 10. Go-to-market shape (technical implication)

Beachhead flows where correctness pays:

- Auth / OTP  
- Support with policy  
- Catalog / rank queries (“best job”, “top product”)  
- Anything mid-wait (Park) + tools  

Deploy: open-source core + hosted Sunjet/runtime for teams that want managed.

---

## 11. The ask (technical)

Build and harden:

1. Control DSL freeze  
2. Sol.Convert in production Sunjet  
3. Tool path: Bind → Policy → Once → Invoke → Sig  
4. Durable Sense + login/direction pathway demo  
5. Golden simulations for pitch and OSS credibility  

---

## 12. YC-style Q&A (technical)

**Q: Isn’t this just Temporal + LLM?**  
A: Orchestration alone has no Sense/Understand/Recall/Learn, no Sol conversion graph, no agent-native Park semantics. We sit above orchestration ideas with an agent ISA.

**Q: Isn’t this LangGraph?**  
A: Graphs of prompts are still model-centric. We invert: **program-centric**, model on cold install.

**Q: Won’t the LLM still be needed always?**  
A: For copy and novel situations, sometimes. For known converters and promoted logins, **no**. That’s the margin.

**Q: What’s the data moat?**  
A: Per-tenant promoted procedures + conversion graphs + pathway embeddings — the “installed software” of the agent.

**Q: Open source kill the company?**  
A: Contract + runtime OSS; value in hosted reliability, tenant ops, vertical pathway packs, and continuous learning infra — classic OS/cloud pattern.

---

## 13. One paragraph for the application

Aelio is an open-source agent operating system: a closed kernel of control/compute/effect ops, a Sol Contract DSL so pipeline stages share typed JSON state, and Sunjet-backed storage for process state, memory, and a learning graph of converters and procedures. The LLM proposes missing translations and paths under typecheck; once promoted, the hot path runs without the model. That yields agents that can follow real product flows (e.g. OTP login with Park/resume and policy), accumulate meaning in the database, and hit production bars for cost, audit, and reliability that prompt-loop agents miss.

---

## 14. Companion docs

- `aelio_tech_doc.md` — long-form pitch tech doc  
- `aelio-dsl-summary-synopsis.md` — design conversation synopsis  
- `AELIO_AGENT_OS.md`, `AELIO_L0_GLOSSARY.md`, `AELIO_L1_GLOSSARY.md` — normative contracts  
- `AELIO_SOL_CONVERSION_*.md` — conversion theory  

---

*Aelio: the OS under agents — LLM as compiler, Kernel as CPU, Sunjet as disk.*
