# Walkthrough: harness → steps → Sol contract → store → use

One concrete example end-to-end. The **final Sol contract** is what gets stored and replayed.

---

## 0. Human intent (what we want)

> “Add three numbers. If the sum is greater than 10, say the sum is large; otherwise say it is small.”

That is the harness goal.

---

## 1. Break into harness steps (sugar names)

| # | Sugar step | Meaning |
|--:|------------|---------|
| 1 | `seed` | Put `a=7`, `b=5`, `c=3` in the bag |
| 2 | `add_ab` | `partial = a + b` |
| 3 | `add_c` | `sum = partial + c` |
| 4 | `gate` | If `sum > 10` → large message, else → small message |
| 5a | `say_large` | Emit `"sum is large"` |
| 5b | `say_small` | Emit `"sum is small"` |

Reading aid:

```text
seed → add_ab → add_c → gate → (say_large | say_small)
```

---

## 2. Map each sugar step → Sol

| Sugar | Sol op | Notes |
|-------|--------|-------|
| `seed` | `Const` | Sets bag to `{a,b,c}` |
| `add_ab` | `Call` `compute.hold@1` with `fn add` | Result lands at path `partial` |
| `add_c` | `Call` `compute.hold@1` with `fn add` | Result lands at path `sum` |
| `gate` | `Branch` | Pred: `gt(sum, 10)` |
| `say_*` | `Call` `express.say@1` | Result lands at path `msg` |

Outer wrapper: one `Seq` so steps run in order.

---

## 3. Final Sol program (the runnable tree)

This is the App E instruction tree the **kernel** compiles and runs:

```json
{
  "nid": "root",
  "op": "Seq",
  "steps": [
    {
      "nid": "seed",
      "op": "Const",
      "v": { "a": 7, "b": 5, "c": 3 }
    },
    {
      "nid": "add_ab",
      "op": "Call",
      "id": "compute.hold@1",
      "args": {
        "v": {
          "fn": "add",
          "args": [{ "pull": "a" }, { "pull": "b" }]
        }
      },
      "into": "partial"
    },
    {
      "nid": "add_c",
      "op": "Call",
      "id": "compute.hold@1",
      "args": {
        "v": {
          "fn": "add",
          "args": [{ "pull": "partial" }, { "pull": "c" }]
        }
      },
      "into": "sum"
    },
    {
      "nid": "gate",
      "op": "Branch",
      "pred": {
        "fn": "gt",
        "args": [{ "pull": "sum" }, { "lit": 10 }]
      },
      "then": {
        "nid": "say_large",
        "op": "Call",
        "id": "express.say@1",
        "args": { "text": { "lit": "sum is large" } },
        "into": "msg"
      },
      "else": {
        "nid": "say_small",
        "op": "Call",
        "id": "express.say@1",
        "args": { "text": { "lit": "sum is small" } },
        "into": "msg"
      }
    }
  ]
}
```

---

## 4. Final stored Sol **contract** (what sits in the DB)

Not only the program — the **contract envelope** we put in the store table `sol_harness_contracts`:

```json
{
  "id": "harness.calc_sum_gate",
  "kind": "sol_harness",
  "summary": "Add three numbers, branch on whether sum > 10, emit a short status.",
  "program_json": "<stringified Sol program from §3>"
}
```

Pretty form used in docs (program inlined instead of stringified):

```json
{
  "id": "harness.calc_sum_gate",
  "summary": "Add three numbers, branch on whether sum > 10, emit a short status.",
  "program": {
    "nid": "root",
    "op": "Seq",
    "steps": [
      { "nid": "seed", "op": "Const", "v": { "a": 7, "b": 5, "c": 3 } },
      {
        "nid": "add_ab",
        "op": "Call",
        "id": "compute.hold@1",
        "args": {
          "v": { "fn": "add", "args": [{ "pull": "a" }, { "pull": "b" }] }
        },
        "into": "partial"
      },
      {
        "nid": "add_c",
        "op": "Call",
        "id": "compute.hold@1",
        "args": {
          "v": { "fn": "add", "args": [{ "pull": "partial" }, { "pull": "c" }] }
        },
        "into": "sum"
      },
      {
        "nid": "gate",
        "op": "Branch",
        "pred": { "fn": "gt", "args": [{ "pull": "sum" }, { "lit": 10 }] },
        "then": {
          "nid": "say_large",
          "op": "Call",
          "id": "express.say@1",
          "args": { "text": { "lit": "sum is large" } },
          "into": "msg"
        },
        "else": {
          "nid": "say_small",
          "op": "Call",
          "id": "express.say@1",
          "args": { "text": { "lit": "sum is small" } },
          "into": "msg"
        }
      }
    ]
  }
}
```

Canonical file: [`calc_sum_gate.json`](./calc_sum_gate.json)

---

## 5. Store → retrieve → use

```text
put_if_absent(tenant="demo", table="sol_harness_contracts",
              key="harness.calc_sum_gate", value=contract)

get(...) → read program_json
compile(program_json) → Node
Instance::start(empty bag) → Completed
  bag.sum  = 15
  bag.msg  = { "text": "sum is large" }
replay(program, ledger, bag) → same bag_hash
get again → run again → same bag_hash   // reuse
```

Proven by: `cargo test -p aelio-kernel --test sol_harness_store_replay`

---

## Bonus: semantic harness (same pattern)

Steps:

```text
classify → route(Branch) → ack_clear
                         └→ ask → Park → ack
```

Final contract file: [`semantic_ack.json`](./semantic_ack.json)
