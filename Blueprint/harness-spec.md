# Aelio Harness — Design Specification

The harness is Aelio's turn engine: a deterministic **plan → bind → resolve →
execute → synthesize** loop that guides each request with as few LLM calls as
possible. It replaces the flat 5-iteration tool loop. Enabled by default
(`harness.enabled: true`); the legacy loop remains selectable for one release.

Guiding principle: **the LLM proposes, deterministic code disposes.** Every place
the model could gate execution, a code gate sits behind it.

## Turn lifecycle

```
message
 └─ per-session lock (serialize turns for this customer+channel)
     ├─ resume? a parked recoil plan intercepts the message (validate answer → resume)
     ├─ pending confirmation? (yes/no → invoke/cancel)   [legacy path, still used]
     ├─ response cache hit? → reply                       (0 LLM calls)
     ├─ PASS 1  emit_turn (forced tool):
     │     reply   → shallow answer                        (1 LLM call total)
     │     refuse  → gated by a feasibility probe (fail-open)
     │     plan    → capability-level steps
     ├─ BIND     capability → tool (planner hint → semantic search + graph)
     ├─ RESOLVE  derive the dependency DAG from the BOUND schemas
     ├─ EXECUTE  topological wavefront (reads parallel, writes sequential)
     │     per call: assemble args → gate → idempotency → invoke → ledger → transition
     └─ SYNTHESIZE final reply from the ledger              (1 LLM call)
```

Call budget: shallow = 1, deep happy path = 2 (plan + synthesis), worst case
bounded by hard budgets.

## Components (`packages/core/src/harness/`)

- **planner.ts** — Pass 1. One forced `emit_turn` call returns a validated union
  (`reply | refuse | plan`). One corrective retry on invalid structured output,
  then graceful degrade (never crashes a turn). Merged router+planner: shallow
  vs deep is which branch the model emits, not a separate call.
- **binder.ts** — capability → concrete tool. Planner's own suggestion first;
  else semantic search over the Lighthouse mirror, bound only when the top hit
  clears `scoreMin` and beats the runner-up by `ambiguityGap`.
- **resolver.ts** — *the spine-bug fix.* Dependency structure is derived here,
  from the **bound tool schemas**, never from LLM prose. A required param
  produced by an earlier step → a dependency edge (a "rock"); independent params
  → parallelizable ("river"); unproducible + unsupplied → a recoil trigger.
  Cycle detection + a topological `nextWave` generator. `read`/`write` effect
  comes from the schema.
- **executor.ts** — runs the DAG as a wavefront: reads in a wave run in
  parallel, writes run alone in id order (side effects never race). Per
  invocation: assemble args (piping producer outputs into consumers) → coerce →
  **gate** → **idempotency** (ledger short-circuit) → invoke → ledger append →
  **transition**. Returns a typed outcome: complete / suspend / blocked / replan.
- **gates.ts** — the verdict engine: `allow | deny_fatal | needs_approval |
  needs_info | transform`, composed from `evaluateSafety` + state tool-gating +
  guard conditions, evaluated on the concrete resolved args regardless of what
  the plan proposed.
- **suspension.ts / resume.ts** — the unified suspend/resume store, for BOTH
  recoil (`awaiting_info`) and write-confirmation (`awaiting_confirmation`). On
  suspend the plan + ledger are persisted (SQLite authoritative, Aelio DB mirror);
  the next message rehydrates it (rebind tools, replay ledger so finished steps
  never re-run) and resumes — so a mid-plan confirmation continues the WHOLE
  plan, not just the one confirmed write. Recoil pins the validated answer;
  confirmation grants the pending write's approval so its gate passes. Explicit
  "no/cancel" abandons; a changed registry hash or vanished tool discards;
  recoil is bounded by `maxRecoilsPerIntent`. (The legacy pending-confirmation
  path in turn.ts remains only as the fallback when the harness runs without a
  suspension store, and for the legacy tool loop.)
- **transitions.ts** — declarative lifecycle transitions on tool success
  (`create_order` succeeds → `awaiting_payment`), guard-checked. SDK `set_state`
  always overrides.
- **budgets.ts** — deterministic loop protection: max instructions/replans/tool
  calls, wall clock, token meter, and progress detection (identical
  (tool, args) three times = a loop). The LLM never controls loop exit.
- **traces.ts** — append-only Aelio DB firehose (plan/bind/wave/gate/…); never fed
  back into prompts; rolls up the existing L0–L3 compaction ladder.

## Lighthouse (`packages/core/src/lighthouse/`)

The read model over the SDK registry. A content **hash** of the merged registry
is the invalidation key for tool embeddings, the capability taxonomy, and every
binding-cache entry — so reconnects that don't change the registry recompute
nothing. The in-memory bridge is authoritative for liveness; the Aelio DB mirror
(`harness_tools` with prerequisite-graph edges, `harness_capabilities`) serves
semantic search and the feasibility probe, degrading to in-process embedding
rank when Aelio DB is off.

## Storage — aelio-os only, zero external deps

- **Registry mirror + capability index** → Aelio DB (vector + graph + FTS).
- **Hot state** (suspended plans, ledger) → SQLite authoritative + Aelio DB mirror.
- **Traces** → append-only Aelio DB, tiered by the compaction daemon.

No Redis, no external services — the single-container promise holds.

## Config (`harness.*`)

```yaml
harness:
  enabled: true
  budgets: { max_instructions: 12, max_replans: 2, max_recoils_per_intent: 3,
             max_tool_calls: 15, wall_clock_ms: 60000, max_turn_tokens: 30000 }
  binding: { score_min: 0.55, ambiguity_gap: 0.08, cache_ttl_minutes: 1440 }
```

## Deferred (not in this pass, no correctness cost)

- Batched LLM disambiguation for genuinely-ambiguous bindings (today: left
  unbound → graceful "no way to do that").
- Surprise-driven segment replanning after a rock (the outcome/replan path
  exists; the trigger is conservative).
- Binding cache reads/writes against `harness_bindings` (table + key defined).
- `transform` policy verdict (arg caps) — enum + plumbing exist; no built-in rules.

## Verification

`pnpm test:harness` (10 unit checks: the cart/coupon spine case, river
parallelism, write sequencing, the gate matrix, recoil round-trip, stale-registry
discard, state transition) + `AELIO_TEST_MODE=1 pnpm test:all` (integration,
harness on) + the same with `harness.enabled: false` (legacy path).
