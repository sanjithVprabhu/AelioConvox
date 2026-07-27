# Sol Contracts — Kernel Op I/O Spec

**Status:** draft living spec (v0.1) — discuss & amend per op  
**Depends on:** Kernel v0 ([`AELIO_L0_GLOSSARY.md`](./AELIO_L0_GLOSSARY.md) §4), Sol Contract language (design conversation)  
**Rule:** Opₙ emits a Sol Contract; Opₙ₊₁ consumes it. Control ops may wrap child Ops; data still moves as Sol Contracts.

---

## 0. Shared Sol language

### 0.1 Sol Contract

A Sol Contract is a JSON-shaped object:

```json
{
  "sol": "1",
  "imprint": "optional.schema.id",
  "body": { }
}
```

| Field | Meaning |
|---|---|
| `sol` | Sol language version |
| `imprint` | optional schema/signature id for reuse & validation |
| `body` | key → **SolValue** map (the working bag) |

For Kernel plumbing, many ops treat the **entire threaded value** as either:

- a full Sol Contract `{ sol, imprint?, body }`, or  
- a **bare `body` map** (normalized to a Sol Contract at Call boundaries).

**Normative for Kernel v0:** evaluator threads a `Value`. When we say Sol Contract, we mean that `Value` interpreted as:

```text
if Map with key "body" (and optional sol/imprint) → Sol Contract
else → Sol Contract { sol:"1", body: wrap(value) }
```

`wrap`: non-map values become `{ "_": value }` so every stage still speaks KV.

### 0.2 SolValue (tagged kinds)

Every value in `body` is one of:

| Tag | JSON shape | Meaning |
|---|---|---|
| `data` | `{ "k": "data", "v": <any JSON> }` | literal |
| `var` | `{ "k": "var", "path": "a.b" }` | reference into this or outer bag |
| `fn` | `{ "k": "fn", "id": "Add", "args": {} }` | named pure/ability (Call target) |
| `flow` | `{ "k": "flow", "ref": "proc:…", "op"?: … }` | nested pipeline / procedure ref |
| `sol` | `{ "k": "sol", "contract": { … } }` | nested Sol Contract |
| `list` | `{ "k": "list", "items": [ SolValue… ] }` | array of SolValues |

**Sugar:** plain JSON literals in `body` may be treated as `{ k:"data", v: literal }` until an imprint requires tags.

### 0.3 Fulfillment

- **Fulfill** = body contains required keys with correct kinds/types.  
- Missing → `NeedsRepair` / `Missing`.  
- Wrong kind/type → `TypeViolation`.  
- Ops document **requires** (in) and **ensures** (out).

---

## 1. Kernel L0-A — Sol contracts per op

### 1.1 `Identity` — LOCKED

| | |
|---|---|
| **Job** | Pass Sol Contract through unchanged (proves the generic wire type). |
| **Requires (in)** | valid Sol Contract `S` = `{ sol, imprint?, body }` |
| **Ensures (out)** | `S' == S` (no key add/drop; imprint unchanged) |
| **Fails** | not a Sol Contract → `TypeViolation` |
| **Talks to next** | Next op receives exactly `S` |

```text
eval(Identity, S) → Ok(S)
```

**Imprint:** optional schema label; Identity ignores its meaning.  
**Body keys:** whatever the pipeline already placed; Identity does not require a fixed key set.

**Example in → out:** identical Sol Contract (see design conversation test with `turn.ingress.v1`).

---

### 1.2 `Const(v)` — LOCKED (draft below; confirm in conversation)

| | |
|---|---|
| **Job** | Ignore input; **mint** a fixed Sol Contract from `v`. |
| **Requires (in)** | anything (discarded) — may be missing/empty |
| **Ensures (out)** | new Sol Contract built from `v` |
| **Fails** | if `v` cannot be normalized to Sol → `TypeViolation` |
| **Talks to next** | Downstream only sees the constant bag, not prior stage |

**Normalization `sol_from(v)`:**

| `v` | Out |
|---|---|
| already Sol `{ sol, body }` | use as-is (optionally force `sol: "1"`) |
| plain map `{ k: … }` | `{ "sol":"1", "body": tag_as_data(map) }` |
| scalar / list | `{ "sol":"1", "body": { "_": { "k":"data", "v": v } } }` |

```text
in:  <ignored>
out: sol_from(v)
```

---

### 1.3 `Call{id, args}`

| | |
|---|---|
| **Job** | Invoke named `fn` / ability; bridge Kernel → registry. |
| **Requires (in)** | Sol Contract `S`; `args` map (static and/or var paths into `S.body`) |
| **Ensures (out)** | Sol Contract from callee return (wrapped if bare) |
| **Fails** | `NotFound`, callee `ReasonCode`s, `BudgetExceeded` (calls) |
| **Talks to next** | Downstream sees **result bag**, not the pre-Call bag (unless callee merges) |

```text
in:  S
resolve args from S.body + literal args
invoke id(args, S)
out: sol_from(result)
```

**Convention:** prefer callees that return maps so imprint chaining stays clean.

---

### 1.4 `Seq[A, B, …]`

| | |
|---|---|
| **Job** | Pipeline wire: out of n → in of n+1. |
| **Requires (in)** | Sol Contract accepted by first child |
| **Ensures (out)** | Sol Contract ensured by last successful child |
| **Stops** | first `Err` or `Park` (Suspended) |
| **Talks** | **This is the primary Sol-to-Sol conversation between ops** |

```text
S0 = in
S1 = eval(A, S0)
S2 = eval(B, S1)
…
out = Sk
```

---

### 1.5 `Branch{pred, then, else}`

| | |
|---|---|
| **Job** | Soft fork on boolean. |
| **Requires (in)** | Sol Contract `S` |
| **Pred** | `eval(pred, S)` → body bool or `data` bool (or `{ "_": bool }`) |
| **Arms** | **both see original `S`**, not the bool |
| **Ensures (out)** | out of chosen arm |
| **Fails** | pred not bool → `TypeViolation` |

```text
b = as_bool(eval(pred, S))
out = b ? eval(then, S) : eval(else, S)
```

---

### 1.6 `Let{bindings, body}`

| | |
|---|---|
| **Job** | Bind named SolValues into scope; run body. |
| **Requires (in)** | Sol Contract `S` |
| **Bindings** | each name ← `eval(op, S)` (sees original `S`, not sibling bindings — Kernel rule today) |
| **Ensures (out)** | out of `body`; locals available to Call/var resolution inside body |
| **Talks** | Enriches interpretation context without necessarily mutating `S.body` |

```text
locals[name] = eval(bindingOp, S)   // for each
out = eval(body, S)                 // with locals in EvalCtx
```

**Amendment candidate:** optionally merge bindings into `S.body` for stricter Sol-only threading — discuss later.

---

### 1.7 `Tee{body, side}`

| | |
|---|---|
| **Job** | Main result + side effect; side result discarded. |
| **Requires (in)** | `S` |
| **Flow** | `R = eval(body, S)` then `eval(side, R)` (side sees **body out**) |
| **Ensures (out)** | `R` (main Sol Contract) |
| **Side errors** | soft-ignore or policy — document implementation choice |

```text
R = eval(body, S)
_ = eval(side, R)   // LedgerAppend, metrics, …
out = R
```

---

### 1.8 `Loop{while, body, max_iter}`

| | |
|---|---|
| **Job** | In-eval repetition with hard cap. |
| **Requires (in)** | `S0` |
| **Each iter** | if `as_bool(eval(while, Si))` then `Si+1 = eval(body, Si)` |
| **Ensures (out)** | last `Si` |
| **Fails** | still true after `max_iter` → `LoopBudgetExceeded` |
| **Forbidden** | using Loop to wait on humans — use `Park` |

---

### 1.9 `Try{body, catch, finally?}`

| | |
|---|---|
| **Job** | On Err, recover by `ReasonCode` key. |
| **Requires (in)** | `S` |
| **Success** | out = body out |
| **Catch** | `eval(catch[code], S)` — **same original `S`** (recovery sees pre-failure contract) |
| **Finally** | if present, run after success or catch; does not replace out unless defined |
| **Ensures** | recovered Sol Contract or bubbled Err |

---

### 1.10 `Fallback[A, B, …]`

| | |
|---|---|
| **Job** | Alternate strategies; **same input** `S` to each until Ok. |
| **Requires (in)** | `S` |
| **Ensures (out)** | first Ok child’s Sol Contract |
| **Fails** | all Err → last error |
| **vs Try** | Fallback = strategy ladder; Try = reason-keyed repair |

---

### 1.11 `Guard{invariant, body, on_violation}`

| | |
|---|---|
| **Job** | Fail-closed gate. |
| **Requires (in)** | `S` |
| **Invariant** | `as_bool(eval(invariant, S))` |
| **If false** | `Err(on_violation)` — no else arm |
| **If true** | `out = eval(body, S)` |
| **Ensures** | body’s Sol Contract or typed deny |

---

### 1.12 `Budget{body, calls?, tokens?, ms?}`

| | |
|---|---|
| **Job** | Resource ceiling on subtree; nest by **min**. |
| **Requires (in)** | `S` |
| **Ensures (out)** | body’s out if within budget |
| **Fails** | `BudgetExceeded` |
| **Sol effect** | may write budget leftovers into out body under `_budget` (optional convention) |

---

### 1.13 `Timeout{body, ms}`

| | |
|---|---|
| **Job** | Wall-clock deadline on subtree. |
| **Requires (in)** | `S` |
| **Ensures (out)** | body’s out if finished in time |
| **Fails** | `Timeout` |
| **vs Park** | Timeout = failure; Park = intentional wait |

---

### 1.14 `Once{body, idem_key}`

| | |
|---|---|
| **Job** | At-most-once effect for key. |
| **Requires (in)** | `S`; `idem_key` string (may include vars from `S`) |
| **First** | `out = eval(body, S)`; record key → out |
| **Replay** | return recorded Sol Contract |
| **Ensures** | same typed out on duplicate |

---

### 1.15 `Park{until}`

| | |
|---|---|
| **Job** | End turn; wait Event / Ttl / Instant. |
| **Requires (in)** | `S` (often persisted as flow slots separately) |
| **Ensures (out)** | `Suspended` — **no normal Sol out to next Seq sibling this turn** |
| **Talks** | Next **turn** hydrates durable state; not next Op in same Seq after Park |

```text
in: S
persist what runtime needs from S
suspend until …
// Seq after Park does not run this turn
```

---

### 1.16 `Map{body, max_items}`

| | |
|---|---|
| **Job** | Per-element transform. |
| **Requires (in)** | Sol Contract whose body has a **list** (convention: `body.items` as `list`/`data` array, or whole value is List wrapped) |
| **Each** | `eval(body, sol_from(element))` |
| **Ensures (out)** | Sol Contract `{ body: { items: [ …outs ] } }` |
| **Fails** | not list / over `max_items` → `BudgetExceeded` |

---

### 1.17 `Filter{pred, max_items}`

| | |
|---|---|
| **Job** | Keep elements where pred bool true. |
| **Requires (in)** | list-shaped Sol Contract (same convention as Map) |
| **Ensures (out)** | Sol Contract with filtered `items` |
| **Fails** | pred non-bool per item → `TypeViolation`; oversize → `BudgetExceeded` |

---

## 2. Kernel L0-C — Sol contracts (effects)

Effects still take/return Sol Contracts; additionally **ledger** the nondeterminism.

| Op | Requires (in) | Ensures (out) |
|---|---|---|
| `Now` | any / empty | body adds `now: data(Instant)` or returns `{ "_": instant }` |
| `Uuid` | any / empty | `id: data(uuid)` |
| `Random{seed?}` | optional seed in body | `random: data(float)` |
| `LedgerAppend` | `kind` + `payload` (from args/body) | `receipt: data(…)` |
| `Park` | see §1.15 | Suspended |

---

## 3. How two ops “talk” (canonical)

```text
Seq [
  Const({ "user_id": "u1", "utterance": "…" }),   // out: S0
  Call("Sense.Env"),                             // in S0 → out S1 (env keys merged or replaced by callee)
  Call("Sense.Session")                          // in S1 → out S2
]
```

Better Sense pattern (merge-preserving callees):

```text
Seq [
  Call("Sense.Assemble")   // single Call that fulfills imprint "sense.v1"
]
```

Imprint example `sense.v1` required keys: `now`, `channel`, `turn_index`, `active_flow`, `pending_step`, …

---

## 4. Open discussion points

1. **Merge vs replace** on Call out — recommend callees **merge** into input body under namespaces (`sense`, `result`) unless imprint says replace.  
2. **Let** mutates locals only vs writes into Sol body.  
3. **Plain JSON sugar** vs mandatory SolValue tags everywhere.  
4. **Park** persistence: which keys of `S` are written to `flow_instances.slots`.

---

## 5. Acceptance checklist (per op)

When we “lock” an op:

- [ ] requires / ensures written  
- [ ] error codes listed  
- [ ] neighbor behavior under `Seq` clear  
- [ ] no domain nouns in the contract  

**Locked so far in conversation intent:** Const, Identity, Call, Seq (wire) — review this file and confirm or amend.
