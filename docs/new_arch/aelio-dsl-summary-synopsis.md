# Aelio DSL — Architecture Synopsis

**Document:** `aelio-dsl-summary-synopsis.md`  
**Status:** living design synopsis (v0.1)  
**Purpose:** Single narrative of everything discussed for the Aelio agent OS / DSL architecture — how work is broken down, where the LLM enters, how logic is expressed, and how we build and verify it.

**Related docs:**

- [`AELIO_AGENT_OS.md`](./AELIO_AGENT_OS.md)
- [`AELIO_L0_GLOSSARY.md`](./AELIO_L0_GLOSSARY.md)
- [`AELIO_L1_GLOSSARY.md`](./AELIO_L1_GLOSSARY.md)
- [`AELIO_SOL_CONTRACTS_KERNEL.md`](./AELIO_SOL_CONTRACTS_KERNEL.md)
- [`AELIO_SOL_CONVERSION_THEORY_TEST.md`](./AELIO_SOL_CONVERSION_THEORY_TEST.md)
- [`AELIO_SOL_CONVERSION_TYPE_RULES.md`](./AELIO_SOL_CONVERSION_TYPE_RULES.md)
- [`AELIO_L0_KERNEL_COMPOSITION_LAB.md`](./AELIO_L0_KERNEL_COMPOSITION_LAB.md)
- [`AELIO_PRODUCTION_SPINE_BACKLOG.md`](./AELIO_PRODUCTION_SPINE_BACKLOG.md)

---

## 1. Vision in one paragraph

Aelio is an **agent operating system**: a small agnostic kernel of operations, pipelines (flows) composed from those operations, a shared JSON-shaped language (**Sol Contracts**) so stages can talk, durable storage in **Sunjet**, and an **LLM** used to author and repair logic — not to be the unsupervised kernel every turn. The system prefers **retrieve a known routine or conversion**; if missing, **ask the LLM once**, typecheck, **store**, and reuse.

That is the “life” of the system: flows, meanings, and converters accumulate in the database; the runtime executes them deterministically whenever they already exist.

---

## 2. Agent OS metaphor

| Classic OS | Aelio |
|---|---|
| ISA / syscalls | Kernel ops (control + compute primitives) |
| System services | L1 abilities (Sense, Understand, Recall, Bind, Invoke, Learn, Express, …) |
| Userland programs | Flows / procedures (Op trees / DSL programs) |
| Filesystem | Sunjet (tables, text, vectors, edges) |
| Scheduler | Turn loop + `Park` / `Schedule` (no blocking `Sleep`) |
| Package install | Propose → TypeCheck → Promote (routines & Sol converters) |
| Policy / MAC | Policy as executor invariant around effectful tool/state writes |

**Non-goal:** LLM-as-kernel (freeform control of every turn). That breaks replay, policy, cost, and open-source verifiability.

---

## 3. How everything is broken down

### 3.1 Five operation classes

All work is classified so the DSL stays closed and teachable:

| Class | Role | Examples |
|---|---|---|
| **Control** | How the pipeline runs | `Seq`, `Branch`, `Loop`, `Try`, `Fallback`, `Guard`, `Budget`, `Timeout`, `Once`, `Park`, `Let`, `Tee`, `Map`, `Filter`, `Const`, `Identity` |
| **Compute** | Pure transforms on Sol data | `Add`, `Sub`, `Equal`, `Pull`/path get, length, merge, count, validate format, … |
| **I/O** | Read/write durable store | Sense hydrate, `State.Read`, save state, Recall, Remember, conversion graph lookup |
| **Model** | LLM in/out | choose prompt, parse closed JSON, synthesize utterance, repair classification |
| **Tool** | External world + diagnose | `Invoke.Call`, signature extract, classify tool errors |

**Build order (agreed):** finish **Control** scope and its DSL first → then Compute → I/O → Model → Tool, always composing upward.

`Call` is the **bridge**: control trees invoke Compute/I/O/Model/Tool by id without putting domain into the kernel.

### 3.2 Fixed emit vs transform

Every data-facing step is one of:

1. **Fixed emit** — prescribe the output Sol; ignore input (`Const`).
2. **Transform** — interpret input Sol, execute, emit derived Sol (`Add`, `Equal`, most Calls).

Control may also **Err** or **Park** (stop / suspend turn) — not a third data mode.

### 3.3 Per-op machine

Every op runs the same abstract loop:

```text
incoming Sol
  → INPUT INTERPRETER   (digest / pointers / required keys)
  → EXECUTE             (abstract machine for this op)
  → emit Sol  (or Err / Park)
```

Ops are **detachable**: they do not hard-code their neighbor’s private layout. Mismatched shapes are handled by **converters** between them.

---

## 4. Sol Contracts — the shared language

### 4.1 Why Sol

If every pipeline stage speaks the same JSON-shaped contract language, humans and LLMs can read the system, stages plug together, and verification is structural. **Sol Contracts** are that language.

### 4.2 Shape

```json
{
  "sol": "1",
  "imprint": "optional.schema.id",
  "body": {
    "<key>": { "k": "data|var|fn|flow|sol|list", ... }
  }
}
```

| Field | Meaning |
|---|---|
| `sol` | Language version |
| `imprint` | Optional schema/kind label (e.g. `sense.v1`, `user.state.v1`, `person.raw.v1`) |
| `body` | KV working memory — pointers target these keys |

**SolValue kinds:** `data`, `var` (path ref), `fn` (named callable), `flow` (nested pipeline), `sol` (nested contract), `list`.

Plain JSON literals in `body` may be sugar for `data`.

### 4.3 Pipeline speech

```text
Op1  --Sol-->  [Convert?]  --Sol-->  Op2  --Sol-->  …
```

- Universal **envelope** is always Sol (seamless open/close).
- **Body imprints** may differ between ops.
- **Convert** maps Sol_A → Sol_B when shapes differ.

### 4.4 Identity and Const (locked ideas)

- **`Identity`:** in Sol `S` → out `S` unchanged. Proves the wire type.
- **`Const(v)`:** ignore in → mint Sol from `v`. Creates bags (e.g. ingress or test fixtures).

Handoff is not magic: either Const already fulfills the next digest shape, or **Convert** sits between.

---

## 5. Instruction language (DSL schema)

### 5.1 Shared instruction envelope

Every instruction (all classes) uses a predefined schema — not free prose:

```text
{
  op:          string,
  args:        object,           // op-specific
  target?:     path | path[],    // pointer(s) into Sol body
  scope?:      this|cell|row|column|all,
  at?:         { index?, row?, column? },
  on_missing?: error|skip|default,
  default?:    any
}
```

**Scope:**

| Scope | Meaning |
|---|---|
| `this` | This bag, one key/field |
| `cell` | One cell (row index + column / path) |
| `row` | Whole row (imprint-gated which fields) |
| `column` | Named field across list rows |
| `all` | Every element in list scope |

List home convention (to lock): typically `body.items`.

### 5.2 Control instructions (priority catalog)

| Op | Core args |
|---|---|
| `Seq` | `steps[]` |
| `Const` | `v` |
| `Identity` | — |
| `Let` | `bindings`, `body` |
| `Tee` | `body`, `side` |
| `Branch` | `pred`, `then`, `else` |
| `Guard` | `invariant`, `body`, `on_violation` |
| `Loop` | `while`, `body`, **`max_iter`** (mandatory) |
| `Map` / `Filter` | child + `max_items` |
| `Try` | `body`, `catch{ReasonCode→Instruction}`, `finally?` |
| `Fallback` | `steps[]` |
| `Once` | `body`, `idem_key` |
| `Budget` | `body`, `calls?`, `tokens?`, `ms?` |
| `Timeout` | `body`, `ms` |
| `Park` | `until: event \| ttl \| instant` |
| `Call` | `id`, `args` (bridge) |

**Forbidden:** `Sleep`; using `Loop` to wait for humans (use `Park` + next turn).

### 5.3 Pointed compute examples

```text
Add(by: 2, target: years_lived, scope: this)
Equal(left: loggedin, right: true, scope: this)
Pull(from: State, path: registered)
```

Commands carry **pointers** into the KV Sol bag.

---

## 6. Sol conversion — gluing mismatched logic

### 6.1 Problem

Author writes `Equal(Val("loggedin"), …)` but payload has `active`. Without a bridge, the pipeline breaks. With a **sanitiser/converter**, the system learns `active ≡ loggedin` for that edge.

### 6.2 Cold vs warm

```text
mismatch
  → lookup Sunjet conversion graph
  → HIT: apply pure rules (no LLM)
  → MISS: LLM proposes rule list → typecheck → Promote → apply
```

Converters are keyed by imprint pair and/or field type edges, and can be tied to **op-serial / edge id** between two steps so reuse is local and persistent.

### 6.3 Fundamental types and rules

Types: `null | bool | int | float | str | list | map`.

Warm rule ops (closed): `rename`, `drop`, `keep`, `default`, `cast` (allow-matrix), `wrap`, `unwrap`, `map_enum`, `path_copy`, `const_set`.

Examples tested in design:

- `age:int` → rename → `years_lived:int` → `Add(2, …)`
- `sex:str` → `map_enum` → `gender:str` (`Male`→`M`)

### 6.4 Metadata stored per conversion edge

Must include: `conversion_id`, `version`, `tenant_id`, `status`, `from_imprint`, `to_imprint`, `rules`, `type_map`, `extra_key_policy`, `on_parse_fail`, `proposed_by`, `prompt_hash?`, `evidence`, `sensitivity`.

Graph index enables: find translation without LLM when types/imprints match.

---

## 7. Where the LLM comes into the picture

| Moment | LLM role | Not LLM’s role |
|---|---|---|
| **Cold converter miss** | Propose Sol→Sol pure rules | Run as warm converter every time |
| **Cold pathway authoring** | Describe expected histories per conversational direction; embeddings stored | Re-pick pathway with free prose every turn |
| **Model class ops** | Prompted closed JSON / synthesis / parse | Own the scheduler or skip Policy |
| **Learn.ProposePath** | Ordered path over **declared** abilities | Emit arbitrary unbounded code |
| **Error triage (cold)** | Genuine error vs mistaken mapping | Silently invent enum values on warm path |
| **Summaries (optional)** | Produce day/hour/10m digests into Sol | Replace structured state |

**Asymmetry:** expensive/rare to promote; cheap to demote; warm path is deterministic graph + Kernel.

---

## 8. State, Sense, and storage

### 8.1 Message arrival order

```text
message in
  → hydrate durable rows (states, flow_instances, sessions, …)
  → Sense (first awareness Sol)
  → flow gate (pending_step wins over new intent)
  → rest of program
  → persist
```

### 8.2 Sense is not a `.vss` file

- **Sunjet / `.vss` + WAL:** durable row engine on disk.
- **Sense:** named Sol snapshot in RAM for this turn, assembled from durable data + clock + request.

### 8.3 What gets stored where

| Kind | Store |
|---|---|
| Lifecycle (`authenticated` / deployer states) | `states` |
| Mid-flow (`pending_step`, slots) | `flow_instances` |
| Session timing / channel | `sessions` |
| Open loops, facts | `memories` |
| Messages / turns | `messages` / `turns` |
| Conversion edges, pathway prototypes, procedures | Sunjet graph/tables |
| Turn budget | ephemeral envelope |

Sense fields (conceptual): env (now, tz, turn_index, channel, timings), session (open_loops, active_flow, pending_step, last_seen), budget, tenant, plus deployer state & reachable tools.

### 8.4 KV bag vision

Typed, expandable, tenant-scoped KV structures with **imprint/signature**; Sense is one named role. Surgical ops edit keys with strict types. LLM may **propose** new bag kinds/fills; lasting shapes are typechecked and promoted.

---

## 9. Login flow — reverse-engineered hard test

Natural-language flow (design conversation) mapped to classes:

### 9.1 `Hi` → Aware

Control `Seq` of I/O Calls: Sense assemble, State read, reachable tools/policies. Rich awareness Sol (budget, turn thread, timings, prior focus, deployer unauthorized state).

### 9.2 Authorised?

```text
Seq(Pull(State, …), Convert?, Equal(loggedin, true))
Branch → AuthedPath | UnauthedPath
```

Sanitiser/Convert repairs `active` vs `loggedin` on first miss; stores edge in Sunjet.

### 9.3 If unauthorised — conversational direction

Four pathways (declared):

1. Immediately begin login  
2. Suggest anon-capable actions  
3. Entertain and nudge login  
4. Divert topic  

Signals: turn timing urgency, intent, conversation context Sol (day / hour / 10m / 5m / last turn). Pathway prototypes (expected histories) authored with LLM, stored; **vector search** picks highest score; Control branches.

### 9.4 If pathway = start login

```text
AskPhone → Park
→ Normalize/Validate phone
→ Guard+Once+Invoke SendOtp
→ AskOtp → Park
→ Convert/Sanitise OTP
→ Invoke VerifyOtp
→ State transition authenticated
→ Express logged-in
→ Next nudge / flow
```

Classes: Model + Control Park + Compute + Tool + I/O + Convert.

### 9.5 Fairness verdict

The login diagram **can** be represented in the DSL architecture. Gaps to name explicitly: `Pull`/`Equal` schemas, Convert edge ids between ops, Direction pathway registry, `Switch` sugar, conversation context imprint.

---

## 10. How we will build and verify (process)

### 10.1 Design cadence

1. Discuss one unit (op class, Sol rule, or flow fragment).  
2. Decide keep/change/lock.  
3. Write glossary / synopsis.  
4. Only then implement code citing glossary ids.

### 10.2 Assignment method

1. Scene + history + inbound message.  
2. Reverse-engineer jobs.  
3. One job at a time.  
4. Map to Control / Compute / I/O / Model / Tool.  
5. Write instructions + Sol in/out.  
6. Insert Convert where shapes differ.  
7. Verdict: missing op vs wrong class.

### 10.3 Theory tests already run (design)

- Const → convert age→years_lived → Add(2)  
- sex→gender via map_enum  
- Identity Sol pass-through  
- Login flow reverse map  

### 10.4 Implementation waves (from backlog)

**P0:** real Timeout/Budget, durable Sense hydrate, Bind→Policy→Once→Invoke→Sig, Park resume re-check Policy, Promote/Demote, CAS persistence.  
**P1:** tenant isolation, repair registry, Understand/Recall depth, conversion graph production, direction pathways.  
**P2+:** Schedule/Proactive/Observe, aspirational control (`Switch`, `Retry`, …).

---

## 11. Kernel v0 (adopted control+effect core)

**L0-A:** Const, Identity, Call, Seq, Branch, Loop, Try, Fallback, Guard, Budget, Timeout, Once, Park, Let, Tee, Map, Filter  

**L0-C (v0):** Now, Uuid, Random, Park, LedgerAppend  

**L0-B:** category compute (numeric, string, path, list, logic, validate, hash) — detailed DSL after Control is finished  

Domain formatters (`NormalizePhone`, …) = **registered** Call targets, not eternal kernel nouns.

---

## 12. End-to-end mental model

```text
Deployer declares: states, tools, policies, pathways, personalities
        │
User message
        │
Hydrate Sunjet → Sense Sol
        │
Control program (DSL / promoted procedure)
   ├─ Compute on Sol (pointers + scope)
   ├─ Convert Sol↔Sol (graph; LLM cold)
   ├─ I/O load/save
   ├─ Model ask/synthesize/parse (closed JSON)
   └─ Tool invoke (+ Policy, Once, Sig)
        │
Park or complete → persist → next turn
        │
Cold loop: score, attribute, promote converters & paths
```

---

## 13. One-sentence summary

**Aelio DSL is a typed pipeline language over Sol KV contracts: Control wires the program, Compute edits bags by pointer and scope, Convert learns and stores shape bridges in Sunjet, I/O persists and recalls, Model and Tool are gated Calls — and the LLM authors what the OS does not yet know, then steps aside for deterministic reuse.**

---

## 14. Open locks (explicit)

Still to finalize in follow-up discussions:

1. Exact scope enum + list home (`body.items`) as normative.  
2. Full Compute instruction catalog (after Control DSL freeze).  
3. Convert edge id scheme between two ops.  
4. Conversational direction registry + context imprint schema.  
5. Call out merge vs replace policy for Sol namespaces.  
6. Production implementation of Timeout/Budget and durable Sense.

This synopsis is the narrative glue; normative tables live in the companion glossary docs and should be updated when locks are decided.
