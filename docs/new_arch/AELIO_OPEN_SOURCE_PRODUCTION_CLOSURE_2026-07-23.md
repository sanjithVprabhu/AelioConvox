# Aelio open-source production closure — 2026-07-23

## Verdict

The runtime gaps identified in the external audit are closed in the implemented Rust path. The
result is suitable for an open-source core release: it has a generic typed turn spine, durable
learning, bounded organized recall, strict declared evidence boundaries, fail-closed production
configuration, restart semantics, operator controls, and readable decision traces.

This verdict covers the core runtime and single-process server topology. It does not pretend that
deployment-specific product policy—retention law, provider pricing, tenant calibration, channel
delivery, or multi-node ownership—can be chosen generically by the engine.

## External-audit reconciliation

| Finding | Closure evidence |
|---|---|
| P0-1 Tier-2 goal was an intent label | `GoalSpec` is built from declared output semantics. The bounded recursive backward planner satisfies missing evidence through promoted procedures and supports three-hop chains. |
| P0-2 rank bypassed the ladder | Rank normalization now supplies bounded slots and a tenant-declared query capability before the ordinary Tier 0/1/2/3 lookup. Repeated success promotes and later hits Tier 0. |
| P0-3 production defaulted to hash embeddings | `aelio-server` now requires a semantic gateway/embed URL. Hash mode requires the explicit `AELIO_ALLOW_HASH_EMBEDDER=1` development override. |
| P0-4 rank discarded tool results | Declared safe tool fields become provenance-bearing claims used by grounded synthesis. Undeclared fields are removed from the safe response entirely. |
| P1 vertical and English routing leaks | Flow triggers and business vocabulary come from tenant declarations. Confident semantic declared intent forces deep execution. Thin depth decisions and ambiguous clause splitting use closed, bounded model schemas when a live provider is configured. Non-English ranked extraction is covered by a golden test. Generic English heuristics remain only as the offline/free pre-gate, not production authority. |
| P2-1 boundary was an echo stub | Boundary turns assemble user-scoped recall, current state, and active capabilities, then use the same grounded-claim synthesis boundary. |
| P2-2 demo defaults looked production-like | The production binary starts with an empty tenant, unavailable LLM, required authentication, and required semantic embedding configuration. Hash/scripted behavior remains explicitly confined to examples and tests. |
| P2-3 inline durable promotion was off | This remains deliberate: the durable cold loop records behavior, promotes with compare-and-set, and refreshes the registry. The hot loop never races an uncommitted promotion. |
| P2-4 commit log was cosmetic | The renderer says `Commit(runtime)`; durable execution emits a separate `Persist(durable)` record. |
| P2-5 hardcoded terminal/greeting fallback | Authored flow completion is a personality template. Deep path construction and execution fail closed instead of learning a greeting fallback. The authored greeting path is used only for shallow greetings. |
| P2-6 clause heuristics were English-only | The pure splitter is only a pre-gate. Ambiguous conjunctions can invoke `aelio.split_clauses`, a closed one-to-four-clause schema, with deterministic fallback on provider failure. |
| P3-1 weak lexical term matches | Partial anchors require a minimum length, attribute names require exact/all-token matches, and novel semantic equivalence is forbidden in hash mode. |
| P3-2 vector-space drift | A persisted embedding-space identity binds endpoint, model, and dimension. Same-dimension model drift is rejected. |
| P3-3 memory two-phase desynchronization | Only the durable wrapper can enable durable memory, and it uses the same pure request parser as the hot path. |
| P3-4/P3-5 stale tests and traces | The 12-scenario report was regenerated at 12/12 supported, and the decision-log conversation now shows Tier 2 observation, promotion, and Tier 0 retrieval. |

## Additional defects found during closure

1. Sanitization retained undeclared tool response fields. It now constructs an empty response and
   copies only declared output paths; sensitive declared paths are redacted.
2. Composed tools could not bind a prior tool's result through `ParamSource::ToolOutput`. The
   executor now hands forward only sanitized declared evidence.
3. Durable promotion used a fresh hash embedder even when runtime lookup used a semantic embedder.
   Promotion, replay, Tier 1, term resolution, and recall now share the same embedding handle.
4. Tier-1 candidates were scored before hard state/slot/capability filtering. Hard boundaries now
   run first.
5. The gateway embedding cache retained raw text and was unbounded. Keys are now one-way
   fingerprints, input is capped at 32,768 characters, and the cache is capped at 4,096 entries.
6. A custom completion URL could be reused accidentally as an embedding route. Only the canonical
   `/v1/llm/complete` route is derived; custom gateways must declare `AELIO_LLM_EMBED_URL`.
7. Invalid numeric embedding configuration silently fell back to defaults. Invalid or zero values
   now fail startup.
8. Operator-imported promoted procedures had no cold proposal row, causing an otherwise successful
   turn to fail during learning. Imported procedures now execute without being misclassified as
   runtime-earned proposals.
9. The generated 12-scenario harness encoded obsolete expectations and exposed the imported
   procedure defect. Its scenarios now exercise typed OTP repair, real two-tool evidence handoff,
   rank execution, and conversational recall end to end.

## Runtime invariants after closure

- The model may classify a thin language decision, propose a cold path, or express verified
  evidence. It cannot invent tools, policies, state transitions, evidence fields, or procedure
  dependencies.
- Tool output is usable only when declared in `OutputSpec`; secrets and undeclared fields cannot
  reach another tool, memory, synthesis, or a decision log.
- Tier 0 exact retrieval and Tier 1 near retrieval apply hard state, required-evidence, capability,
  signature, tool-version, prompt, and embedding-space boundaries.
- Tier 2 is bounded recursive backward search. Tier 3 output is a closed list of declared ability
  identifiers and is type-checked before execution.
- Read-only paths may promote from behavioral evidence. Effectful paths require tenant approval.
  Auth, payment, deletion, and other authored protected flows are structurally non-learnable.
- Turns, effects, flows, memories, proposals, procedure versions, provider calls, and step traces
  have durable replay/restart behavior. Unknown non-idempotent outcomes require manual review.

## Verification

Executed from `Sunjet/Astrolobe`:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p aelio --example aelio_12_simulations
cargo run -p aelio --example decision_log_conversation
```

Results:

- Full Rust workspace: pass.
- Strict clippy with warnings denied: pass.
- Aelio deterministic core/server corpus: 159 tests passed; five network-backed tests intentionally
  ignored because no live provider credentials were present.
- Architecture simulation: 12/12 supported, 0 degraded.
- Decision-log conversation: wrong OTP follows typed repair; correct OTP authenticates; the ranked
  request follows Tier 2 → evidence gate → promotion → Tier 0.

## Honest remaining release gates

These are operator/product decisions, not hidden engine implementation holes:

1. Publish a retention/deletion policy, including how user deletion affects aggregate learning.
2. Calibrate semantic and model thresholds on each tenant's language/domain corpus.
3. Declare versioned provider/tool pricing before reporting monetary cost.
4. Select metrics/export infrastructure and alert thresholds for tier rate, escalation, repair,
   disagreement, latency, and cost. The durable facts and traces exist; the dashboard backend is a
   deployment choice.
5. Define entity merge/split authority, locale/currency presentation, channel delivery guarantees,
   SDK heartbeat policy, and multi-node tenant ownership before horizontally scaling.
6. Run the ignored live-network suite against the exact production model/gateway release candidate.

Cross-tenant learning remains intentionally disabled.
