# Harness Recall Audit Checklist

This is the end-to-end audit map for the semantic decision harness. It tracks
what is implemented now, what must still be built, and what must be proven before
the recall spine is considered production-complete.

## Implemented and Verified

- [x] Atomic operation vocabulary v0: `docs/HARNESS_ATOMIC_OPERATIONS_VOCABULARY.md`
  defines 100 independent harness primitives across sense, transform, query,
  retrieve, evaluate, reason, act, control, learn, and observe/govern families.
- [x] Sunjet data model v0: `docs/HARNESS_SUNJET_DATA_MODEL.md` maps all 26
  current Sunjet tables into identity, conversation, memory, harness runtime,
  learning graph, execution audit, async control, and SDK catalog spaces, with
  keys, vector fields, edge fields, decay rules, and missing future tables.
- [x] Full turn decision journal: `pathway`, `stance`, `prompt`, `reply`, `cache`,
  `confirmation`, and `proactive` trace kinds are written to Sunjet
  `harness_traces`.
- [x] Turn reconstruction API:
  `GET /api/v1/admin/harness/turns/:turnId` returns pathway, stance, prompt,
  reply, chronological traces, and per-turn API/embedding calls.
- [x] Immediate Context Engine: hot 5-minute verbatim context, 5-15/15-30/30-60
  minute and 1-24 hour persisted buckets, bucket reload, prompt factory wiring,
  and slide behavior are covered by `scripts/test-immediate-context.mjs`.
- [x] Archetype stance engine: positive/negative/neutral scores per category,
  margin/threshold prompt feeding, and single-vector reuse are covered by
  `scripts/test-archetype-engine.mjs`.
- [x] Semantic Pathway Engine: memory recall, tool ranking, hard-policy survival,
  active-flow selection, disengagement suppression, degradation behavior, and
  latency budget reporting are covered by `scripts/test-semantic-pathway.mjs`.
- [x] Reply relevance audit: memory, stance, immediate context, and pathway
  decision are proven to reach the exact LLM system prompt by
  `scripts/test-turn-relevance.mjs`.
- [x] Proactive daemon auditability: reflection follow-up suppression, blocked
  sends, sent nudges, reasons, and dedup keys are journaled as `proactive`
  traces.
- [x] Generic hot-path gate: narrow greetings/thanks/farewells bypass embeddings
  and LLM generation with deterministic template replies, and record a `generic`
  trace.
- [x] Archetype span attribution: mixed messages are split into sentence/clause
  spans for stance matching, so a negative clause can win sentiment even when
  the whole message starts positive.
- [x] Staged aspect publication: discovered aspects start as `candidate`,
  candidate buckets are withheld from live prompt steering, and approved/promoted
  aspects can feed guidance. Covered by `scripts/test-aspect-discovery.mjs`.

## Remaining Recall Spine

- [ ] Explicit temporal resolver.
  - Done means phrases such as "earlier today", "last week", "yesterday", "the
    previous invoice", and "when I last asked" become structured temporal query
    scopes.
  - Done means those scopes constrain immediate-context, message, compaction,
    memory, and trace recall.
  - Done means tests prove temporal phrases pull the intended window and exclude
    misleading older evidence.

- [ ] Full deterministic atom/span contract.
  - Current status: archetype stance has sentence/clause span attribution.
  - Done means every incoming message is split into typed spans: request,
    objection, sentiment, urgency, identity fact, policy-sensitive instruction,
    temporal reference, and closure.
  - Done means span offsets, source text, type, embedding id, and winning
    evidence are persisted or journaled.
  - Done means pathway, memory, policy, flow, and axis recall can all consume the
    same typed spans instead of only the stance engine using local spans.

- [ ] Batch span embeddings.
  - Done means spans share one batched embedding call where the provider supports
    it, with hash/local fallback preserving deterministic tests.
  - Done means embedding telemetry reports batch size, cache hits, provider, and
    latency.

- [ ] Centroid/BM25 atom matching.
  - Done means atom candidates are retrieved by hybrid text + vector search, not
    vector-only archetype search.
  - Done means Sunjet returns candidates with BM25/RRF where available, and
    TypeScript recomputes cosine for comparable final scores.
  - Done means tests cover lexical-only, semantic-only, and mixed cases.

- [ ] User-specific Harness Axis graph.
  - Done means aspects/archetypes are connected through Sunjet graph edges, not
    only scalar `aspect_id`.
  - Done means user-specific occurrences attach to axes with timestamp, message
    span, valence, confidence, and source turn.
  - Done means axes maintain head/previous occurrence chains for fast "what has
    this user repeatedly shown?" retrieval.

- [ ] Graph-constrained hybrid recall.
  - Done means retrieval can ask for "pricing sensitivity evidence for this
    customer in the current temporal scope" and traverse user -> axis ->
    occurrences -> source spans/messages.
  - Done means graph constraints and vector similarity cooperate instead of
    being merged only in application memory.

- [ ] Unified evidence scorer.
  - Done means pathway, memory, stance, immediate context, tools, policies,
    flows, traces, and proactive hints are normalized into one evidence score.
  - Done means the prompt receives top evidence by final score, with reason
    codes for included and excluded evidence.
  - Done means the journal records the formula inputs and the final score.

- [ ] Admin aspect governance.
  - Done means the admin UI/API can activate, retire, and merge discovered
    aspects.
  - Done means each governance action is audited with actor, before/after state,
    and reason.

- [ ] Proactive decision maturity.
  - Done means the daemon uses resolution state, engagement/interest counters,
    tenant activation scoring, and Harness Axis recall.
  - Done means every proactive decision records why it sent, blocked,
    suppressed, or deferred.
  - Done means tests cover opt-in, dedup, daily cap, WhatsApp window, closure
    suppression, unresolved-flow nudges, and low-interest deferral.

## Known Risks To Close

- [ ] Prompt trace privacy.
  - Current traces intentionally store the full compiled system prompt for
    replay/debugging. This can include customer personal information.
  - Done means strict admin access remains enforced, retention is configurable,
    and prompt payloads either support redaction-at-rest or a safe redacted
    projection for UI use.

- [ ] Trace durability.
  - Current traces are fire-and-forget and lossy by design.
  - Done means replay-critical events are promoted into a guaranteed ledger or
    the product explicitly labels traces as diagnostic-only.

- [ ] Real embedding calibration.
  - Current deterministic tests use controlled/hash embeddings.
  - Done means OpenAI/Ollama/provider-backed embeddings are benchmarked against
    a fixed relevance set with threshold calibration.

- [ ] Standard test inclusion.
  - `test:relevance` is now wired into `test:all`.
  - Done means repo-wide `pnpm typecheck` and `pnpm test:all` pass in a clean
    environment.

## Auditor Commands

Run these before calling the current harness layer healthy:

```bash
pnpm --filter @aelio/core typecheck
pnpm --filter @aelio/server typecheck
node scripts/test-immediate-context.mjs
node scripts/test-archetype-engine.mjs
node scripts/test-semantic-pathway.mjs
node scripts/test-turn-relevance.mjs
node scripts/test-scenario-matrix.mjs
node scripts/test-admin-db.mjs
```
