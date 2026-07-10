# Aelio Fix Log (Hinglish)

Local development aur integration testing ke dauran jo bugs fix kiye, unka record yahan hai.

---

## Web Widget — `data-customer-id` ignore ho rahi thi

**Package:** `apps/web-widget`

**Problem kya thi:**
- Widget ko script tag se `data-customer-id` aur `data-server-url` chahiye hota hai
- Code yeh bahut **late** read kar raha tha (`useMemo` / `DOMContentLoaded` ke baad)
- Tab tak `document.currentScript` **null** ho chuka hota hai
- Result: galat default values use hoti thi (`demo-user`, `window.location.origin`)

**Fix kya kiya:**
- Script load hote hi turant capture karo: `const EMBED_SCRIPT = document.currentScript`
- Baad mein `EMBED_SCRIPT.dataset` se values read karo

**Files:** `apps/web-widget/src/widget.tsx`

---

## Web Widget — message parsing aur list keys unstable thi

**Package:** `apps/web-widget`

**Problem kya thi:**
- Galat server message aane pe `JSON.parse` crash kar sakta tha
- Same message do baar aaye toh list keys duplicate ho jati thi (`role + content`)

**Fix kya kiya:**
- `JSON.parse` ko try/catch mein wrap kiya — invalid message ignore
- Message keys ab `role + index` use karti hain

**Files:** `apps/web-widget/src/widget.tsx`

---

## Web Widget — WebSocket fail hone pe koi error nahi dikhta tha

**Package:** `apps/web-widget`

**Problem kya thi:**
- WebSocket band ho jata tha (jaise origin block) lekin UI **"Connecting…"** pe hi atka rehta tha
- User ko pata hi nahi chalta tha kya galat hua

**Fix kya kiya:**
- `onclose` mein close code/reason handle kiya
- Chat panel mein error message dikhaya (origin blocked, server unreachable, etc.)
- `onerror` handler add kiya

**Files:** `apps/web-widget/src/widget.tsx`

---

## Aelio Server — dusre app se embed karte waqt origin block ho raha tha

**Package:** `apps/server` + `config.yaml`

**Problem kya thi:**
- Tumhara app `http://127.0.0.1:4173` pe chal raha tha
- Widget WebSocket `http://127.0.0.1:3000` (Aelio) pe connect karta hai
- Browser bhejta hai: `Origin: http://127.0.0.1:4173`
- `config.yaml` mein sirf `http://localhost:3000` allowed tha
- Server connection reject kar deta tha: **"Origin not allowed"**
- `ready` message kabhi nahi aata → **"Connecting…"** pe atka rehta

**Fix kya kiya:**
- `config.yaml` mein common local dev origins add kiye:
  - `http://127.0.0.1:3000` / `http://localhost:3000`
  - `http://127.0.0.1:4173` / `http://localhost:4173`
  - `http://127.0.0.1:5173` / `http://localhost:5173`

**Note:** Browser ke liye `127.0.0.1` aur `localhost` **alag-alag** hain — zarurat ho toh dono add karo.

**Files:** `config.yaml`

---

## Aelio Server — `/widget/ws` pe non-WebSocket request se crash

**Package:** `apps/server`

**Problem kya thi:**
- `HEAD` request `/widget/ws` pe aane se server crash ho raha tha
- Error: `Cannot read properties of undefined (reading 'remoteAddress')`
- Kyunki non-upgrade request mein `request.socket` undefined ho sakta hai

**Fix kya kiya:**
- Optional chaining use kiya: `request.socket?.remoteAddress ?? 'unknown'`

**Files:** `apps/server/src/routes/widget.ts`

---

## Aelio-Test — chat config mein galat server URL

**Project:** `Aelio-Test` (sibling test app)

**Problem kya thi:**
- `/api/aelio/chat-config` return kar raha tha: `serverUrl: http://127.0.0.1:4173`
- Yeh **test app ka URL** hai, Aelio server ka nahi
- Sahi hona chahiye: `http://127.0.0.1:3000`
- Widget galat jagah WebSocket khol raha tha

**Fix kya kiya:**
- `aelioHttpUrl()` helper add kiya (`ws://127.0.0.1:3000` → `http://127.0.0.1:3000`)
- `chat-config` ab `aelioHttpUrl()` return karta hai, `appHttpUrl()` nahi

**Files:** `Aelio-Test/server.mjs`

---

## Aelio-Test — WebSocket proxy ne `init` message kho diya (race condition)

**Project:** `Aelio-Test` (sibling test app)

**Problem kya thi:**
- Kabhi connection `4173` proxy se hota tha (`/widget/ws` → Aelio `3000`)
- Flow aisa tha:
  1. Browser connect → turant `init` bhej diya
  2. Proxy abhi Aelio se connect ho raha tha
  3. Pehla `init` message **beech mein kho gaya**
  4. Aelio ko pata hi nahi user aaya → `ready` nahi bheja
  5. UI **"Connecting…"** pe atka rehta

**Fix kya kiya:**
- Jab tak upstream connect nahi hota, client messages `pending` queue mein buffer karo
- Upstream `open` hone pe pending messages flush karo, phir relay start karo
- Full relay lagane se pehle temporary listener hata do

**Files:** `Aelio-Test/server.mjs`

---

## Symptom → Problem (quick check)

| Dikhe kya | Matlab kya hai |
|---|---|
| Input **"Connecting…"** pe atka | `ready` message nahi aaya — DevTools mein WS check karo |
| WS close code **1008** | Origin `allowed_origins` mein nahi hai |
| `init` gaya, reply nahi | Galat `data-server-url` ya proxy race issue |
| Galat customer ID | `currentScript` late read ho raha tha (widget mein fix) |
| `data-customer-id` ignore | Same issue — naya `widget.js` build karke use karo |

---

## Fix ke baad verify kaise karein

1. **Aelio-Convox:** `pnpm start` — latest `config.yaml` aur `widget.js` ke saath server restart
2. **Widget rebuild:** `pnpm --filter @aelio/web-widget build`
3. **Aelio-Test:** `Aelio-Test` folder mein `npm start` — `server.mjs` changes pick honge
4. Browser mein kholo: `http://127.0.0.1:4173/chat` → Mount chat → input pe **"Type a message…"** aana chahiye
5. DevTools → Network → WS → `init` ↑ aur `ready` ↓ confirm karo

---

## Ek line mein poora flow (ab kaise kaam karta hai)

```
Tumhara app (4173)
  → widget load
  → WebSocket Aelio (3000) pe connect
  → init bheja → ready mila ✅
  → "Type a message…" dikha
  → message → Aelio → SDK → backend → reply
```

---

---

## BUG-001..023 — Full bug-list pass (2026-07-09)

**Packages:** server, core, db, protocol, sdk-node, web-widget, docs, scripts, docker

### Security (P0/P1)

| Bug | Fix |
|-----|-----|
| BUG-001 identity spoofing | `/auth/verify` issues HMAC `sessionToken`; when `allow_anonymous: false`, widget init requires it and binds identity from claims |
| BUG-002 default secret | Production (`NODE_ENV=production`, not `AELIO_TEST_MODE`) refuses `change-me-in-production` |
| BUG-003 docker `*` origins | Removed; require `AELIO_ALLOWED_ORIGINS` / explicit list; production rejects empty/`*` |
| BUG-009 magic-link abuse | Bearer SDK secret + per-IP/email rate limit |
| BUG-010 telemetry leak | `/api/v1/telemetry/*` require Bearer secret |

### Correctness (P1/P2)

| Bug | Fix |
|-----|-----|
| BUG-004 concurrent turns | Per-socket promise chain serializes `processTurn` |
| BUG-005 WhatsApp HMAC | Route `preParsing` captures raw body for signature |
| BUG-006 memory timing | `await extractMemories(...)` before turn returns |
| BUG-007 embed dim | `createDatabase({ vectorDimensions })` from config; recreate vec table on change |
| BUG-008 schema split | Drizzle `0004_runtime_tables.sql`; removed runtime CREATE for those tables |
| BUG-014 SDK wipe | Stop full DELETE on boot; prune stale heartbeats only |
| BUG-015 dedup skip | Fallback hash key from `from|text` when `messageId` missing |
| BUG-016 silent vec fail | Startup `console.warn`; `/ready` includes `vectorIndex` + warning |

### SDK / Widget (P2)

| Bug | Fix |
|-----|-----|
| BUG-011 silent SDK drops | `{ type: 'error' }` on invalid JSON/schema |
| BUG-012 message length | Zod `.max(4096)` + widget `maxLength` |
| BUG-013 lifecycle errors | Validate catalog; send `ack` / `error` |
| BUG-017 reconnect | Exponential backoff; preserve messages |
| BUG-018 confirmation UI | Server sends `{ type: 'confirmation' }`; Confirm/Cancel buttons |

### Docs / tests (P3)

| Bug | Fix |
|-----|-----|
| BUG-019 | `AELIO_SDK_SECRET` in SDK README |
| BUG-020 | README layout adds `channels/`, `sunjet-client/` |
| BUG-021 | MANUAL.md telemetry + proactive + diagnostics |
| BUG-022 | Phase 6 in `run-tests.mjs` + `diagnostic.mjs` |
| BUG-023 | Deleted `scripts/e2e-widget-test.mjs` |

**Key files:** `apps/server/src/routes/{widget,auth,telemetry,sdk,whatsapp}.ts`, `apps/server/src/{config,auth,app,sdk-bridge}.ts`, `packages/core/src/identity/session-token.ts`, `packages/db/src/index.ts`, `packages/db/drizzle/0004_runtime_tables.sql`, `apps/web-widget/src/widget.tsx`, `packages/protocol/src/index.ts`

**Verify:** `AELIO_TEST_MODE=1 pnpm test:all` (Phases 2–6 passed).

---

## Pass 2 audit fixes (2026-07-09)

Full-repo audit found 24 additional bugs (BUG-024–BUG-047). All fixed in one pass.

### Concurrency / data integrity (P1)

| Bug | Fix |
|-----|-----|
| BUG-024 overlapping inbound turns | `enqueueCustomerTurn()` per `channel:from` key in inbound worker |
| BUG-025 cross-channel sessions | `findOrCreateSession` filters active session by `channel` |
| BUG-026 stuck processing jobs | `requeueStaleJobs()` sweeps locks older than 5 min |
| BUG-027 inbound retry duplicates | `turnCompleted` / `outboundEnqueued` flags in job payload |
| BUG-028 stale confirmations | 10 min TTL; clear on expired or unrelated user input |
| BUG-029 SDK connection storm | `MAX_SDK_CONNECTIONS=32`; invoke picks newest heartbeat |
| BUG-030 unregistered SDK zombies | `SDK_REGISTER_TIMEOUT_MS=15s` closes pre-register sockets |

### Security / lifecycle (P2)

| Bug | Fix |
|-----|-----|
| BUG-031 origin bypass | Strict origin when set; loopback + test mode exempt |
| BUG-032 rate-limit spoofing | Rate limit uses `request.ip`; `AELIO_TRUST_PROXY=1` optional |
| BUG-033 pre-register SDK ops | `requireRegistered()` guard on ingest/state/flow/result |
| BUG-034 re-init identity swap | Reject second `init` on same widget socket |
| BUG-035 shutdown leaks | `stopHeartbeat()` + `sdkBridge.shutdown()` in `onClose` |
| BUG-036 `/ready` info disclosure | SDK catalog redacted in production without Bearer auth |
| BUG-037 widget reconnect leak | Close previous socket before opening new one |
| BUG-038 SDK silent drop | `send()` logs warning when WS not open |
| BUG-039 SDK parallel reconnect | `reconnecting` mutex in `scheduleReconnect()` |
| BUG-040 magic-link TOCTOU | Atomic `UPDATE … RETURNING` on consume |
| BUG-041 ensureCustomer race | Catch unique violations; re-select existing row |
| BUG-042 proactive early sent | `recordProactiveDelivery()` in outbound worker after send |

### Polish (P3)

| Bug | Fix |
|-----|-----|
| BUG-043 timing-safe auth | `timingSafeEqual` in `authorized()` |
| BUG-044 frame size limit | `MAX_WS_FRAME_BYTES=64KB` on widget + SDK WS |
| BUG-045 session token TTL | `channels.web.magic_link.session_token_ttl_minutes` config |
| BUG-046 confirm regex | Exact-match patterns (`^yes$`, not `\b` suffix) |
| BUG-047 double confirm click | `pendingConfirmationIndex` disables buttons after click |

**Key files:** `packages/core/src/{job-queue,session/lifecycle,runtime/turn,runtime/customer-turn-queue,runtime/proactive,identity/magic-link,safety/confirmations}.ts`, `apps/server/src/{workers/inbound,outbound,routes/widget,routes/sdk,routes/health,routes/auth,sdk-bridge,app,auth,config}.ts`, `packages/sdk-node/src/index.ts`, `apps/web-widget/src/widget.tsx`, `packages/protocol/src/index.ts`

**Verify:** `pnpm typecheck` + `AELIO_TEST_MODE=1 pnpm test:all` (all phases passed).

---

*Last updated: 2026-07-09*
