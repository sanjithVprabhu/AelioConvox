# Sol Conversion Theory — Worked Test

**Status:** theory test (v0.1)
**Claim:** KV Sol bag + pointed ops + Aelio DB conversion graph + LLM only to install new edges.

---

## Setup

**Working memory:** Sol Contract (KV bag).
**Ops:** FixedEmit (`Const`) vs Transform (`Add` via Call / L0-B).
**Bridge:** converter Sol_A → Sol_B, stored as a **graph edge** in Aelio DB when promoted.

---

## Run 1 — cold path (no converter in Aelio DB yet)

### Step 1 — Fixed emit

```text
Const({ age: 34 })
```

**Out Sol_A:**

```json
{
  "sol": "1",
  "imprint": "person.raw.v1",
  "body": {
    "age": { "k": "data", "v": 34 }
  }
}
```

### Step 2 — Next op wants `years_lived`

Next machine (e.g. `Add`) **requires** pointer `years_lived` (imprint `person.norm.v1`).

Interpreter: Sol_A has `age`, not `years_lived` → **shape mismatch**.

### Step 3 — Lookup conversion graph

```text
Aelio DB query: edge?
  from_imprint: person.raw.v1
  to_imprint:   person.norm.v1
  OR field map: age → years_lived
→ MISS
```

### Step 4 — LLM proposes converter (cold only)

Proposed mapping (closed JSON):

```json
{
  "from": "person.raw.v1",
  "to": "person.norm.v1",
  "rules": [
    { "from_key": "age", "to_key": "years_lived", "op": "rename", "type": "Int→Int" }
  ]
}
```

**Typecheck:** Int→Int OK → **Promote** to Aelio DB as graph edge (not re-ask next boot).

### Step 5 — Apply converter

```text
Sol_A  --convert-->  Sol_B
```

**Out Sol_B:**

```json
{
  "sol": "1",
  "imprint": "person.norm.v1",
  "body": {
    "years_lived": { "k": "data", "v": 34 }
  }
}
```

### Step 6 — Transform with pointer

```text
Add(2, years_lived, scope=this)
```

Digest: read `body.years_lived` → 34
Execute: 34 + 2 → 36
Emit:

```json
{
  "sol": "1",
  "imprint": "person.norm.v1",
  "body": {
    "years_lived": { "k": "data", "v": 36 }
  }
}
```

**Job finished.**

---

## Run 2 — warm path (edge already in Aelio DB)

Same DSL. Step 3 **HIT** on graph → skip LLM → convert → Add → `{ years_lived: 36 }`.

That is the theory: **LLM installs; graph reuses.**

---

## What is stored in Aelio DB (conversion edge)

Conceptual row / graph record:

```text
conversion_id:  conv.age_to_years_lived.v1
from_imprint:   person.raw.v1
to_imprint:     person.norm.v1
rules:          [{ from_key: age, to_key: years_lived, op: rename, type: Int→Int }]
status:         promoted
evidence:       { observations, last_success }
```

Optional field-level edge for search:

```text
(age:Int) --rename--> (years_lived:Int)  [via conv.age_to_years_lived.v1]
```

---

## Verdict

| Claim | Result in this test |
|---|---|
| Const fixed-emits a KV Sol | pass |
| Next op needs different keys | pass (mismatch detected) |
| Converter Sol→Sol | pass |
| Miss → LLM → typecheck → save | pass (by design) |
| Hit → no LLM | pass |
| Add uses pointer into bag | pass |
| Types checked | pass (Int→Int) |

**Theory holds** for this minimal pipeline.

---

## Failure cases to keep honest

| Case | Expected |
|---|---|
| LLM proposes `age` → `years_lived` as Str | Typecheck fail → no promote |
| Add pointer missing after convert | `Missing` / NeedsRepair |
| Loop without max_iter | illegal Kernel |
| Converter mutates meaning wrong under traffic | Demote edge |

---

## Mapping to Kernel (no new domain ISA)

```text
Seq [
  Const({ age: 34 }),
  Call("Sol.Convert", { from: "person.raw.v1", to: "person.norm.v1" }),
  Call("Add", { by: 2, path: "years_lived" })
]
```

`Sol.Convert` = registry Call that **loads promoted rules from Aelio DB** (LLM only inside Propose when miss).
