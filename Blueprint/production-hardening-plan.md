# AelioConvox Production-Hardening Plan

**Goal:** take the AelioConvox SDK + Aelio server (with embedded Sunjet engine) from
"works in demo" to production-grade: domain-neutral core, resilient storage, secure
transport, token-efficient prompting, measurable spend, and agent-grade tool selection.

**Status legend:** `[ ]` planned · `[x]` done · each item lists the files it touches.

---

## P0 — Correctness · Security · Availability

### P0.1 De-ShopCo the core (client-declared intelligence) `[x]`

**Problem.** The generic brain is contaminated with one demo tenant's domain:
`intent/stack.ts` hardcodes ShopCo tool names (`getOrderStatus`, `upgradePlan`…) and
order/invoice regexes; `analyst/extract.ts` hardcodes "metric units"/"order status"
heuristics. Every real tenant gets wrong intent labels and useless memory extraction.

**Design.**
- Tools, states, policies, flows already come from the client SDK. The missing piece
  is the **intent taxonomy** — it must also come from the client:
  - `FunctionDefinition` gains an optional `intent` field (a category label, e.g.
    `"order_inquiry"`). Declared in `aelio.expose(name, handler, { intent: 'order_inquiry', ... })`.
  - When a turn executes tools, the intent label = the executed tool's declared
    `intent` (fallback: the tool's name). Client vocabulary, zero core hardcoding.
  - With no tool executed, classification falls back to **embedding similarity**
    between the user message and each registered tool's `intent + description`
    (embeddings are memoized, so this is nearly free) — above a floor it adopts the
    best tool's intent; otherwise `general`.
  - Only two universal linguistic rules remain in core: `greeting` and
    conversation-conclusion detection (pop) — domain-free.
- **Memory extraction** keeps only domain-neutral, explicit self-declarations
  (name, email, stated preference "I prefer/always/never…", language). Deep domain
  facts belong to the reflection daemon (LLM, once per closed session) — cheaper and
  richer than per-turn keyword guessing.

**Files.** `packages/protocol` (intent on FunctionDefinition), `packages/sdk-node`,
`packages/sdk-python`, `packages/core/src/intent/stack.ts`,
`packages/core/src/analyst/extract.ts`, `examples/sample-saas` (declare intents).

### P0.2 Sunjet write-path fallback `[x]`

**Problem.** `fallback_sqlite_on_error` is honored for reads only. A Sunjet outage
makes `persistMessage` throw → **every turn hard-fails** even with dual-write SQLite
available. Config promises resilience the write path doesn't deliver.

**Design.** Wrap the Sunjet append; on failure with `fallbackSqliteOnError`, log and
continue with the SQLite write (which dual-write performs anyway). A Sunjet outage
degrades archival, never conversations. (Divergence is acceptable: SQLite is the
operational store; Sunjet rows can be backfilled by a future reconciliation job.)

**Files.** `packages/core/src/runtime/turn.ts` (`persistMessage`).

### P0.3 SDK secret in Authorization header `[x]`

**Problem.** The secret rides in the WS URL (`/sdk?secret=…`) and is printed into
server request logs by Fastify. Query strings leak into logs/proxies.

**Design.** Node + Python SDKs send `Authorization: Bearer <secret>` on the WS
handshake. Server accepts the header first, query param as deprecated fallback
(back-compat with older SDKs). Docs updated.

**Files.** `packages/sdk-node`, `packages/sdk-python/sdk.py`,
`apps/server/src/routes/sdk.ts`.

### P0.4 WhatsApp webhook idempotency `[x]`

**Problem.** Meta retries webhooks. `messageId` is carried but never deduped — a
retry re-runs the turn (double LLM spend, possible double tool execution).

**Design.** `inbound_dedup` table (message_id PK, created_at). INSERT OR IGNORE at
enqueue; duplicates are dropped. Applies to WhatsApp webhook and SDK `ingest`
messages that carry a `messageId`. Rows older than 7 days pruned opportunistically.

**Files.** `packages/db/src/index.ts` (table on startup),
`apps/server/src/routes/whatsapp.ts`, `apps/server/src/routes/sdk.ts`.

---

## P1 — Token efficiency & measurability

### P1.1 Prompt composer `[x]`

**Problem.** The system prompt is ad-hoc string concatenation: volatile content
(memories, intent) interleaves with stable content (persona, policies, tools), so
provider prompt-caching never hits — full price for the stable prefix on every call,
including every tool-loop iteration. No token budgets → unbounded growth. Persona is
hardcoded in core; tenants can't set voice. Indented template literals ship leading
whitespace as tokens.

**Design.** `runtime/prompt-composer.ts`:
- **Sections** with `stability` (stable | volatile) and `priority`:
  persona → policies → lifecycle → guidance (stable); summary → memories → intent
  (volatile). **Stable sections always render first** → cacheable prefix for
  OpenAI (auto ≥1024 tokens) and Anthropic (`cache_control`-ready ordering).
- **Token budgets** (≈ chars/4) per section + a total budget; over budget, trim
  lowest-priority volatile sections first; stable sections never silently dropped.
- **Whitespace normalization** (dedent, collapse blank runs) on every section.
- **Client persona:** `RegisterMessage.persona` (SDK: `aelio.persona(text)`) and/or
  config `llm.system_prompt`. Fallback to the built-in default.

**Files.** `packages/core/src/runtime/prompt-composer.ts` (new), `turn.ts`,
`packages/protocol`, `packages/sdk-node`, `packages/sdk-python`,
`apps/server` (config + bridge), `examples/sample-saas`.

### P1.2 Token accounting `[x]`

**Problem.** Providers return usage on every response; adapters discard it. Telemetry
has durations but no tokens — spend is unmeasurable. `messages.tokens_in/out` exist,
unused.

**Design.** `LLMCompleteResult.usage { inputTokens, outputTokens }` populated by
OpenAI-compatible (`usage.prompt_tokens/completion_tokens`), Anthropic
(`usage.input_tokens/output_tokens`), Gemini (`usageMetadata`). The instrumented LLM
wrapper records tokens into `turn_api_calls` (`tokens_in/tokens_out`, added via an
idempotent ALTER at migrate time); per-turn totals = SUM over `turn_id`.

**Files.** `packages/llm/src/*`, `packages/core/src/telemetry/turn-calls.ts`,
`packages/db/src/{schema,index}.ts`.

### P1.3 Tool-result truncation `[x]`

**Problem.** `JSON.stringify(result)` enters the context uncapped — one fat tool
result inflates every subsequent LLM call in the window.

**Design.** Cap tool-result content (default 4000 chars) with an explicit
`…[truncated, N chars total]` marker so the model knows to ask for narrower queries.

**Files.** `packages/core/src/runtime/tool-loop.ts`.

### P1.4 Session summary refresh `[x]`

**Problem.** Summary is generated once and cached forever — turn 200 runs on a
summary from turn 50.

**Design.** Track `summarizedAtCount` in session metadata; re-summarize when
`messageCount ≥ summarizedAtCount + summarize_after`, folding the previous summary
into the new prompt.

**Files.** `packages/core/src/session/summary.ts`.

---

## P2 — Agent-grade tool selection

### P2.1 Smart tool retrieval (top-K per turn) `[x]`

**Problem.** All registered tools ship to the model every turn. Fine at 6; at
company scale (50–200 tools across appendages) selection accuracy drops and token
cost rises linearly.

**Design.** `runtime/tool-retrieval.ts`:
- Below `TOOL_RETRIEVAL_THRESHOLD` (default 12 tools) → send all (zero overhead).
- Above it: embed each tool's `name + intent + description` **once** (memoized by
  content), embed the user message (already memoized), rank by cosine, take top-K
  (default 8), **always force-include**: tools executed earlier in the session's
  active intent frame, the active flow step's tool, and any pending-confirmation
  tool. Lifecycle state filtering (allowed/blocked) still applies first.
- Pure functions + injected embedder → unit-testable without a server.

**Files.** `packages/core/src/runtime/tool-retrieval.ts` (new), `turn.ts`.

---

## P3 — Hygiene

| # | Fix | Files | Status |
|---|---|---|---|
| P3.1 | Recall filters expired memories (`expires_at`) | analyst/recall.ts | `[x]` |
| P3.2 | Dim mismatch: warn once per (table, dim) instead of silent truncate | storage/messages.ts, conversations.ts | `[x]` |
| P3.3 | Telemetry writes fire-and-forget (never block the hot path) | analyst/embeddings.ts, telemetry | `[x]` |
| P3.4 | Sample lifecycle descriptions dedented (stop shipping indentation as tokens) | examples/sample-saas | `[x]` |

---

## Deferred (explicitly not in this pass)

- **Sunjet scan projection** — engine-side change (Astrolobe repo): `cols` on scan,
  `order_by` + `desc`. Unblocks Sunjet-primary history reads without vector payloads.
- **Streaming completions** — UX latency; provider interface change.
- **Sunjet↔SQLite reconciliation job** — backfill archive rows after outages.
- **Multi-tenant workspace isolation** — cloud scope.

## Verification contract

Every item lands only with: `pnpm typecheck` (15/15) · `pnpm build` (9/9) ·
`pnpm test:all` (6/6) · `pnpm test:sunjet` (13 OK) · phase6 PASSED · plus a focused
test per item. Finished work is committed and pushed.
