# Aelio L0-A — Combinators Guide

A learning wrap-up of the S0-A combinator layer: what each Op does, when to use it, what is implemented in Rust today, and how these pieces compose a login-style path.

Companion sources:

- Design dictionary: `docs/new_arch/aelio_dsl_dictionary.md` §1
- Rust evaluator: `Sunjet/Astrolobe/crates/aelio/src/ops/combinators.rs`
- Wrong-OTP fix proposal (related to `Try`): `docs/new_arch/FIX_WRONG_OTP_SIGMISMATCH.md`

---

## 0. Core vocabulary

| Term | Meaning |
|---|---|
| **`Op`** | One executable instruction node in the recipe tree |
| **`Value`** | Data flowing through evaluation (JSON-like: Str, Bool, Int, List, Map, …) |
| **`eval(op, input)`** | Run an Op on a Value → output Value or typed error |
| **Combinator** | An Op whose job is to organize other Ops (`Seq`, `Branch`, `Try`, …) |
| **Leaf** | An Op with no child Ops (`Const`, `Identity`, `Call`, `Park`) |

**Universal rules**

1. Everything returns `Result` with a closed `ReasonCode` — no vague exceptions.
2. Tenants cannot invent new combinators (closed set; keeps tier-2 search finite).
3. Loops, lists, retries, and budgets are always bounded.
4. There is no `Sleep` — only `Park`.

---

## 1. Cheat sheet — combinators you learned

### Leaves & plumbing

| Op | One-liner | Notes |
|---|---|---|
| **`Const(v)`** | Ignore input; return fixed Value `v` | Lifts a literal into Op position |
| **`Identity`** | Return input unchanged | No-op arm / pass-through |
| **`Call{id, args}`** | Run named pure op or ability | Bridge from tree → real work; spends call budget |
| **`Let{bindings, body}`** | Name locals, then run body | Bindings see original input; scope ends after Let |
| **`Tee{body, side}`** | Keep body’s result; run side for effect; discard side result | Side sees body output; side errors soft-ignored today |
| **`Pipe`** | Sugar for `Seq` | Dictionary only |

### Control shape

| Op | One-liner | Notes |
|---|---|---|
| **`Seq[A,B,C]`** | Run in order; thread output → next input | Stops on first Err or Park |
| **`Branch{pred, then, else}`** | If pred bool true → then, else → else | Arms get **original input**, not the bool |
| **`Loop{while, body, max_iter}`** | Repeat while true, hard-capped | Still true after cap → `LoopBudgetExceeded` |
| **`Fallback[A,B,C]`** | First Ok wins on **same** input | Cheap → expensive escalation |
| **`Try{body, catch, finally?}`** | On Err, recover by `ReasonCode` | Unmatched codes bubble |
| **`Guard{invariant, body, on_violation}`** | Fail-closed precondition | False → Err; no else-path |
| **`Budget{body, calls?, tokens?, ms?}`** | Resource ceiling on subtree | Nested budgets **intersect (min), never widen** |
| **`Timeout{body, ms}`** | Wall-clock deadline | **Structural stub** in sync eval today |
| **`Once{body, idem_key}`** | Run effect at most once per key | Dedup marker on replay |
| **`Park{until}`** | End turn; wait for Event / TTL / Instant | Returns `Suspended`; not Sleep |

### List combinators

| Op | One-liner | Notes |
|---|---|---|
| **`Map{body, max_items}`** | Transform each list item | Input must be List; oversized → `BudgetExceeded` |
| **`Filter{pred, max_items}`** | Keep items where pred is true | Pred must be bool per item |

---

## 2. When to use which (decision table)

| You want… | Use |
|---|---|
| Inject a literal / force a test path | `Const` |
| Do nothing / keep data | `Identity` |
| Run named work | `Call` |
| A then B then C | `Seq` |
| If bool then A else B | `Branch` |
| Several named intermediates | `Let` |
| Main result + audit log | `Tee` |
| Typed recovery (`NeedsRepair` → re-ask) | `Try` |
| Cheap strategy, else expensive | `Fallback` |
| Refuse if invariant false | `Guard` |
| Don’t double-send / double-charge | `Once` |
| Wait for next user message / TTL | `Park` |
| Cap calls/tokens/time envelope | `Budget` |
| Cap wall time on one subtree | `Timeout` |
| Repeat while condition (machine-side) | `Loop` |
| Transform / select list items | `Map` / `Filter` |

### Easy mix-ups

| Pair | Difference |
|---|---|
| `Const("SendOtp")` vs `Call("SendOtp")` | String data vs actually running the skill |
| `Branch` vs `Guard` | Else-path vs hard refuse |
| `Try` vs `Fallback` | Recover by reason code vs try alternate strategies |
| `Seq` vs `Fallback` | Thread outputs vs same input to each candidate |
| `Park` vs `Timeout` | Intentional wait vs “took too long” failure |
| `Park` vs `Sleep` | Free the worker vs block a worker (Sleep banned) |
| `Loop` vs human OTP wait | Loop = in-eval repetition; human wait = Park + next turn |

---

## 3. Implementation status (Rust L0 eval)

| Status | Ops |
|---|---|
| **Implemented** | Const, Identity, Call, Seq, Branch, Loop, Try, Guard, Fallback, Tee, Once, Budget (calls/tokens), Map, Filter, Let, Park |
| **Present but weak** | `Timeout` (runs body; does not enforce wall clock yet); Budget `ms` not fully enforced |
| **Durable story beyond L0** | `Once` keys and Park persistence are completed by runtime/storage (idempotency tables, flow instances), not only in-memory `EvalCtx` |

---

## 4. Dictionary-only / not in Rust `Op` yet

These are in `aelio_dsl_dictionary.md` but not (fully) in the current evaluator enum:

| Op | Role | Notes |
|---|---|---|
| **`Par`** | Parallel ops + explicit merge rule | Latency; needs merge discipline |
| **`Switch`** | Multi-way on closed labels | Sugar over nested Branch |
| **`ForEach`** | Per-item body, serial/parallel | Overlaps Map; more effect-oriented |
| **`Retry`** | Re-run with policy | **Idempotent bodies only** |
| **`Race`** | First success; cancel losers | **Pure/read-only only** |
| **`Memo`** | Cache pure results by key/ttl | Optimization |
| **`Debounce`** | Suppress repeated outbound work | Proactive |
| **Collection extras** | Reduce, FlatMap, Any, All, Find, … | Often macros over Map/Filter + pure |

You can build a full interactive login path **without** these. They matter for latency, retries, and richer list processing later.

---

## 5. Worked example — login-shaped path from L0-A

Conceptual Op tree (ids illustrative):

```text
Guard {
  invariant: Call("StateEq.unauthenticated"),
  on_violation: Conflict,
  body: Seq [
    Call("ExtractPhone"),
    Branch {
      pred: Call("HasPhone"),
      then: Seq [
        Guard {
          invariant: Call("Policy.Allow.auth.otp.send"),
          on_violation: Denied,
          body: Once {
            idem_key: "send_otp:user:flow:args_hash",
            body: Tee {
              body: Call("Invoke.SendOtp"),
              side: Call("LedgerAppend.receipt")
            }
          }
        },
        Call("Express.AskOtp"),
        Park { until: Event("user_reply") }
      ],
      else: Seq [
        Call("Express.AskPhone"),
        Park { until: Event("user_reply") }
      ]
    }
  ]
}
```

Later turn (resume / verify):

```text
Guard {
  invariant: Call("Policy.Allow.auth.otp.verify"),
  on_violation: Denied,
  body: Try {
    body: Call("Invoke.VerifyOtp"),
    catch: {
      "needs_repair": Seq [
        Call("Express.AskOtp"),
        Park { until: Event("user_reply") }
      ]
    }
  }
}
```

**Why simulation 6 broke:** `VerifyOtp` returned `SigMismatch` before error classification, so the `needs_repair` catch never fired. See `FIX_WRONG_OTP_SIGMISMATCH.md`.

---

## 6. Escalation pattern (Fallback + Budget)

```text
Budget {
  calls: 3,
  tokens: 400,
  body: Fallback [
    Call("Pure.ExtractPhone"),
    Call("Llm.ExtractPhone"),
    Seq [ Call("Express.AskPhone"), Park { until: Event("user_reply") } ]
  ]
}
```

Order is cost-ascending. Budget stops A+B+C from blowing the turn envelope.

---

## 7. Mental model — three jobs of L0-A

```text
1. Structure   Seq / Branch / Let / Loop / Map / Filter
2. Safety      Guard / Once / Budget / Timeout / Try
3. Time        Park (wait)  ·  Timeout (deadline)
4. Work        Call  (+ Const/Identity for data plumbing)
5. Audit       Tee
6. Escalate    Fallback
```

L0-A does **not** know phones, CRM, or embeddings.  
Those arrive as **named Calls** into L0-B pure ops and L1 abilities.

---

## 8. Grasp checklist

You are solid on L0-A if you can explain:

1. Op vs Value vs eval  
2. Why Const exists (uniform Op slots)  
3. Seq threads; Branch arms see original input  
4. Try vs Fallback vs Branch  
5. Guard vs Branch  
6. Once key design matters as much as Once itself  
7. Park ≠ Sleep  
8. Budget nests by min  
9. Loop requires max_iter  
10. Map/Filter require max_items and list input  

---

## 9. What’s next

**L0-B — Pure operations** (`ops/pure.rs`): Add, strings, regex, GetPath, validate, normalize, hash/redact, …

Then **L0-C — Effects**: Now, Uuid, Random, LedgerAppend, Park (already met), Schedule, …

Then **L1 — Abilities**: Sense, Understand, Bind, Invoke, Express, Recall, Judge, Learn, …

---

## 10. Quick reference card

```text
Const / Identity     set or keep data
Call                 do named work
Seq                  pipeline (and-then)
Branch               if / else
Let                  named locals
Tee                  main + side audit
Try                  recover by ReasonCode
Fallback             alternate strategies (same input)
Guard                fail-closed invariant
Once                 dedup effect by key
Park                 end turn and wait
Budget               cap calls/tokens/ms
Timeout              wall deadline (stub today)
Loop                 bounded while
Map / Filter         list transform / select
```
