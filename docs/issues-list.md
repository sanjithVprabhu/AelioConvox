# Aelio-Convox — Full Repository Issues List

> **Audit date:** 2026-07-10  
> **Scope:** End-to-end review of **265 source files** (TypeScript, Rust, Python, scripts, configs, Blueprint) across `server/`, `packages/`, `sdk/`, `chat/`, `examples/`, `scripts/`, `Sunjet/Astrolobe/`, and root configs.  
> **Companion:** [production-readiness-log.md](./production-readiness-log.md) · [bug-list.md](./bug-list.md) · [fix-log.md](./fix-log.md)

---

## Critical finding: documentation vs code drift

`docs/bug-list.md` reports **0 open bugs** and **49 fixes** (BUG-001–047, Pass 1–2). A line-by-line audit of the current tree shows **many listed fixes are not present in code**. Examples:

| bug-list claim | Expected fix | Actual code state |
|----------------|--------------|-------------------|
| BUG-001 widget identity spoofing | HMAC `sessionToken` from `/auth/verify` | `authToken` accepted in schema, **never validated** (`server/src/routes/widget.ts:17,125`) |
| BUG-002 default secret rejected in prod | Refuse `change-me-in-production` | **No check** in `server/src/config.ts` or `main.ts` |
| BUG-010 telemetry leak | Bearer auth on `/api/v1/telemetry/*` | **No auth** (`server/src/routes/telemetry.ts`) |
| BUG-005 WhatsApp HMAC raw body | `preParsing` hook | Still `JSON.stringify(request.body)` (`server/src/routes/whatsapp.ts:28`) |
| BUG-009 magic-link abuse | SDK secret + rate limit | **No auth, no rate limit** (`server/src/routes/auth.ts`) |
| BUG-018 confirmation UI | Confirm/Cancel buttons | **Not in** `chat/src/widget.tsx`; users must type yes/no |
| BUG-024–026 concurrency | `enqueueCustomerTurn`, `requeueStaleJobs` | **Symbols absent** from codebase |
| BUG-029–030 SDK limits | `MAX_SDK_CONNECTIONS`, register timeout | **Not implemented** |
| BUG-038 SDK silent drop | Warning on `send()` when disconnected | `sdk/node/src/index.ts:476–478` still **silent no-op** |
| BUG-011 SDK error responses | `{ type: 'error' }` on invalid messages | SDK route **logs and returns** (`server/src/routes/sdk.ts:72–97`) |

**Action:** Treat this issues list as the **source of truth for open work**. Reconcile or retract `bug-list.md` entries that do not match code.

---

## Summary

| Severity | Count | Ship-blocking? |
|----------|-------|----------------|
| **P0** | 8 (was 12) | Yes — security + planner unproven |
| **P1** | 25 (was 28) | Yes for production hardening |
| **P2** | 39 | Recommended before scale |
| **P3** | 18 | Tech debt / polish |
| **Doc drift** | 15+ | Misleading if not corrected |
| **Fixed 2026-07-10** | **7** | SEC-003/008, HAR-003/004/008/012, CON-006 (see Resolved) |
| **Total open** | **~89** | |

### Smoke test session (2026-07-10)

| Check | Result | Notes |
|-------|--------|-------|
| `pnpm build` (after `pnpm install`) | ✅ Pass | First attempt failed — see **RUN-001** |
| `AELIO_TEST_MODE=1 pnpm test:all` | ✅ Pass | Harness unit (12) + phases 2–5 |
| `pnpm test:phase6:lifecycle` | ✅ Pass | Weak assertion — mock generic reply accepted (**TST-005**) |
| `node scripts/demo.mjs` (live) | ✅ Starts | Server `:3000`, sample-saas `:8081`, 6 SDK tools registered |
| Real-LLM smoke | ⛔ Blocked | No working OpenAI key in env — demo fell back to mock (**HAR-001**) |
| Live mock scenarios A–D (WebSocket) | ✅ Pass* | *C/D don't exercise recoil/refuse — see **SMK-002**, **SMK-003** |
| Telemetry probe | 🔴 Open | `GET /api/v1/telemetry/turn-calls` → 200, no auth (**SEC-002**) |

---

## P0 — Critical (fix or explicitly accept before production)

### Security

| ID | Issue | Location | Notes |
|----|-------|----------|-------|
| SEC-001 | **Widget customer impersonation** — any `customerId` accepted on `init`; `authToken` logged but never verified | `server/src/routes/widget.ts:96–129` | Magic link verify returns identity but issues no bound session credential |
| SEC-002 | **Telemetry APIs unauthenticated** — turn calls, Sunjet events expose conversation data | `server/src/routes/telemetry.ts:14–78` | `/telemetry` HTML also public |
| ✅ SEC-003 | **WhatsApp webhook verify bypass** — if `verify_token` unset, `undefined === undefined` subscribes webhook | `server/src/routes/whatsapp.ts:20` | `verify_token` optional in config schema |
| SEC-004 | **WhatsApp HMAC on re-serialized JSON** — not raw body; signatures fail or `app_secret` disabled | `server/src/routes/whatsapp.ts:28–34` | Meta requires byte-identical body |
| SEC-005 | **WhatsApp POST unsigned when `app_secret` unset** — fake inbound messages accepted | `server/src/routes/whatsapp.ts:30` | Entire HMAC block gated on secret |
| SEC-006 | **Magic link issuance unauthenticated** — no SDK secret, no rate limit, email bombing | `server/src/routes/auth.ts:17–39` | Returns full URL (token in query) |
| SEC-007 | **Production Docker ships `allowed_origins: ["*"]`** | `config.docker.yaml:17–18` | Any site can embed widget |
| ✅ SEC-008 | **Default SDK secret not rejected at boot** — `change-me-in-production` works in `NODE_ENV=production` | `server/src/config.ts`, `server/src/main.ts` | docker-compose defaults to it |

### Harness / LLM (production confidence)

| ID | Issue | Location | Notes |
|----|-------|----------|-------|
| HAR-001 | **Real-LLM planning quality unproven** — mock uses keyword heuristics only | `packages/llm/src/mock.ts` | Cannot validate `emit_turn` from Claude/GPT/Gemini |
| HAR-002 | **Provider forced-tool mappings not live-tested** | `packages/llm/src/{anthropic,openai-compatible,gemini}.ts` | `toolChoice: { type: 'tool', name: 'emit_turn' }` per provider |
| ✅ HAR-003 | **Failed SDK invoke still returns `complete`** — executor continues plan after tool error | `packages/core/src/harness/executor.ts:286–320` | Downstream steps may run with missing outputs; synthesis may hallucinate success |
| ✅ HAR-004 | **`presentFields` never wired from turn** — lifecycle `requires_fields` guards always empty | `packages/core/src/harness/gates.ts`, `packages/core/src/runtime/turn.ts` | `HarnessRunInput.presentFields` optional; no caller sets it |

### Deployment

| ID | Issue | Location | Notes |
|----|-------|----------|-------|
| DEP-001 | **Fly.io template missing `AELIO_SDK_SECRET`** | `server/fly.toml` | Render/Railway set it; Fly deploy likely broken |

---

## P1 — High

### Security & auth

| ID | Issue | Location |
|----|-------|----------|
| SEC-009 | Test/diagnostic routes leak internals when `AELIO_TEST_MODE=1` or `AELIO_DIAGNOSTICS=1` | `server/src/routes/test.ts`, `health.ts`, `auth.ts:38` |
| SEC-010 | Magic link token in URL query — referrer/log leakage | `packages/core/src/identity/magic-link.ts` |
| SEC-011 | SDK `?secret=` query param still supported (deprecated, hits logs) | `server/src/routes/sdk.ts:39–54` |
| SEC-012 | `/ready` exposes full SDK catalog (function/state/flow names) | `server/src/routes/health.ts` |
| SEC-013 | `ll-server` Docker runs open mode without `LL_API_KEYS` | `Sunjet/Astrolobe/crates/ll-server/src/main.rs` |

### Concurrency & data integrity

| ID | Issue | Location |
|----|-------|----------|
| CON-001 | **Stuck job queue** — no lock timeout/reclaim for `processing` jobs | `packages/core/src/job-queue/index.ts` |
| CON-002 | **Inbound worker unbounded parallelism** — 250ms poll spawns overlapping `processTurn` | `server/src/workers/inbound.ts:15–59` |
| CON-003 | **Widget WS no per-socket turn serialization** — rapid messages interleave | `server/src/routes/widget.ts:79–206` |
| CON-004 | **Session lock in-process only** — multi-instance races | `packages/core/src/runtime/session-lock.ts` |
| CON-005 | **SDK connections wiped on server restart** | `server/src/sdk-bridge.ts:54` |
| ✅ CON-006 | **Dual confirmation paths** — legacy `pendingConfirmation` in `turn.ts` runs before harness suspension resume | `packages/core/src/runtime/turn.ts:306–365` |

### Harness (logic & persistence)

| ID | Issue | Location |
|----|-------|----------|
| HAR-005 | **`harness_ledger` table never written** — idempotency in-memory only; crash = duplicate writes | `packages/db/drizzle/0004_odd_ezekiel.sql`, `packages/core/src/harness/executor.ts` |
| HAR-006 | **Segment replan not implemented** — `ExecOutcome` has `replan` kind but executor never returns it | `packages/core/src/harness/executor.ts:46` |
| HAR-007 | **Resolve failure does not replan** — comment promises replan; returns static apology | `packages/core/src/harness/index.ts:166–173` |
| ✅ HAR-008 | **`BudgetMeter.noteUsage()` never called** — `maxTurnTokens` unenforced | `packages/core/src/harness/budgets.ts:60`, LLM paths |
| HAR-009 | **Hard policies prompt-only** — `severity: 'hard'` never evaluated in gates | `packages/core/src/lifecycle/index.ts:200`, `packages/core/src/harness/gates.ts` |
| HAR-010 | **Sunjet/Astrolobe E2E untested from TS harness path** — mirror, tool search, traces only hit in-process fallback | `packages/core/src/lighthouse/mirror.ts`, default `sunjet.enabled: false` |

### SDK & channels

| ID | Issue | Location |
|----|-------|----------|
| SDK-001 | **Node SDK silently drops WS messages** when socket not OPEN | `sdk/node/src/index.ts:475–479` |
| SDK-002 | **SDK state/flow updates fire-and-forget** — no ack/error to SDK | `server/src/routes/sdk.ts:144–190` |
| SDK-003 | **SDK ingest for `web` channel dead path** — outbound worker no-ops web | `server/src/routes/sdk.ts:239`, `server/src/workers/outbound.ts` |
| SDK-004 | **Ingest dedup only when `messageId` provided** | `server/src/routes/sdk.ts:229–238` |
| SDK-005 | **Python SDK lacks Node parity** — no guards/transitions/persona parity | `sdk/python/sdk.py` vs `sdk/node/src/index.ts` |

### Runtime / bootstrap (observed 2026-07-10)

| ID | Issue | Location |
|----|-------|----------|
| RUN-001 | **Fresh clone build can fail** — `@aelio/core` DTS/runtime errors `Cannot find module 'zod'` until `pnpm install` links deps into `packages/core/node_modules` | `packages/core/package.json`, pnpm workspace |
| RUN-002 | **Real-LLM smoke blocked locally** — `demo.mjs` key probe failed; `.env` has placeholder keys; no Anthropic/Gemini smoke run | `scripts/demo.mjs`, `.env.example` |
| SMK-002 | **Mock scenario C skips recoil** — "Cancel my order" plans `cancelOrder` with `orderId: last` instead of `needs_info` | `packages/llm/src/mock.ts` — confirms **HAR-001** |
| SMK-003 | **Mock scenario D skips refuse** — "Book flight to Mars" returns generic help, not `mode: refuse` | `packages/llm/src/mock.ts` — confirms **HAR-001** |

### Testing & CI

| ID | Issue | Location |
|----|-------|----------|
| TST-001 | **No CI** — no `.github/workflows` | repo root |
| TST-002 | **`test:all` excludes Phase 6, Sunjet, real LLM** | `scripts/run-tests.mjs` |
| TST-003 | **`docker:verify` / `diagnostic` same gaps** | `scripts/docker-verify.mjs`, `diagnostic.mjs` |
| TST-004 | **No automated real-LLM smoke test** | — |

### Examples & deploy

| ID | Issue | Location |
|----|-------|----------|
| EX-001 | **Python FastAPI example broken** — `aelio.run()` blocks; uvicorn never starts | `examples/python-fastapi/main.py` |
| DEP-002 | **Docker image is server-only** — no SDK; tools fail without separate process | `server/Dockerfile` |
| DEP-003 | **Shipped config uses mock LLM** — silent degraded prod if keys not set | `config.docker.yaml` |

### Schema

| ID | Issue | Location |
|----|-------|----------|
| DB-001 | **DDL duplication** — `reflections`, `proactive_messages`, `response_cache`, `inbound_dedup` in migrations + runtime `CREATE TABLE` | `packages/db/src/index.ts`, `packages/db/drizzle/` |

---

## P2 — Medium

### Widget & UX

| ID | Issue | Location |
|----|-------|----------|
| UX-001 | **No Confirm/Cancel buttons in chat widget** — write confirmations require typing yes/no | `chat/src/widget.tsx` |
| UX-002 | **No live browser multi-turn recoil/confirmation click-through tested** | manual only |
| UX-003 | **Widget allows re-init / identity swap** — no reject-second-init guard | `server/src/routes/widget.ts` |
| UX-004 | **Internal errors leaked to widget** — `error.message` sent over WS | `server/src/routes/widget.ts:200` |
| UX-005 | **No message length limit in widget** — server may reject | `chat/src/widget.tsx` |
| UX-006 | **`sample-saas` demo user in `onboarding`** — blocks upgrade/cancel in demo prompts | `examples/sample-saas/src/index.ts`, `server/public/demo.html` |

### Harness & runtime

| ID | Issue | Location |
|----|-------|----------|
| HAR-011 | Binder ambiguity unresolved — no disambiguation LLM | `packages/core/src/harness/binder.ts` |
| ✅ HAR-012 | `hashArgs` uses `JSON.stringify` — key order breaks idempotency dedup | `packages/core/src/harness/executor.ts` |
| HAR-013 | Suspension `clear()` does not mirror delete to Sunjet | `packages/core/src/harness/suspension.ts` |
| HAR-014 | Binding cache table defined but not used | `Blueprint/harness-spec.md`, `server/src/config.ts` |
| HAR-015 | `transform` gate verdict — enum exists, no built-in rules | `packages/core/src/harness/gates.ts` |
| HAR-016 | Planner degrade path silent — `degraded: true` not surfaced to user/telemetry prominently | `packages/core/src/harness/planner.ts` |

### Safety & confirmations

| ID | Issue | Location |
|----|-------|----------|
| SAF-001 | Confirmation regex false positives — `ok`, `sure` prefix match | `packages/core/src/safety/confirmations.ts:11` |
| SAF-002 | Tenant-specific `cancelOrder` success text in core | `packages/core/src/safety/confirmations.ts:39–43` |
| SAF-003 | Pending confirmation TTL / unrelated-input clear **not verified in code** | `packages/core/src/session/confirmations.ts` — check if implemented |

### Performance & scale

| ID | Issue | Location |
|----|-------|----------|
| PERF-001 | **No load/concurrency testing** beyond session-lock logic | — |
| PERF-002 | `lookupCachedResponse` O(n) scan per customer — no vector index | `packages/core/src/runtime/response-cache.ts` |
| PERF-003 | Backup worker `VACUUM INTO` may stall writers under load | `server/src/workers/backup.ts` |
| PERF-004 | Embedding dimension mismatch pads/truncates silently | `packages/db/src/index.ts`, `packages/core/src/analyst/embeddings.ts` |

### SDK & server

| ID | Issue | Location |
|----|-------|----------|
| SDK-006 | Multiple SDK connections — function merge last-wins, no conflict warning | `server/src/sdk-bridge.ts` |
| SDK-007 | `sendViaChannel` picks first `canSend` — no channel affinity | `server/src/sdk-bridge.ts` |
| SDK-008 | No `MAX_SDK_CONNECTIONS` cap | — |
| SDK-009 | No WS frame size limit on widget/SDK | — |
| SDK-010 | No `requireRegistered()` before ingest/state/flow | `server/src/routes/sdk.ts` |

### Channels

| ID | Issue | Location |
|----|-------|----------|
| CH-001 | WhatsApp webhook parser — unchecked cast, no Zod; malformed → `[]` | `packages/channels/src/whatsapp/parser.ts` |
| CH-002 | WhatsApp sender receipt/typing payload shapes unverified vs Meta API | `packages/channels/src/whatsapp/sender.ts` |

### Analyst & memory

| ID | Issue | Location |
|----|-------|----------|
| MEM-001 | `extractMemories` ignores assistant reply — misses assistant-stated facts | `packages/core/src/analyst/extract.ts` |
| MEM-002 | `reflectOnSession` parses `followup` but never enqueues proactive send | `packages/core/src/analyst/reflect.ts` |

### Testing

| ID | Issue | Location |
|----|-------|----------|
| TST-005 | Phase 6 weak — any assistant reply passes; no state-gate assertion | `scripts/test-phase6-lifecycle.mjs` |
| TST-006 | Integration uses minimal `nodejs-express` (3 tools), not `sample-saas` | `scripts/run-tests.mjs` |
| TST-007 | `e2e-widget-test.mjs` orphaned — not in npm scripts | `scripts/e2e-widget-test.mjs` |
| TST-008 | `@aelio/chat` zero automated tests | `chat/` |
| TST-009 | No tests for daemon, proactive, cache, BYO WhatsApp configs | `config.*.yaml` |
| TST-010 | Sunjet Rust tests isolated from `pnpm test:all` | `Sunjet/Astrolobe/crates/**/tests/` |

### Config & docs

| ID | Issue | Location |
|----|-------|----------|
| CFG-001 | 11 `config.*.yaml` files — no index of purpose | root |
| CFG-002 | Provider configs omit explicit `harness`/`intent` blocks | `config.openai.yaml`, etc. |
| CFG-003 | Sunjet URL port mismatch — `8080` vs test `18080` | `config.yaml`, `config.sunjet-test.yaml` |
| CFG-004 | `config.logging.format` defined but ignored | `server/src/config.ts`, `server/src/app.ts` |
| CFG-005 | Root `pnpm lint` — no package defines `lint` script | `package.json`, `turbo.json` |
| CFG-006 | `@aelio/chat` excluded from `pnpm typecheck` | `chat/package.json` |
| DOC-001 | Blueprint paths stale (`apps/server`, `packages/sdk-node`) | `Blueprint/v1-oss-spec.md` |
| DOC-002 | Hardening plan claims `test:all (6/6) + sunjet` — false | `Blueprint/production-hardening-plan.md` |
| DOC-003 | README Sunjet submodule claim — no `.gitmodules` | `README.md` |
| DOC-004 | `project-documentation.md` references `apps/web-widget`, `MANUAL.md` paths may drift | `docs/project-documentation.md` |
| DOC-005 | `demo.html` links `/telemetry` — misleading when Sunjet off | `server/public/demo.html` |

### Schema

| ID | Issue | Location |
|----|-------|----------|
| DB-002 | `suspended_plans` / `harness_ledger` no FK to `sessions` | `packages/db/src/schema.ts` |
| DB-003 | `customers.externalId` unique allows multiple NULLs (SQLite) | `packages/db/src/schema.ts` |
| DB-004 | `EMIT_TURN_INPUT_SCHEMA` hand-maintained mirror of Zod — drift risk | `packages/core/src/harness/schema.ts` |

### Deploy

| ID | Issue | Location |
|----|-------|----------|
| DEP-004 | No compose recipe for ll-server + Aelio together | — |
| DEP-005 | `docker-verify` runs with `AELIO_TEST_MODE=1` — unsafe if copied to prod | `scripts/docker-verify.mjs` |
| DEP-006 | Committed `server/public/widget.js` can drift from `chat` build | `server/public/widget.js` |
| DEP-007 | `fly.toml` `min_machines_running = 0` — cold starts | `server/fly.toml` |

### Proactive

| ID | Issue | Location |
|----|-------|----------|
| PR-001 | Proactive blocked sends return HTTP 200 | `server/src/routes/proactive.ts:74` |

---

## P3 — Low / tech debt

| ID | Issue | Location |
|----|-------|----------|
| TD-001 | Mock LLM embeds tenant-specific tool heuristics in core package | `packages/llm/src/mock.ts` |
| TD-002 | Stale harness comment — "Phase 5 persists suspension" (implemented) | `packages/core/src/harness/index.ts:71` |
| TD-003 | Expired `suspended_plans` cleaned only on `get()`, not sweeper | `packages/core/src/harness/suspension.ts` |
| TD-004 | Python SDK reconnects forever — no graceful `disconnect()` | `sdk/python/sdk.py` |
| TD-005 | Embed telemetry errors swallowed | `packages/core/src/analyst/embeddings.ts` |
| TD-006 | `assertWithinRateLimit` throws generic `Error` | `packages/core/src/runtime/rate-limit.ts` |
| TD-007 | Django example README stub only | `examples/python-django/` |
| TD-008 | `.env.example` missing `SUNJET_API_KEY`, `VOYAGE_API_KEY`, etc. | `.env.example` |
| TD-009 | Phase 5 uses `tsx`; phases 2–4 use `node` | `package.json` |
| TD-010 | `apps/web-widget/` dead scaffold — only `node_modules` | `apps/` |
| TD-011 | `identity.allowAnonymous` in types unused in resolve path | `packages/core/src/identity/resolve.ts` |
| TD-012 | Sunjet archive reconciliation after fallback not implemented | `packages/core/src/runtime/turn.ts` (comment) |
| TD-013 | Richer state guard predicates deferred | `packages/protocol/src/index.ts` |
| TD-014 | No HTTP CORS/helmet — acceptable for WS-heavy server | `server/src/app.ts` |
| TD-015 | `replan.ts`, `store.ts`, `cycle-detect.ts` in glob — verify exports used | `packages/core/src/harness/` |
| TD-016 | `run-telemetry-test-server.mjs` has no verification script | `scripts/` |
| TD-017 | Express examples port collision undocumented | `examples/nodejs-express`, `examples/sample-saas` |
| TD-018 | `destructive` safety blocked from chat — accepted V1 limitation | by design |

---

## Repository map (audited)

| Area | Files | Role |
|------|-------|------|
| `server/` | 22 TS + public assets | Fastify HTTP/WS, workers, config |
| `packages/core/` | ~70 TS | Turn engine, harness, safety, analyst |
| `packages/db/` | schema + 5 migrations | SQLite + Drizzle + sqlite-vec |
| `packages/llm/` | 12 modules | Providers + embeddings + mock |
| `packages/protocol/` | 1 module | Zod wire schemas |
| `packages/channels/` | WhatsApp adapter | Parser, sender, verify |
| `packages/sunjet-client/` | HTTP client | Astrolobe table API |
| `sdk/node/` | Node SDK | Primary integration surface |
| `sdk/python/` | Python SDK | Minimal subset |
| `chat/` | Preact widget | Built → `server/public/widget.js` |
| `examples/` | 4 backends | Express, sample-saas, FastAPI, Django stub |
| `scripts/` | 16 test/run scripts | Phase tests, harness, sunjet, docker |
| `Sunjet/Astrolobe/` | Rust crates | ll-server, index, query, daemon |
| `Blueprint/` | Specs | harness-spec, v1-oss-spec, daemon |
| `docs/` | 5 markdown files | This list + readiness log + drifted bug-list |
| Root | 11 `config.*.yaml` | Environment profiles |

---

## What is solid (preserve)

- **Harness architecture** — plan → bind → resolve → execute → synthesize; dependency DAG from schemas, not LLM ordering (`packages/core/src/harness/resolver.ts`)
- **Harness unit tests** — 10 checks for spine/river/gates/recoil (`scripts/test-harness-executor.mjs`)
- **Protocol layer** — comprehensive Zod schemas (`packages/protocol/src/index.ts`)
- **Gate ordering** — deterministic safety before invoke (`packages/core/src/harness/gates.ts`)
- **Suspension/resume design** — registry hash staleness, ledger replay (`packages/core/src/harness/resume.ts`)
- **SDK WebSocket auth** — Bearer header, invalid secret rejected (`server/src/routes/sdk.ts`)
- **Widget origin policy** — diagnosable `origin_not_allowed` (`server/src/routes/widget.ts`)
- **Config validation** — Zod + env interpolation (`server/src/config.ts`)
- **Graceful shutdown** — SIGTERM handling (`server/src/shutdown.ts`)
- **Sunjet boot fallback** — degrades to SQLite (`server/src/app.ts`)
- **Proactive guardrails** — opt-in, caps, WhatsApp window (`packages/core/src/runtime/proactive.ts`)
- **Intent stack** — spine frames protected from TTL eviction (`packages/core/src/intent/stack.ts`)
- **Phase integration tests** — 2–5 pass on mock LLM (`AELIO_TEST_MODE=1 pnpm test:all`)

---

## Recommended fix order

### Before any production traffic

1. **SEC-001** — Bind widget `init` to verified session token (magic link → signed JWT/HMAC)
2. **SEC-002** — Authenticate telemetry APIs or disable in production
3. **SEC-003–005** — WhatsApp: require `verify_token` + raw-body HMAC
4. **SEC-006–008** — Magic link hardening; reject default secret; tighten docker origins
5. **HAR-001–002** — Real-LLM smoke test (see [production-readiness-log.md](./production-readiness-log.md))

### Before scale / multi-step harness in prod

6. **HAR-003–005** — Executor halt on tool failure; wire `presentFields`; persist `harness_ledger`
7. **CON-001–004** — Job reclaim, per-customer serialization, multi-instance queue
8. **HAR-008–010** — Token budget wiring; Sunjet live harness test

### Before claiming "production ready" in docs

9. **Reconcile bug-list.md** with this file — retract or implement claimed fixes
10. **TST-001–004** — CI + real-LLM smoke in nightly matrix
11. **DOC-001–005** — Fix Blueprint/README path drift

---

## How to use this file

1. Pick an issue ID (e.g. `SEC-001`)
2. Fix in a focused change
3. Move row to **Resolved** section below with PR/commit reference
4. Update [fix-log.md](./fix-log.md) for significant fixes
5. Re-run `AELIO_TEST_MODE=1 pnpm test:all && pnpm test:harness`

---

## Resolved

| ID | Date | Fix | Commit | Verified |
|----|------|-----|--------|----------|
| SEC-003 | 2026-07-10 | WhatsApp GET verify requires `verify_token` set — closes `undefined === undefined` subscribe bypass | `491eaf0` | Build + code review |
| SEC-008 | 2026-07-10 | `main.ts` refuses to boot in `NODE_ENV=production` with the default/empty SDK secret; warns in dev | `491eaf0` | Build + code review |
| HAR-003 | 2026-07-10 | Failed tool whose output a later step needs now HALTS the plan (blocked, non-fatal → reason surfaced) instead of continuing/hallucinating; failures without dependents stay best-effort | `83e9850` | `test:harness` [13] |
| HAR-004 | 2026-07-10 | `presentFields` wired end-to-end — `getCustomerPresentFields` (customer metadata keys) → `turn.ts` → harness gates, so `requires_fields` guards evaluate | `83e9850` | Build + typecheck |
| HAR-008 | 2026-07-10 | `maxTurnTokens` enforced — planner/synthesis return usage; harness feeds every LLM call into `BudgetMeter.noteUsage`; feasibility replan gated on token + clock budgets | `83e9850` | Build + typecheck |
| HAR-012 | 2026-07-10 | `hashArgs` canonicalizes (recursively sorts) keys — `{a,b}` and `{b,a}` dedup as one call; idempotency no longer breaks on key order | `83e9850` | `test:harness` [14] |
| CON-006 | 2026-07-10 | Confirmation unified into the suspension store; the legacy `pendingConfirmation` path only runs for the legacy tool loop (harness owns it via the store, no dual-path conflict) | `4874df3` | `test:harness` [12], phase-4 |
| LLM-ORG | 2026-07-10 | `@aelio/llm` reorganized into `providers/` + `embeddings/`; three chat providers (Anthropic/OpenAI/Gemini) config-selectable with loud key errors; `test:llm` proves construction | `a628463` | `test:llm` |

> **Fixed this session (2026-07-10):** SEC-003, SEC-008, HAR-003, HAR-004, HAR-008,
> HAR-012, CON-006, plus the `@aelio/llm` clean-separation refactor. All verified
> by `pnpm build && pnpm typecheck && AELIO_TEST_MODE=1 pnpm test:all` (which now
> also runs `test:llm` + the 14-check `test:harness`). **Remaining P0/P1 items
> below are still open** — most notably the real-LLM smoke (HAR-001/002),
> widget-identity binding (SEC-001), telemetry auth (SEC-002), and the remaining
> WhatsApp HMAC hardening (SEC-004/005).

---

*Generated from full-repo audit on 2026-07-10. Re-audit after major merges.*
