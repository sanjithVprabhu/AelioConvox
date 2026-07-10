# Aelio-Convox — Production Readiness Log

> **Created:** 2026-07-10  
> **Purpose:** Living record of pre-production gaps, smoke-test findings, architecture fixes, logic fixes, and recommendations.  
> **Companion docs:** [bug-list.md](./bug-list.md) · [fix-log.md](./fix-log.md) · [project-documentation.md](./project-documentation.md) · [Blueprint/harness-spec.md](../Blueprint/harness-spec.md)

Use this file when running real-LLM conversations, live Sunjet tests, or manual widget click-throughs. Append findings under **Session Logs**; promote confirmed issues to **Open Issues**; move resolved items to **Resolved**.

---

## Status Legend

| Symbol | Meaning |
|--------|---------|
| 🔴 | Unverified / blocking for production confidence |
| 🟡 | Partially verified (unit/mock only) |
| 🟢 | Verified live |
| ⚪ | Accepted V1 limitation (by design) |

---

## Executive Summary (2026-07-10)

| Area | Status | What we know |
|------|--------|--------------|
| Unit + mock integration | 🟢 | 49 bugs fixed (Passes 1–2); `AELIO_TEST_MODE=1 pnpm test:all` + `pnpm test:harness` pass on **mock LLM** |
| Real-LLM planning quality | 🔴 | **Single most important gap.** Mock keyword-matches (`packages/llm/src/mock.ts`); cannot validate `emit_turn` plans from Claude/GPT/Gemini |
| Provider forced-tool mappings | 🟡 | Anthropic `tool_choice`, OpenAI `tool_choice`, Gemini `functionCallingConfig.mode: ANY` coded; construction + config selection now covered by `pnpm test:llm`. Still **not run live** (structured-output compliance) against a real provider |
| 3-provider configurability | 🟢 | `@aelio/llm` split into `providers/` + `embeddings/`; Anthropic/OpenAI/Gemini selected purely by config (`config.{anthropic,openai,gemini}.yaml`); missing keys fail loudly at boot; `pnpm test:llm` green |
| Sunjet/Astrolobe E2E | 🟡 | `pnpm test:sunjet` exists but requires live `ll-server`; default `sunjet.enabled: false` — TS mirror, tool-graph search, trace firehose only tested against **in-process fallback** |
| Multi-turn recoil / confirmation (browser) | 🟡 | Covered by unit tests (`test-harness-executor.mjs`) + mock phase tests — **no human widget click-through** |
| Load / concurrency | 🔴 | Per-session turn lock logic tested; no multi-session load, no SDK invoke storm, no Sunjet backpressure |
| Security hardening | 🟢 | P0/P1 audit items closed (see bug-list) |
| Harness architecture | 🟡 | Design complete (`Blueprint/harness-spec.md`); executor/resolver/gates unit-tested; planner quality depends on real LLM |

**Bottom line:** The deterministic harness (bind → resolve → execute → synthesize) is solid in isolation. Production confidence hinges on **one real-LLM smoke pass** with your actual SDK tool registry.

---

## Honest Gaps — Verify Before Production

### 1. Real-LLM planning quality (P0)

**Risk:** The mock planner uses keyword heuristics (`cancel` → `cancelOrder`, `order|status` → `getOrderStatus`). A real model may:

- Emit `mode: "reply"` when it should `plan` (skipping tools)
- Produce invalid `emit_turn` payloads (degraded to free-text reply after 2 retries)
- Name wrong tools in `instructions[].tool` hints
- Under-specify `produces` fields → resolver recoil loops or unbound capabilities
- Over-plan (too many instructions) → budget abort

**What mock cannot tell you:**

```15:59:packages/llm/src/mock.ts
function mockEmitTurn(content: string, system: string): Record<string, unknown> {
  if (content.includes('cancel') && system.includes('cancelorder')) {
    return { mode: 'plan', goal: '...', instructions: [{ tool: 'cancelOrder', ... }] };
  }
  // ... keyword branches only
}
```

**Mitigation:** Run the Real-LLM Smoke Test (below). Log every `emit_turn` payload, binding outcome, gate verdict, and final reply.

---

### 2. Provider forced-tool mappings (P1)

**Risk:** Forced `emit_turn` is the harness entry point (`packages/core/src/harness/planner.ts`). Each provider maps `toolChoice: { type: 'tool', name: 'emit_turn' }` differently:

| Provider | Mapping | File |
|----------|---------|------|
| Anthropic | `tool_choice: { type: 'tool', name: 'emit_turn' }` | `packages/llm/src/anthropic.ts` |
| OpenAI-compatible | `tool_choice` (function name) | `packages/llm/src/openai-compatible.ts` |
| Gemini | `functionCallingConfig: { mode: 'ANY', allowedFunctionNames: ['emit_turn'] }` | `packages/llm/src/gemini.ts` |

**Not yet validated live:** model returns structured tool call vs free text, schema compliance, token usage reporting into `BudgetMeter`, fallback chain behavior on 429/5xx.

---

### 3. Sunjet/Astrolobe storage E2E (P1)

**Risk:** With `sunjet.enabled: false` (default in `config.yaml`), Lighthouse falls back to in-process embedding rank. The following paths have **not** been exercised against a live `ll-server`:

- Registry mirror (`harness_tools`, `harness_capabilities`) — `packages/core/src/lighthouse/mirror.ts`
- Hybrid tool-graph search for binding
- Suspension mirror (`harness_suspensions`)
- Trace firehose (`harness_traces`) — `packages/core/src/harness/traces.ts`
- Dual-write message store under load

**Existing test:** `pnpm test:sunjet` (`scripts/test-sunjet-integration.mjs`) — requires `SUNJET_URL` (default `http://127.0.0.1:18080`) and bootstrapped tables. Uses mock LLM, not full harness turn.

---

### 4. Live browser multi-turn flows (P2)

**Risk:** Recoil (`awaiting_info`) and write-confirmation (`awaiting_confirmation`) resume paths are unit-tested but not manually verified in the widget:

- User sees confirmation buttons → clicks Confirm → plan resumes (not just single-tool legacy path)
- Recoil: planner asks for missing field → user answers → plan resumes with pinned value
- User says "no/cancel" mid-plan → suspension abandoned cleanly
- Stale registry hash after SDK reconnect → suspension discarded

**Covered by:** `scripts/test-harness-executor.mjs` (recoil round-trip, confirmation rehydration), `scripts/test-phase4-confirmation.mjs` (legacy mock path).

---

### 5. Load / concurrency (P2)

**Risk:** No soak test for:

- Many concurrent widget sessions (per-socket serialization only — not cross-customer)
- `MAX_SDK_CONNECTIONS=32` eviction under heartbeat churn
- Inbound worker `enqueueCustomerTurn` under WhatsApp burst
- SQLite write contention on `harness_ledger` / `suspended_plans`
- Sunjet timeout + `fallback_sqlite_on_error` under sustained errors

**Covered by:** Per-session lock logic, `requeueStaleJobs`, SDK connection cap (unit/integration logic only).

---

## Highest-Value Next Step: Real-LLM Smoke Test

### Setup

```bash
# 1. API keys
export ANTHROPIC_API_KEY=sk-ant-...
export VOYAGE_API_KEY=pa-...          # embeddings when using config.anthropic.yaml
export AELIO_SDK_SECRET=change-me-in-production

# 2. Start server with Anthropic config
AELIO_CONFIG="$(pwd)/config.anthropic.yaml" \
  AELIO_TEST_MODE=1 \
  pnpm --filter @aelio/server dev

# 3. Start sample SaaS backend (real tool registry)
AELIO_SDK_SECRET=change-me-in-production \
  AELIO_SERVER_URL=ws://127.0.0.1:3000 \
  pnpm --filter aelio-sample-saas start

# 4. Open widget
open http://localhost:3000/demo.html
```

**Alternative providers:**

```bash
# OpenAI
AELIO_CONFIG="$(pwd)/config.openai.yaml" OPENAI_API_KEY=sk-... pnpm --filter @aelio/server dev

# Gemini
AELIO_CONFIG="$(pwd)/config.gemini.yaml" GEMINI_API_KEY=... pnpm --filter @aelio/server dev
```

**Production config tweaks before go-live:**

- Set `channels.web.allowed_origins` to explicit origins (not `*`)
- Set `identity.allow_anonymous: false` + magic link for real identity
- Set `secret` / `AELIO_SDK_SECRET` to a strong value (production refuses `change-me-in-production`)

### Conversation Scenarios (run all four)

| # | User message | Expected harness behavior | Watch for |
|---|--------------|---------------------------|-----------|
| A | *"What's the status of my last order?"* | `plan` → bind `getOrderStatus` or `listOrders` → read tool → synthesize | Wrong tool binding; shallow `reply` without lookup |
| B | *"Cancel order A-1002"* | `plan` → `cancelOrder` → gate `needs_approval` → suspend → user confirms → resume → complete | Confirmation UI; plan resumes whole DAG, not one-shot legacy |
| C | *"Cancel my order"* (no order ID) | `plan` → recoil `needs_info` OR bind + gate asks for ID | Infinite recoil; wrong field name in `ask` |
| D | *"Book me a flight to Mars"* | `refuse` (feasibility probe) OR graceful "can't do that" | Hallucinated plan with nonexistent tools; runaway replans |

### What to capture per turn

Enable telemetry inspection:

```bash
# Turn-level API calls (includes planner + synthesis)
curl -s -H "Authorization: Bearer $AELIO_SDK_SECRET" \
  http://localhost:3000/api/v1/telemetry/turn-calls | jq .

# Diagnostics (dev)
AELIO_DIAGNOSTICS=1 curl -s http://localhost:3000/diagnostics | jq .
```

Log template (copy into Session Logs below):

```
### Turn YYYY-MM-DD HH:MM — Scenario X
- **User:** "..."
- **Provider / model:** anthropic / claude-sonnet-4-6
- **emit_turn:** { mode, goal, instructions[] }
- **degraded:** true/false
- **Binding:** capability → tool (score, ambiguous?)
- **Resolver:** DAG edges, recoil triggers
- **Gates:** allow | needs_approval | needs_info | deny_fatal
- **Outcome:** complete | suspend | blocked | replan | refuse
- **Final reply:** "..."
- **Issues:** none | describe
- **Recommendation:** none | describe
```

---

## Architecture Recommendations

| ID | Area | Recommendation | Priority | Rationale |
|----|------|----------------|----------|-----------|
| ARCH-001 | Planner validation | Add `scripts/test-real-llm-smoke.mjs` that runs scenarios A–D headlessly, asserts `emit_turn.mode` + tool names, records payloads to this log | P0 | Automates the #1 gap; mock tests cannot substitute |
| ARCH-002 | Provider matrix | CI job (manual/nightly) matrix: `config.anthropic.yaml`, `config.openai.yaml`, `config.gemini.yaml` × scenarios A–D | P1 | Forced-tool mappings differ per API |
| ARCH-003 | Sunjet live path | Extend `test:sunjet` to register SDK tools, run one harness turn with `sunjet.enabled: true`, verify mirror rows in `harness_tools` + trace append | P1 | Fallback-only testing hides ll-server integration bugs |
| ARCH-004 | Binding cache | Implement `harness_bindings` read/write (deferred in harness-spec) once real-LLM binding scores are logged | P2 | Reduces repeated semantic search cost |
| ARCH-005 | Progress streaming | SSE or WS `plan_progress` events during multi-step execution | P2 | 3-step checkout looks like a hang without it |
| ARCH-006 | Compensating actions | Per-tool rollback definitions for aborted mid-plan writes | P3 | Product decision: cart left populated vs auto-rollback |
| ARCH-007 | Load harness | k6 or scripted WS clients: N sessions × M turns, measure p95 turn latency + SQLite lock waits | P2 | Per-session lock untested at scale |
| ARCH-008 | Embedding dim consistency | When switching provider configs, verify `embeddings.output_dimension` matches `sunjet.embed_dim` and sqlite-vec table | P1 | Dimension mismatch silently breaks recall + Sunjet vectors |

---

## Logic Recommendations

| ID | Area | Recommendation | Priority | Rationale |
|----|------|----------------|----------|-----------|
| LOGIC-001 | Planner degrade path | Log `degraded: true` prominently in telemetry; consider user-facing "I had trouble planning" when degrade happens on `plan`-shaped requests | P1 | Silent degrade masks bad provider output |
| LOGIC-002 | Gemini step caps | Gemini function schemas lack enforced `maxItems`; harness enforces `max_instructions` in code — add explicit post-parse cap assertion in planner with metric | P1 | See `docs/system.md` §3.4 note on OpenAPI subset |
| LOGIC-003 | Recoil bounds | Verify `max_recoils_per_intent` triggers graceful exit in real conversations (scenario C) | P1 | Unit test exists; real LLM may loop differently |
| LOGIC-004 | Binding ambiguity | When top-two scores within `ambiguity_gap`, surface "I found multiple ways to do that" instead of picking wrong tool | P2 | Deferred batched disambiguation in harness-spec |
| LOGIC-005 | Registry hash invalidation | After SDK reconnect with changed `expose()` catalog, confirm suspended plans discard + user gets clear message | P1 | Stale-registry discard unit-tested; UX unverified |
| LOGIC-006 | Memory + harness | Confirm memory recall does not pollute planner with stale facts that change tool selection | P2 | Async extract; ordering vs turn start |
| LOGIC-007 | Fallback chain | Test `llm.fallback` activation: primary 429 → fallback completes turn; telemetry shows both calls | P2 | Config present in `config.anthropic.yaml` |
| LOGIC-008 | `transform` gate | Implement at least one built-in `transform` rule (arg caps) — enum exists, no rules yet | P3 | harness-spec deferred item |

---

## Testing Matrix

| Test | Command | LLM | Sunjet | Harness | Browser |
|------|---------|-----|--------|---------|---------|
| LLM provider wiring | `pnpm test:llm` | none | off | — | — |
| Phase 2 widget | `pnpm test:phase2` | mock | off | on | WS script |
| Phase 4 confirmation | `pnpm test:phase4:confirmation` | mock | off | on | WS script |
| Phase 5 memory | `pnpm test:phase5` | mock | off | on | — |
| Harness executor | `pnpm test:harness` | none | off | direct | — |
| Sunjet integration | `pnpm test:sunjet` | mock | **live** | partial | — |
| Full suite | `AELIO_TEST_MODE=1 pnpm test:all` | mock | off | on | — |
| Diagnostic | `pnpm diagnostic` | mock | off | on | — |
| Docker verify | `pnpm docker:verify` | mock | off | on | — |
| **Real-LLM smoke** | *manual — see above* | **live** | off | on | **human** |
| **Sunjet + harness** | *not automated* | live | **live** | on | — |
| **Load test** | *not implemented* | any | any | on | — |

---

## Accepted V1 Limitations (no fix required now)

| Item | Notes |
|------|-------|
| `destructive` safety blocked from chat | By design — admin channel future |
| Proactive / reflection daemon off by default | Opt-in |
| Sunjet off by default | Opt-in archival |
| Response cache off by default | Opt-in |
| Python SDK minimal | Node SDK is primary |
| SDK secret in URL query param | Deprecated; Bearer header preferred |
| Batched LLM disambiguation for ambiguous bindings | Deferred — graceful unbind today |
| `transform` policy verdict | Plumbing only |

---

## Open Issues

> Record confirmed bugs found during smoke tests. Move to **Resolved** when fixed.

| ID | Date | Severity | Summary | Area | Status |
|----|------|----------|---------|------|--------|
| — | — | — | *No open issues from live testing yet* | — | — |

---

## Resolved (from smoke / production testing)

| ID | Date | Summary | Fix | Verified |
|----|------|---------|-----|----------|
| LLM-ORG | 2026-07-10 | `@aelio/llm` reorganized into `providers/` + `embeddings/`; three chat providers config-selectable | `a628463` | `pnpm test:llm` |
| HAR-003 | 2026-07-10 | Tool failure halts before a dependent step (no hallucinated success) | `83e9850` | `test:harness` [13] |
| HAR-004 | 2026-07-10 | `presentFields` wired from customer metadata → harness gates | `83e9850` | build + typecheck |
| HAR-008 | 2026-07-10 | `maxTurnTokens` enforced via `BudgetMeter.noteUsage` on all LLM calls | `83e9850` | build + typecheck |
| HAR-012 | 2026-07-10 | `hashArgs` key-order canonicalized (idempotency dedup) | `83e9850` | `test:harness` [14] |
| CON-006 | 2026-07-10 | Confirmation unified into the suspension store (no dual path) | `4874df3` | `test:harness` [12], phase-4 |
| SEC-003 | 2026-07-10 | WhatsApp GET verify requires `verify_token` | `491eaf0` | code review |
| SEC-008 | 2026-07-10 | Default SDK secret refused in `NODE_ENV=production` at boot | `491eaf0` | code review |

> Still **not** run: the live real-LLM smoke (HAR-001) — blocked without a working
> provider key. Scenarios A–D below remain the highest-value next step; nothing in
> this session substitutes for one real-provider pass.

---

## Session Logs

> Append entries as you run real conversations. This is the primary artifact for architecture/logic follow-ups.

### Template

```markdown
## Session YYYY-MM-DD — [Provider] smoke test

**Config:** config.anthropic.yaml  
**Backend:** aelio-sample-saas  
**Tester:** name  
**Environment:** local | staging | prod-candidate

### Scenario A — Order lookup
- User: "..."
- emit_turn: ...
- Outcome: ...
- Issues: none | ...
- Action items: none | ARCH-xxx / LOGIC-xxx

### Scenario B — Cancellation with confirmation
...

### Scenario C — Missing info (recoil)
...

### Scenario D — Impossible request
...

### Session summary
- **Planner quality:** acceptable | needs work
- **Provider quirks:** none | describe
- **Blocking for prod:** yes/no — reason
```

---

### Session 2026-07-10 — Baseline (pre-smoke)

**Status:** Not yet run. Document created from codebase audit.

**Known code-level confidence:**

- Harness enabled by default (`config.yaml` → `harness.enabled: true`)
- Mock LLM drives full deep path when `emit_turn` forced (`packages/llm/src/mock.ts`)
- 10 harness unit checks pass (`pnpm test:harness`)
- Integration phases 2–5 pass on mock (`AELIO_TEST_MODE=1 pnpm test:all`)
- Security audit items BUG-001–047 closed ([bug-list.md](./bug-list.md))

**Awaiting:** Human-driven real-LLM smoke (scenarios A–D) + optional Sunjet live pass.

---

## Quick Reference — Key Files

| Concern | Path |
|---------|------|
| Planner (Pass 1) | `packages/core/src/harness/planner.ts` |
| Mock LLM | `packages/llm/src/mock.ts` |
| Anthropic provider | `packages/llm/src/anthropic.ts` |
| Gemini provider | `packages/llm/src/gemini.ts` |
| Lighthouse mirror | `packages/core/src/lighthouse/mirror.ts` |
| Suspension store | `packages/core/src/harness/suspension.ts` |
| Harness spec | `Blueprint/harness-spec.md` |
| Anthropic config | `config.anthropic.yaml` |
| Sunjet test config | `config.sunjet-test.yaml` |
| Sample SaaS tools | `examples/sample-saas/` |

---

## Decision Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-07-10 | Ship-blocking = real-LLM smoke pass, not more mock tests | Mock cannot validate `emit_turn` quality |
| 2026-07-10 | Sunjet live test is P1 but not ship-blocking if `sunjet.enabled: false` in prod | SQLite is authoritative; Sunjet is search/archive |
| 2026-07-10 | Browser click-through recommended for confirmation/recoil UX only | Logic covered by unit tests; UX is the unknown |

---

*Update this file after every smoke session. Promote patterns to ARCH/LOGIC IDs above. Move fixes to [fix-log.md](./fix-log.md) when implemented.*
