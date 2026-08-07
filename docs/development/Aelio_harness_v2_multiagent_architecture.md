# Aelio Harness v2 — Multi-Agent Decomposition + Standard Tool Library + Refinement Loop

## 0. Framing: what this is and isn't

This is **not** a new fourth runtime. It is an upgrade to the existing **Cold Path** (and the legacy TS `runHarness`, which is its current implementation), so that "LLM proposes, deterministic code disposes" gets a recursive planning layer instead of a flat one. It still has to obey everything already true of the system:

- Retrieval and execution stay deterministic. Only *proposal* and *synthesis* are LLM-driven.
- Everything still runs through the same closed ability vocabulary (`Understand.*`, `Invoke.*`, `Express.*`), typed contracts, policy gates, idempotency, and budgets.
- Successful decomposition patterns are still eligible for promotion into the warm path via the existing learning gate — a multi-step plan that works reliably for a tenant should eventually collapse into a stored procedure, same as any other situation key.
- This lives **above** `aelio-kernel`, not inside it. Sub-agents don't get their own kernel — they each lower to a Sol op tree and the *same* kernel executes all of them. Multi-agent-ness is a planning/orchestration concept, not a new execution engine.

So: today, one cold-path turn = one LLM proposal → one verified op tree → one execution → one synthesis. v2 makes the *proposal* step recursive: the top-level proposal is allowed to emit **sub-tasks**, each of which gets its own scoped proposal → verify → execute cycle, and the whole thing can loop before final synthesis.

---

## 1. New concept: the Orchestrator

Add a stage between **Understand** and **Execute** in the turn spine, active only when Tier lookup lands on cold path (or on a warm path that's been flagged `complex`):

```
sense → flow gate → clause split → triage → situation key → tier lookup
  → [ORCHESTRATE] → execute → synthesize → write back
```

`ORCHESTRATE` is a new block, e.g. `aelio-agent/src/blocks/orchestrate.rs`, sitting next to `turn.rs`. Its job:

1. Decide **complexity class** for the (already clause-split) utterance:
   - `atomic` — one ability call suffices → skip orchestration, behave exactly as today.
   - `composite` — needs a sequence of known-shape steps (e.g. "average of these numbers") → decompose into a **linear plan**.
   - `open` — genuinely uncertain shape, needs exploration/branching, may need re-planning after seeing intermediate results (this is the "Cursor-style" case).
2. For `composite`/`open`, produce a **Task Graph**, not a single op tree.

### Why a graph, not just a list

"Composite" queries are usually a DAG (steps can fan out and join), not a strict chain. Model it as a DAG from the start so the sequential case is just a degenerate graph, rather than bolting branching on later.

---

## 2. Sol Contract additions: `TaskNode` and `TaskGraph`

New Sol Contract types (typed, same imprint/hashing discipline as everything else in the kernel):

```
TaskNode {
  id: TaskId
  goal: string                 // natural-language objective — the "what"
  rationale: string             // why this task exists / what it contributes to the parent goal
  strategy_hint: AbilityTag[]   // which closed abilities / standard tools this task is expected to need
  inputs: SlotRef[]             // references to parent outputs, session slots, or literal values
  outputs: SlotSpec[]           // typed shape this task must produce
  depends_on: TaskId[]          // DAG edges
  budget: TaskBudget            // step cap, token cap, wall-clock cap — subset of parent's remaining budget
  max_refinements: u8           // how many rework cycles this node is allowed (see §4)
}

TaskGraph {
  root_goal: string
  nodes: TaskNode[]
  join_spec: OutputSpec         // how leaf outputs combine into the parent's answer
}
```

This is the literal artifact an LLM call is asked to produce at orchestration time (JSON-schema constrained, same as any typed proposal today). The **type system verifies** it exactly like it verifies a flat op-tree proposal — malformed graphs (cycles, unresolved input refs, budget overrun, disallowed ability tags) are rejected before anything executes, and rejection triggers a bounded re-ask, not a silent fallback.

Each `TaskNode` is then **lowered independently** to a Sol op tree by the existing kernel planner, using its own `strategy_hint` as the ability shortlist. That means each sub-agent is, mechanically, just another cold-path proposal — scoped, typed, budgeted — not a different kind of thing. This is what keeps the whole system auditable: you can point at any sub-agent's op tree in the ledger exactly like a top-level one.

---

## 3. Execution: wavefront over the Task Graph

Reuse the **Resolver → wavefront Executor** pattern that already exists in `packages/core/src/harness` (order steps into waves respecting dependencies, run each wave's independent nodes concurrently, respect idempotency keys). Port that scheduling logic into `aelio-runtime` so it runs under Rust, not just in the legacy TS harness:

1. Topologically sort `TaskGraph.nodes` into waves.
2. For each wave, lower each `TaskNode` to an op tree (parallel LLM proposal calls where the tenant's LLM provider allows concurrency; otherwise serialize with the existing budget queue).
3. Execute each node's op tree via `aelio-kernel`, same replay/ledger guarantees as any turn.
4. Bind each node's outputs into the shared `SlotRef` namespace so downstream nodes/joins can reference them.
5. On failure of a node: retry within that node's own budget first (see §4), and only propagate failure upward if refinements are exhausted — don't fail the whole turn because one leaf failed, unless the join spec marks it required.

This is exactly the harness's existing "plan → bind → resolve → execute (wavefront) → synthesize" pipeline, just: (a) running on the Rust side so production turns don't have to fall back to TS, and (b) with nodes that can themselves recurse (a node's `strategy_hint` can include `Orchestrate` as an ability, letting it spawn its own sub-graph — bounded by a hard recursion-depth budget, e.g. depth ≤ 3, to keep this from becoming unbounded agent sprawl).

---

## 4. The refinement loop (self-critique)

This is the "if the system isn't satisfied, rework it" piece. Add it as a **gate**, not a vibe — otherwise it either never fires or fires forever.

For each `TaskNode` (and once more for the joined final answer):

1. **Evaluate** — a cheap, typed critique ability (`Evaluate.*`) checks the node's output against its `outputs: SlotSpec[]` (shape/type check — deterministic, free) and, if that passes, against its `goal`/`rationale` via a scoped LLM judge call (semantic check — this is the only part that costs a token).
2. **Score** — the judge returns a typed verdict: `{ pass: bool, confidence: f32, failure_reason: enum, repair_hint: string }`. This is a contract, not free text — same discipline as everything else.
3. **Loop** — if `pass == false` and `max_refinements` not exhausted:
   - Re-run *only that node's* proposal step, with `repair_hint` and the failed output appended to context.
   - Decrement `max_refinements`.
   - Re-evaluate.
4. **Escalate, don't loop forever** — if refinements are exhausted and it still fails: either (a) fall back to a narrower strategy_hint the node hasn't tried, (b) mark the node's output as low-confidence and let synthesis caveat it to the user, or (c) surface a clarification question to the user via `Express.*` — same "ask rather than guess" pattern the system presumably already uses for underspecified input. Never silently return a wrong answer as if it were confident.

Budget interaction: refinement attempts consume the node's `TaskBudget`, which is carved out of the parent turn's overall budget at graph-build time. This is what stops "keep trying until it's perfect" from becoming an unbounded cost/latency sink — it's the same deterministic budget-check mechanism already described for the rest of the system, just applied per-node instead of per-turn.

This whole mechanism is itself a candidate for the **learning/promotion gate**: if a given task shape reliably needs 2 refinement passes for a given tenant, that's a signal the initial proposal strategy for that situation key is under-specified, and the promoted warm-path version should encode the *already-refined* plan directly — so paying the refinement cost once, not on every turn, is the actual long-term win.

---

## 5. Standard Tool Library

Today tenants register tools over the SDK bridge. Add a **first-party tool namespace** that ships with Aelio itself, registered the same way (same `Tool` contract: capability_tags, params with `source`, continuations) but backed by the Rust kernel directly instead of a round-trip to the tenant's backend. This matters for two reasons: (a) latency — arithmetic shouldn't cost a WebSocket round trip to the tenant's server, and (b) determinism — these are exactly the kind of thing that should never be an LLM guess.

Suggested initial set, organized by capability tag so the planner can shortlist them the same way it shortlists tenant tools:

| Namespace | Tools | Notes |
|---|---|---|
| `std.math.*` | `add`, `subtract`, `multiply`, `divide`, `average`, `sum`, `min`, `max`, `round`, `percent_of` | Pure, deterministic, no side effects — never need confirmation, never need idempotency keys |
| `std.data.*` | `filter`, `sort`, `group_by`, `map_field`, `dedupe`, `paginate` | Operate on typed lists already in scope (tool outputs, session slots) |
| `std.text.*` | `format_template`, `truncate`, `join`, `extract_pattern` | For assembling intermediate strings between steps, not for generation — generation stays an LLM ability |
| `std.control.*` | `branch_on`, `for_each`, `retry_with` | Thin wrappers so a `TaskNode` can express "do this per item in a list" without needing a fresh LLM proposal per item |
| `std.ideation.*` | `brainstorm_options`, `rank_options`, `pick_best` | These *are* LLM-backed (unlike the rest of this table) — flag them explicitly as non-deterministic so policy/budget treats them differently from `std.math.*` |

Key design point: **`std.math.*` and friends are not "LLM calls a calculator tool" in the sense of the model doing arithmetic and a tool double-checking it.** The planner decomposes "calculate my average" into a `TaskGraph` whose nodes call `std.math.sum` and `std.math.divide` directly — the LLM never touches the numbers, it only decides *that* a sum-then-divide shape applies and *what* the inputs are. That's what keeps this consistent with "the LLM proposes, the types check, execution is deterministic" instead of quietly becoming "ask the model to do math and hope."

Registration mechanism: these can literally reuse the existing tenant-tool registration path (`Tools` declaration), just pre-seeded at server boot instead of arriving over the SDK bridge, and tagged `provider: aelio-standard` so policy gates can allow/deny them per tenant like anything else (e.g. a tenant might want `std.control.retry_with` disabled for cost reasons).

---

## 6. Rust crate / file-level plan

```
aelio-os/crates/aelio-agent/
  src/blocks/
    orchestrate.rs        // NEW — complexity classification + TaskGraph proposal + verification
    turn.rs                // MODIFIED — insert ORCHESTRATE stage between tier lookup and execute
    evaluate.rs            // NEW — Evaluate.* ability, scoring contract, refinement trigger

aelio-os/crates/aelio-sol/
  src/contracts/
    task_graph.rs          // NEW — TaskNode, TaskGraph, TaskBudget, EvalVerdict contracts + canonical hashing

aelio-os/crates/aelio-kernel/
  src/
    graph_lower.rs          // NEW — lowers a single TaskNode to an existing Sol op tree (reuses current planner per-node)

aelio-os/crates/aelio-runtime/
  src/
    wavefront.rs            // NEW — port of packages/core harness Resolver/Executor scheduling into Rust
    recursion_guard.rs      // NEW — hard depth cap for nested Orchestrate calls

server/src/
  runtime-deps.ts           // touch only if new turn fields are exposed to TS (should stay Rust-internal otherwise)
```

Standard tools ship as a new package, e.g. `aelio-os/crates/aelio-stdlib`, exposing `std.math.*` etc. as native Rust implementations registered into the same `Tool` contract tenant tools use — not TS, so they run at kernel speed with no bridge hop.

---

## 7. Phased rollout (what to actually ask Cursor to build, in order)

1. **Contracts first**: `TaskNode`/`TaskGraph`/`EvalVerdict` in `aelio-sol`, with tests for the type-checker rejecting cyclic/malformed graphs. No behavior change yet.
2. **Standard math tools**: `aelio-stdlib` with `std.math.*`, registered exactly like a tenant tool, tested via the existing cold path with *no* orchestration — i.e. first prove a single-node plan can call `std.math.average` end to end.
3. **Orchestrate block, linear-only**: complexity classifier that only ever emits linear chains (no branching), wavefront executor that degenerates to sequential execution. This alone should already handle "calculate the average of X, Y, Z."
4. **Refinement loop**: add `Evaluate.*` and the retry-with-repair-hint cycle, gated behind `max_refinements`, on top of step 3.
5. **DAG/branching + recursive sub-orchestration**: only after 1–4 are solid in production, add fan-out/join and let a node's `strategy_hint` include `Orchestrate` itself, bounded by `recursion_guard`.
6. **Promotion integration**: feed successful refined TaskGraphs into the existing learning/promotion gate so repeat situations skip planning entirely and go warm.

Steps 1–3 alone deliver the "break a complex query into an internal plan and execute it with real tools" behavior you described. Steps 4–6 add the self-correction and long-term cost amortization on top. Build and ship in that order — don't start with the general branching/recursive case, it's the part most likely to blow up cost and latency if it ships before the budget/refinement guards exist.