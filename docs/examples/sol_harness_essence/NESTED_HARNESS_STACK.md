# Nested harness stack (parent waits on child waits on grandchild)

**Status:** proven in `aelio-kernel` Sol library (not yet wired into live Conductor turn path)  
**Source:** `aelio-os/crates/aelio-kernel/src/sol_harness_lib.rs`  
**Proof:** `cargo test -p aelio-kernel nested_harness_stack` / `nested_stack_a_waits_on_b_waits_on_c`

---

## What this is

A **harness** here is a saved Sol program (`sol_harness_contracts`). One harness can **Call** another via:

```text
Call id = "harness.invoke@1"
args    = { "id": "<child harness id>", ...bag fields for child }
into    = "child"   // completed child bag lands here
```

`Call` is **synchronous**: the parent Sol walker does not advance to the next step until the child **Completes** and returns its bag. That is “wait for it to conclude.”

Children get the **same** invoke registry, so a child can itself Call another harness. That builds a **stack**:

```text
stack_top_a          (A)
   │  harness.invoke@1  → wait until B Completes
   ▼
stack_mid_b          (B)
   │  harness.invoke@1  → wait until C Completes
   ▼
stack_leaf_c         (C)
   │  express.say@1
   ▼
Completed bag → B continues → Completed bag → A continues → Completed
```

Final bag text for the demo chain:

```text
top-A after mid-B after leaf-C done
```

---

## Seed contracts (multiple harnesses)

| id | Role | Calls |
|----|------|-------|
| `stack_leaf_c` | Leaf — no further invoke | `express.say@1` → `"leaf-C done"` |
| `stack_mid_b` | Mid — child that is also a parent | `harness.invoke@1` → `stack_leaf_c`, then say |
| `stack_top_a` | Top of stack | `harness.invoke@1` → `stack_mid_b`, then say |
| `parent_waits_on_child` | Shallower demo (1 level) | `harness.invoke@1` → `calc_sum_gate` |

Many other library entries also use invoke (pipelines / routers). Those are **sibling compositions**; the stack trio is the explicit **multi-level wait** demo.

---

## Depth limit

`HARNESS_INVOKE_MAX_DEPTH` (default **8**) caps how deep A→B→C→… can go. Exceeding it returns `BudgetCalls` from `harness.invoke@1`.

This is a demo safety rail (cycles / runaway nesting), not Mother ISA.

---

## What wait means (and what it does not)

| Kind | Mechanism | Meaning |
|------|-----------|---------|
| **Sync child wait** (this doc) | Sol `Call` → `harness.invoke@1` | Parent step blocked until child **Completes**; bag returned into `into` |
| **Conversational Park** | Sol `Park` | Turn ends; resume later with user/tool input — **not** the same as invoke wait |
| **Parked child** | Child returns `Parked` | **Not supported** in this demo — invoke errors; no parent resume-of-child yet |

So: “wait for child harness to conclude” = wait for **Completed**, not “park and come back later.”

---

## How to run / replay

```bash
cd aelio-os
cargo test -p aelio-kernel nested_stack_a_waits_on_b_waits_on_c
cargo test -p aelio-kernel nested_harness_stack_a_waits_on_b_waits_on_c
```

Store → load → run → ledger replay (`bag_hash` identity) is covered by the integration test.

---

## Proper Mother Sol (registered-flow Calls)

`harness.invoke@1` is **demo sugar**: one Call target that looks up a child by runtime `args.id`.

Mother §8.4 / §10 / App A already prescribe the durable shape:

```text
Call(id: "flow.stack_mid_b@1", args: { utterance: pull(utterance) }, into: child)
```

Pinned flow ids for the same A→B→C wait:

| Call id | Body contract |
|---------|----------------|
| `flow.stack_leaf_c@1` | `proper_sol_stack_leaf_c` |
| `flow.stack_mid_b@1` | `proper_sol_stack_mid_b` (Calls leaf) |
| `flow.stack_top_a@1` | `proper_sol_stack_top_a` (Calls mid) |

Flow call graph is declared acyclic (`top → mid → leaf`). Cycles are refused.

Proof:

```bash
cargo test -p aelio-kernel --test proper_sol_nested_stack
cargo test -p aelio-kernel proper_sol_registered_flow
```

Sugar and proper Sol agree on outcome text: `top-A after mid-B after leaf-C done`.

## Relation to product Gate HS

- **Done here:** nested wait (sugar + proper registered-flow Sol), multi-level library, acyclic graph check, tests.
- **Not done:** Conductor selecting/playing these on a live turn; Park handoff across parent/child; production ability registry instead of demo Call stubs.

See also: [`EXHAUSTIVE_OPS_AND_HARNESSES.md`](./EXHAUSTIVE_OPS_AND_HARNESSES.md), [`library/`](./library/).
