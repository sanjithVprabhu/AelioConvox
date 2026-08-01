# L0 Kernel Composition Lab

**Purpose:** Strengthen the Kernel v0 glossary by pairing building blocks and recording what **meaningful higher operation** emerges — without inventing domain ops.
**Method:** For each pair `(A, B)`, ask: wrap? sequence? invalid? already named sugar? gap?
**Status:** Wave 1 complete (structural pairs). Further waves discuss with design owner before glossary amendments.

Kernel v0 set: see `AELIO_L0_GLOSSARY.md` §4.

---

## Experiment protocol

1. Pick two Kernel ops `A`, `B`.
2. Consider shapes: `A(B)`, `B(A)`, `Seq[A,B]`, `Seq[B,A]` (when types allow).
3. Name the **emergent meaning** in agnostic English.
4. Classify outcome:
   - **covered** — expressible; no new op needed
   - **sugar** — deserves a named L2/macro alias later
   - **gap** — Kernel missing a primitive (rare; justify hard)
   - **forbidden** — pair must not be allowed (document why)
5. Only promote **gap** into glossary after explicit design decision.

---

## Wave 1 — high-value pairs (results)

### Plumbing × work

| Pair | Emergent meaning | Outcome |
|---|---|---|
| `Const` + `Call` | Force a fixed arg / test fixture into a Call | covered |
| `Identity` + `Seq` | No-op stage / placeholder slot in a pipeline | covered |
| `Let` + `Call` | Name intermediates, then invoke | covered |
| `Tee` + `Call` | Main result + side effect Call (audit) | covered → pattern **AuditCall** |
| `Tee` + `LedgerAppend` | Canonical “do work, append receipt” | covered → pattern **Receipted** |

### Control × control

| Pair | Emergent meaning | Outcome |
|---|---|---|
| `Seq` + `Branch` | Pipeline with a conditional fork | covered |
| `Branch` + `Guard` | Soft fork vs hard refuse (else vs Err) | covered — keep both |
| `Try` + `Fallback` | Recover by reason **or** try alternate strategies | covered — distinct; do not merge |
| `Try` + `Park` | On NeedsRepair: ask/wait path | covered → pattern **RepairAndWait** |
| `Fallback` + `Budget` | Cheap→expensive under a shared envelope | covered → pattern **Escalation** |
| `Guard` + `Once` | Only run effect if invariant holds, at most once | covered → pattern **SafeOnce** |
| `Guard` + `Park` | Refuse to wait unless precondition true | covered |
| `Loop` + `Budget` | Bounded iteration under call/token cap | covered |
| `Loop` + `Park` | **Forbidden as busy-wait** — Park ends turn; Loop is in-eval only | forbidden (document) |
| `Map` + `Budget` | Fan-out transform with shared budget | covered |
| `Filter` + `Map` | Select then transform | covered (sugar: could be ForEach later) |
| `Seq` + `Park` | Do work then suspend | covered → pattern **StepThenWait** |

### Safety × time

| Pair | Emergent meaning | Outcome |
|---|---|---|
| `Budget` + `Timeout` | Cap resources **and** wall clock | covered — both required |
| `Once` + `Timeout` | Deduped effect with deadline | covered |
| `Timeout` + `Park` | Deadline on wait? Park already ends turn — Timeout wraps pre-Park body only | covered (clarify: Timeout does not replace Park) |
| `Once` + `LedgerAppend` | Deduped audited effect | covered |
| `Budget` + `Fallback` | Same as Escalation | covered |

### Effects × structure

| Pair | Emergent meaning | Outcome |
|---|---|---|
| `Now` + `Call` | Stamp time into args / IdemKey | covered |
| `Uuid` + `Once` | Fresh idempotency material (careful: Once key usually derived, not raw Uuid alone) | covered with caution |
| `Random` + `Call` | Seeded nondeterminism into a Call | covered — must ledger Random |
| `Seq(Now, LedgerAppend)` | Timestamped audit line | covered |

### Data-flow × lists

| Pair | Emergent meaning | Outcome |
|---|---|---|
| `Let` + `Map` | Bind config, map over list | covered |
| `Map` + `Try` | Per-item recovery | covered — errors policy must be explicit |
| `Filter` + `Guard` | Odd: Guard is fail-closed on whole input; prefer Filter then Guard on aggregate | clarify — prefer Filter |
| `Const(List)` + `Map` | Static fan-out recipe | covered |

---

## Emergent named patterns (not new Kernel ops)

These are **L2-shaped macros** over Kernel v0 — candidates for a later pattern glossary, **not** new L0-A opcodes:

| Pattern id | Expansion | Meaning |
|---|---|---|
| `Escalation` | `Budget{ Fallback[cheap, mid, expensive] }` | Cost-ascending strategies |
| `SafeOnce` | `Guard{ Once{ body } }` | Invariant then deduped effect |
| `Receipted` | `Tee{ body, LedgerAppend }` | Result + audit |
| `StepThenWait` | `Seq[ work, Park ]` | Turn boundary after work |
| `RepairAndWait` | `Try{ body, catch: NeedsRepair → Seq[AskCall, Park] }` | Typed repair suspension |
| `AuditCall` | `Tee{ Call, side }` | Generic side observation |

**Glossary decision so far:** Kernel stays closed; patterns live in L2 / docs — **no Wave-1 gaps requiring new L0 ops**.

---

## Forbidden / sharp edges (strengthen glossary)

1. **`Loop` + `Park` inside the same in-eval Loop** — Park suspends the turn; do not model human-wait as Loop. Human wait = `Park` + next turn resume.
2. **`Timeout` ≠ `Park`** — Timeout fails a running subtree; Park intentionally waits.
3. **`Branch` ≠ `Guard`** — else-path vs hard Err.
4. **`Try` ≠ `Fallback`** — reason-keyed recovery vs alternate strategies on same input.
5. **`Once` key design** is part of the contract — pairing with `Uuid` without a stable key scheme is footgun, not a new op.

---

## Wave 2 candidates (discuss next)

Pair each Kernel op with **`Call`** as the second block systematically (Call is the bridge):

- `Guard + Call(Policy.*)`, `Once + Call(Invoke.*)`, `Fallback + Call(Understand.*)` …

That wave validates L1 buildability without expanding Kernel.

---

## Verdict after Wave 1

Kernel v0 is **compositionally closed** for the pairs that matter: meaningful “next operations” appear as **patterns**, not missing syscalls.
Next: design owner reviews Wave 1 table → accept patterns list → then Wave 2 (`* × Call`) in conversation.
