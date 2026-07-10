# Aelio-Convox — Complete Project Documentation

> **Last updated:** 2026-07-09  
> **Repository:** Aelio-Convox (product name: **Aelio**)  
> **License intent:** Apache-2.0

This document is a comprehensive reference for the Aelio-Convox monorepo — architecture, packages, configuration, APIs, data model, flows, testing, and deployment. For step-by-step setup, see [MANUAL.md](./MANUAL.md). For fix history, see [fix-log.md](./fix-log.md).

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Monorepo Structure](#2-monorepo-structure)
3. [Architecture](#3-architecture)
4. [Core Components](#4-core-components)
5. [End-to-End Flows](#5-end-to-end-flows)
6. [Configuration Reference](#6-configuration-reference)
7. [Environment Variables](#7-environment-variables)
8. [HTTP & WebSocket API](#8-http--websocket-api)
9. [Database Schema](#9-database-schema)
10. [SDK Integration](#10-sdk-integration)
11. [Web Widget](#11-web-widget)
12. [WhatsApp Channel](#12-whatsapp-channel)
13. [Intelligence Features](#13-intelligence-features)
14. [Build, Run & Deploy](#14-build-run--deploy)
15. [Testing & Verification](#15-testing--verification)
16. [Development Phases](#16-development-phases)
17. [Dependencies](#17-dependencies)
18. [Related Documentation](#18-related-documentation)

---

## 1. Executive Summary

**Aelio** is an open-source conversational runtime for SaaS products. It provides:

1. **Conversational agent** — Real-time chat on Web and WhatsApp, LLM tool-calling into your backend via the SDK, and safety rails (read / write / destructive).
2. **Analytical agent** — Extracts user facts from conversations, stores them in SQLite + `sqlite-vec`, recalls context on future turns, and optionally runs a reflection daemon for session summaries.

**Key design principle:** The **Aelio SDK** runs inside *your* backend and dials *out* to the Aelio server over WebSocket. The server never calls your APIs directly and never holds your business credentials.

**Tech stack:** TypeScript · pnpm monorepo · Fastify · SQLite + Drizzle + sqlite-vec · optional Sunjet (ll-server) archival · single-container Docker deployment.

```
Customer (Web widget / WhatsApp)
        │
        ▼
┌───────────────────────────────────────┐
│  Aelio Server (apps/server)           │
│  Channels · Identity · Runtime        │
│  LLM · Memory · Safety · Job Queue    │
└───────────────────────────────────────┘
        ▲  WebSocket (SDK dials OUT)
        │
┌───────────────────────────────────────┐
│  Your Backend + @aelio/sdk            │
│  aelio.expose('getOrderStatus', ...)  │
│  Your auth · Your DB · Your logic     │
└───────────────────────────────────────┘
```

---

## 2. Monorepo Structure

```
Aelio-Convox/
├── apps/
│   ├── server/              # @aelio/server — Fastify HTTP/WS service
│   └── web-widget/          # @aelio/web-widget — Preact embeddable chat UI
├── packages/
│   ├── protocol/            # @aelio/protocol — Wire protocol + Zod schemas
│   ├── db/                  # @aelio/db — Drizzle SQLite schema + sqlite-vec
│   ├── llm/                 # @aelio/llm — LLM + embedding providers
│   ├── core/                # @aelio/core — Turn engine, memory, safety, storage
│   ├── channels/            # @aelio/channels — WhatsApp Meta/mock adapters
│   ├── sdk-node/            # @aelio/sdk — Published Node.js SDK
│   ├── sdk-python/          # Minimal Python SDK
│   └── sunjet-client/       # @aelio/sunjet-client — Sunjet ll-server HTTP client
├── examples/
│   ├── nodejs-express/      # Minimal Express + SDK (used by phase tests)
│   ├── sample-saas/         # ShopCo realistic SaaS demo
│   ├── python-fastapi/      # FastAPI + Python SDK
│   └── python-django/       # Django stub README
├── scripts/                 # start, test, diagnostic, demo, docker-verify
├── docker/                  # Dockerfile, compose, Railway/Render/Fly templates
├── docs/                    # MANUAL.md, fix-log.md, production-testing.md
├── Blueprint/               # v1-oss-spec, daemon-spec, hardening plan
└── config*.yaml             # Environment-specific configuration files
```

### Package Overview

| Package | NPM Name | Purpose |
|---------|----------|---------|
| `apps/server` | `@aelio/server` | Main server: health, SDK WS, widget WS, WhatsApp webhook, auth, workers, static files |
| `apps/web-widget` | `@aelio/web-widget` | Preact IIFE bundle → `apps/server/public/widget.js` |
| `packages/protocol` | `@aelio/protocol` | SDK↔server message types, Zod validation, constants |
| `packages/db` | `@aelio/db` | Drizzle schema, migrations, `createDatabase()` with sqlite-vec |
| `packages/llm` | `@aelio/llm` | Providers: mock, anthropic, openai, gemini, groq, ollama + fallback chain |
| `packages/core` | `@aelio/core` | `processTurn()`, memory, intent, lifecycle, job queue, Sunjet storage |
| `packages/channels` | `@aelio/channels` | WhatsApp webhook parser, HMAC verify, Meta/mock senders |
| `packages/sdk-node` | `@aelio/sdk` | Node SDK: `expose()`, `listen()`, `onSend()`, `ingest()`, lifecycle APIs |
| `packages/sunjet-client` | `@aelio/sunjet-client` | HTTP client for Sunjet ll-server |

---

## 3. Architecture

### Server Boot Sequence

Entry point: `apps/server/src/main.ts` → `createApp()` in `apps/server/src/app.ts`

1. Load and validate `config.yaml` (Zod schema in `apps/server/src/config.ts`)
2. Create SQLite database + run Drizzle migrations
3. Wire LLM provider chain (with optional fallback)
4. Configure embedding provider (hash or real model)
5. Initialize `ServerSdkBridge` for SDK communication
6. Optionally initialize Sunjet client (with SQLite fallback on failure)
7. Create WhatsApp sender (Meta, mock, or SDK BYO)
8. Register Fastify routes (health, SDK, widget, WhatsApp, auth, telemetry, proactive)
9. Start background workers:
   - **Inbound worker** — processes WhatsApp/webhook jobs
   - **Outbound worker** — sends WhatsApp replies
   - **Backup worker** — periodic SQLite backups (when enabled)
   - **Daemon worker** — reflection / proactive (when enabled)

### Runtime Dependencies Graph

```
apps/server
  ├── @aelio/core      (processTurn, memory, safety, intent, lifecycle)
  ├── @aelio/db        (SQLite + sqlite-vec)
  ├── @aelio/llm       (LLM + embeddings)
  ├── @aelio/channels  (WhatsApp)
  ├── @aelio/protocol  (message schemas)
  └── @aelio/sunjet-client (optional archival)

packages/core
  ├── @aelio/db
  ├── @aelio/llm
  ├── @aelio/protocol
  └── @aelio/sunjet-client

packages/sdk-node
  ├── ws
  └── zod
```

### Turn Processing Pipeline

Core function: `processTurn()` in `packages/core/src/runtime/turn.ts`

```
Incoming message
  → Resolve identity (phone/email/customerId)
  → Ensure customer record exists
  → Find or create session
  → Rate limit check
  → Persist user message
  → Handle pending write confirmations
  → Response cache lookup (if enabled)
  → Load conversation history
  → Compose system prompt (persona, lifecycle, memories, intent stack)
  → Run tool loop (LLM ↔ SDK invoke/result)
  → Persist assistant reply
  → Async memory extraction (fire-and-forget)
  → Return reply to channel
```

---

## 4. Core Components

### 4.1 Server (`apps/server`)

| Module | Path | Responsibility |
|--------|------|----------------|
| Config loader | `src/config.ts` | YAML + env var resolution, Zod validation |
| SDK bridge | `src/sdk-bridge.ts` | In-memory SDK connection registry, invoke/send routing |
| Routes | `src/routes/*.ts` | HTTP/WS endpoints per channel |
| Workers | `src/workers/*.ts` | Async job processing (inbound, outbound, backup, daemon) |
| Sunjet init | `src/sunjet.ts` | Optional ll-server connection + message store |
| Static assets | `public/` | `widget.js`, `demo.html` |

### 4.2 Core Runtime (`packages/core`)

| Module | Path | Responsibility |
|--------|------|----------------|
| Turn engine | `src/runtime/turn.ts` | Main message processing pipeline |
| Tool loop | `src/runtime/tool-loop.ts` | LLM tool call ↔ SDK invoke cycle |
| Safety | `src/safety/` | Read/write/destructive policy, confirmations |
| Memory | `src/analyst/` | Fact extraction + vector recall |
| Intent | `src/intent/stack.ts` | Conversational focus tracking |
| Lifecycle | `src/lifecycle/` | States, policies, flows from SDK |
| Job queue | `src/job-queue/` | Async inbound/outbound job management |
| Storage | `src/storage/` | Sunjet dual-write message store |
| Telemetry | `src/telemetry/` | Turn-level LLM/API call logging |
| Proactive | `src/runtime/proactive.ts` | Opt-in outbound messaging |
| Response cache | `src/runtime/response-cache.ts` | Semantic response caching |

### 4.3 LLM Layer (`packages/llm`)

Supported providers:

| Provider | Config value | Notes |
|----------|-------------|-------|
| Mock | `mock` | Offline demos, deterministic tool calls |
| Anthropic | `anthropic` | Claude models |
| OpenAI | `openai` | GPT models |
| Gemini | `gemini` | Google models |
| Groq | `groq` | Fast inference |
| Ollama | `ollama` | Local models via Ollama API |

Embedding providers: `hash` (local, no API), `openai`, `anthropic` (Voyage), `gemini`, `ollama`.

Fallback chain: primary provider fails → fallback provider (configured in `llm.fallback`).

### 4.4 Database (`packages/db`)

- **Engine:** better-sqlite3
- **ORM:** Drizzle
- **Vector search:** sqlite-vec extension
- **Migrations:** `packages/db/drizzle/` (0000–0003)
- **Runtime tables:** Some tables created via `CREATE IF NOT EXISTS` in `createDatabase()` (memory_vec, inbound_dedup, etc.)

---

## 5. End-to-End Flows

### 5.1 Web Widget Chat

```
1. Page loads <script src=".../widget.js" data-customer-id="..." data-server-url="...">
2. Widget captures document.currentScript synchronously at load time
3. Opens WebSocket to wss://<server>/widget/ws
4. Server validates Origin against channels.web.allowed_origins
5. Client sends { type: 'init', customerId, email? }
6. Server responds { type: 'ready', customerId }
7. User sends { type: 'message', content }
8. Server: processTurn() → typing indicators → assistant reply
9. Example: "what is my order status?" → mock LLM tool call → SDK getOrderStatus → "shipped"
```

**Key files:**
- Widget: `apps/web-widget/src/widget.tsx`
- Server route: `apps/server/src/routes/widget.ts`
- Turn engine: `packages/core/src/runtime/turn.ts`

### 5.2 WhatsApp

```
1. Meta GET /wa/webhook — subscription verification (hub.verify_token)
2. Meta POST /wa/webhook — inbound message
3. Optional x-hub-signature-256 HMAC verification
4. parseWhatsAppWebhook() extracts message
5. Dedup via claimInboundMessage('wa:<messageId>')
6. Enqueue inbound job
7. Inbound worker: resolveWhatsAppIdentity(phone) → processTurn()
8. Enqueue outbound job
9. Outbound worker: MetaWhatsAppSender or MockWhatsAppSender or SDK onSend
```

**Key files:**
- Webhook: `apps/server/src/routes/whatsapp.ts`
- Workers: `apps/server/src/workers/inbound.ts`, `outbound.ts`
- Channel adapters: `packages/channels/`

### 5.3 SDK Integration

```
1. Your backend: aelio.expose('getOrderStatus', handler, schema)
2. await aelio.listen({ secret, url: 'ws://server/sdk' })
3. SDK sends { type: 'register', functions: [...], states, policies, flows, persona }
4. On tool call: server sends { type: 'invoke', ... } → SDK runs handler → { type: 'result', ... }
5. Heartbeat: server pings every 30s, timeout at 60s
6. Auto-reconnect with exponential backoff (max 30s)
```

**Authentication:** `Authorization: Bearer <AELIO_SDK_SECRET>` (query `?secret=` deprecated).

**Key files:**
- SDK: `packages/sdk-node/src/index.ts`
- Server route: `apps/server/src/routes/sdk.ts`
- Bridge: `apps/server/src/sdk-bridge.ts`

### 5.4 Magic Link Auth

```
1. POST /auth/magic-link { email, externalId? } → returns URL (+ token in test mode)
2. Token stored as SHA-256 hash in magic_links table
3. GET /auth/verify?token=... → consumes link, returns { customerId, externalId, email }
4. Widget init uses verified externalId + email as channel address
```

**Key files:**
- Routes: `apps/server/src/routes/auth.ts`
- Logic: `packages/core/src/identity/magic-link.ts`

### 5.5 Memory & Recall

```
1. After each turn, extractMemories() runs async (not awaited)
2. Rule-based fact detection: email, name, phone, preference statements
3. Facts embedded (hash or real model) → stored in memory table + sqlite-vec index
4. Next turn: recallMemories() vector-searches top-K facts → injected into system prompt
```

**Key files:**
- Extract: `packages/core/src/analyst/extract.ts`
- Recall: `packages/core/src/analyst/recall.ts`

### 5.6 Write Confirmation Flow

```
1. User asks to cancel order
2. LLM calls cancelOrder (safety mode: write)
3. Server pauses, sends confirmation prompt to user
4. User confirms → SDK invoke proceeds → order cancelled
5. User denies → action aborted, assistant explains
```

Configured via `safety.require_confirmation_for: [write, destructive]` and per-function overrides.

---

## 6. Configuration Reference

Primary file: `config.yaml` (validated by Zod in `apps/server/src/config.ts`).

### Config Variants in Repo

| File | Purpose |
|------|---------|
| `config.yaml` | Local dev (mock LLM, hash embeddings) |
| `config.docker.yaml` | Docker deployment |
| `config.openai.yaml` | OpenAI LLM provider |
| `config.anthropic.yaml` | Anthropic LLM provider |
| `config.gemini.yaml` | Gemini LLM provider |
| `config.byo.yaml` | Bring-your-own WhatsApp provider |
| `config.daemon.yaml` | Reflection daemon enabled |
| `config.proactive.yaml` | Proactive messaging enabled |
| `config.followup.yaml` | Proactive follow-up config |
| `config.cache.yaml` | Response cache enabled |
| `config.sunjet-test.yaml` | Sunjet integration testing |

### Full Schema Sections

| Section | Key Options |
|---------|-------------|
| `name`, `secret` | Instance name; SDK secret (`${AELIO_SDK_SECRET}`) |
| `llm` | `provider`, `model`, `api_key`, `base_url`, `max_tokens`, `system_prompt`, `fallback` |
| `embeddings` | `provider` (hash/openai/anthropic/gemini/ollama), `model`, `api_key`, `output_dimension` |
| `channels.whatsapp` | `enabled`, `provider` (meta/sdk), `mock_mode`, `phone_number_id`, `access_token`, `verify_token`, `app_secret`, `webhook_path` |
| `channels.web` | `enabled`, `allowed_origins`, `magic_link.enabled/ttl_minutes` |
| `safety` | `default_mode`, `require_confirmation_for`, `overrides`, `rate_limit` |
| `identity` | `mapping_function` (phone/email), `allow_anonymous` |
| `session` | `idle_timeout_minutes`, `history_window`, `summarize_after` |
| `intent` | `enabled`, `ttl_minutes`, `max_depth` |
| `memory` | `enabled`, `recall_limit` |
| `daemon` | `enabled`, `interval_minutes`, `max_per_cycle`, `reflect_min_messages`, `proactive_followup` |
| `proactive` | `enabled`, `require_opt_in`, `max_per_customer_per_day`, `window_hours` |
| `cache` | `enabled`, `similarity_threshold`, `ttl_minutes` |
| `sunjet` | `enabled`, `url`, `api_key`, `embed_dim`, `timeout_ms`, `dual_write_sqlite`, `fallback_sqlite_on_error`, `tables.*` |
| `storage` | `database_path`, `backup.enabled/interval_hours/retain_count` |
| `logging` | `level`, `format` |
| `server` | `host`, `port` |

Env refs in YAML use `${VAR_NAME}` syntax, resolved at load time.

---

## 7. Environment Variables

| Variable | Purpose |
|----------|---------|
| `AELIO_SDK_SECRET` | Shared SDK↔server authentication secret |
| `AELIO_CONFIG` | Path to config file (use absolute path) |
| `AELIO_MIGRATIONS_PATH` | Override Drizzle migrations directory |
| `AELIO_PUBLIC_PATH` | Override static/public directory |
| `AELIO_TEST_MODE` | Enables `/__test__/*` endpoints + magic link token leak |
| `AELIO_DIAGNOSTICS` | Enables `/diagnostics` endpoint |
| `AELIO_VERSION` | Version string in health response |
| `AELIO_SERVER_URL` | SDK WebSocket URL (examples/scripts) |
| `AELIO_WS_URL` | Widget WS URL for tests |
| `AELIO_LLM` | Force mock LLM in demo script |
| `AELIO_DOCKER_CONTAINER` | Docker verify container name |
| `AELIO_DATABASE_PATH` | Drizzle kit database path |
| `ANTHROPIC_API_KEY` | Anthropic LLM API key |
| `OPENAI_API_KEY` | OpenAI LLM/embedding API key |
| `GEMINI_API_KEY` | Google Gemini API key |
| `GROQ_API_KEY` | Groq API key |
| `VOYAGE_API_KEY` | Voyage embedding API key (Anthropic embeddings) |
| `SUNJET_API_KEY` | Sunjet ll-server authentication |
| `SUNJET_URL` | Sunjet test server URL |

---

## 8. HTTP & WebSocket API

### HTTP Endpoints

| Method | Path | Auth | Purpose |
|--------|------|------|---------|
| GET | `/health` | None | Liveness probe |
| GET | `/ready` | None | Readiness (db, migrations, widget, sunjet, sdk) |
| GET | `/diagnostics` | None (diag mode) | Memory, paths, SDK functions |
| GET | `/widget.js` | None | Serve widget bundle |
| GET | `/demo.html` | None | Static demo page |
| GET | `/telemetry` | None | Telemetry HTML UI |
| GET | `/api/v1/telemetry/events` | None | Sunjet conversation events |
| GET | `/api/v1/telemetry/turn-calls` | None | Turn API call records |
| GET | `/wa/webhook` | Meta verify token | WhatsApp subscription challenge |
| POST | `/wa/webhook` | Optional HMAC | WhatsApp inbound messages |
| POST | `/auth/magic-link` | None | Issue magic link |
| GET | `/auth/verify` | Token query param | Verify magic link |
| POST | `/proactive` | Bearer secret | Send proactive message (when enabled) |
| POST | `/proactive/opt-in` | Bearer secret | Set customer opt-in (when enabled) |
| GET | `/__test__/whatsapp/outbox` | None (test mode) | Mock WhatsApp sent messages |
| GET | `/__test__/sdk/functions` | None (test mode) | Registered SDK function names |

### WebSocket Endpoints

| Path | Auth | Protocol |
|------|------|----------|
| `/widget/ws` | Origin allowlist | Client: `{init, message}` → Server: `{ready, typing, message, error}` |
| `/sdk` | Bearer `AELIO_SDK_SECRET` | Full SDK protocol: `register`, `invoke`, `result`, `ping/pong`, `ingest`, `send`, `set_state`, `set_flow_progress` |

---

## 9. Database Schema

Schema definition: `packages/db/src/schema.ts`  
Migrations: `packages/db/drizzle/` (0000–0003)

### Tables

| Table | Purpose |
|-------|---------|
| `customers` | Internal customer records; `external_id` is SaaS user ID |
| `channel_addresses` | Phone/email per channel; `verified_at` for magic links |
| `sessions` | Per-channel conversation sessions; `metadata` holds intent stack + lifecycle |
| `messages` | Chat history (user/assistant/system/tool) |
| `memory` | Extracted facts with optional embedding JSON |
| `memory_vec` / `memory_vec_index` | sqlite-vec virtual table for semantic recall |
| `function_calls` | Audit log of all SDK invocations |
| `sdk_connections` | Persisted SDK connection metadata (cleared on server boot) |
| `magic_links` | Hashed tokens for email auth |
| `job_queue` | Async inbound/outbound job processing |
| `reflections` | Daemon session reflection outcomes |
| `proactive_messages` | Sent/blocked proactive message log |
| `response_cache` | Per-customer semantic response cache |
| `turn_api_calls` | Per-turn LLM/embed telemetry |
| `inbound_dedup` | Webhook message deduplication (7-day prune) |

---

## 10. SDK Integration

### Node.js (`@aelio/sdk`)

```typescript
import { aelio } from '@aelio/sdk'

aelio.expose('getOrderStatus', async (args, ctx) => {
  // ctx.customerId — resolved Aelio customer
  return { status: 'shipped', trackingNumber: 'TRK-123' }
}, {
  description: 'Get the current order status for the customer',
  parameters: { type: 'object', properties: { orderId: { type: 'string' } } },
  safety: { mode: 'read' },
})

await aelio.listen({
  secret: process.env.AELIO_SDK_SECRET!,
  url: process.env.AELIO_SERVER_URL ?? 'ws://127.0.0.1:3000',
})
```

### SDK API Surface

| Method | Purpose |
|--------|---------|
| `aelio.expose(name, handler, schema)` | Register a function for LLM tool-calling |
| `aelio.listen({ secret, url })` | Connect to Aelio server WebSocket |
| `aelio.onSend(handler)` | BYO channel: handle outbound message delivery |
| `aelio.ingest(message)` | Push inbound messages from custom channels |
| `aelio.setState(state)` | Update customer lifecycle state |
| `aelio.setFlowProgress(flow, step)` | Update flow progress |

### Safety Modes

| Mode | Behavior |
|------|----------|
| `read` | Executes immediately |
| `write` | Requires user confirmation (if configured) |
| `destructive` | Blocked from chat channels in V1 |

---

## 11. Web Widget

### Embed Code

```html
<script
  src="https://your-aelio-server.com/widget.js"
  data-customer-id="user_abc123"
  data-server-url="https://your-aelio-server.com"
></script>
```

### Data Attributes

| Attribute | Required | Description |
|-----------|----------|-------------|
| `data-customer-id` | Yes | Your SaaS user ID (external ID) |
| `data-server-url` | Yes | Aelio server base URL (not your app URL) |

### Build

```bash
pnpm --filter @aelio/web-widget build
# Output: apps/server/public/widget.js (IIFE bundle via Vite)
```

### WebSocket Protocol

**Client → Server:**
```json
{ "type": "init", "customerId": "user_123", "email": "user@example.com" }
{ "type": "message", "content": "What is my order status?" }
```

**Server → Client:**
```json
{ "type": "ready", "customerId": "user_123" }
{ "type": "typing", "active": true }
{ "type": "message", "role": "assistant", "content": "Your order has shipped.", "turnId": "..." }
{ "type": "error", "message": "..." }
```

---

## 12. WhatsApp Channel

### Modes

| Mode | Config | Behavior |
|------|--------|----------|
| Mock | `mock_mode: true` | Captures outbound in memory; test via `/__test__/whatsapp/outbox` |
| Meta | `provider: meta` + credentials | Real WhatsApp Cloud API |
| SDK BYO | `provider: sdk` | Your backend delivers via `aelio.onSend()` |

### Webhook Setup

1. Configure Meta app with webhook URL: `https://your-server/wa/webhook`
2. Set `verify_token` in config (must match Meta dashboard)
3. Set `app_secret` for HMAC signature verification
4. Configure `phone_number_id` and `access_token` for outbound

### Identity Mapping

Phone numbers map to persistent customer records via `identity.mapping_function: phone`. Same phone across messages → same customer.

---

## 13. Intelligence Features

### Memory (Phase 5)

- **Extract:** Rule-based fact detection after each turn (async)
- **Store:** Facts + embeddings in SQLite + sqlite-vec
- **Recall:** Vector search top-K facts injected into system prompt
- **Config:** `memory.enabled`, `memory.recall_limit`

### Intent Stack

- Tracks conversational focus frames with TTL and max depth
- Stored in `sessions.metadata.intentStack`
- SDK functions can declare `intent` category
- **Config:** `intent.enabled`, `intent.ttl_minutes`, `intent.max_depth`

### Lifecycle (Phase 6)

- SDK registers states, policies, and flows
- Server tracks customer state and flow progress
- Injected into system prompt for context-aware responses

### Reflection Daemon (opt-in)

- Periodic background worker analyzes closed sessions
- Generates session summaries and reflections
- **Config:** `daemon.enabled`, `daemon.interval_minutes`

### Proactive Messaging (opt-in)

- Send outbound messages to customers (with opt-in)
- Rate-limited per customer per day
- **Config:** `proactive.enabled`, `proactive.require_opt_in`

### Response Cache (opt-in)

- Semantic similarity cache for repeated questions
- **Config:** `cache.enabled`, `cache.similarity_threshold`

### Sunjet Archival (opt-in)

- Primary message store in Sunjet ll-server
- Dual-write to SQLite for fast reads
- Graceful fallback on Sunjet outage
- **Config:** `sunjet.enabled`, `sunjet.dual_write_sqlite`, `sunjet.fallback_sqlite_on_error`

---

## 14. Build, Run & Deploy

### Prerequisites

- Node.js ≥ 20 (22 LTS recommended)
- pnpm 9
- Optional: Docker, Python ≥ 3.9, Sunjet ll-server (Rust)

### Quick Start

```bash
pnpm install
pnpm build
export AELIO_SDK_SECRET=change-me-in-production
pnpm start   # Starts server + example SDK automatically
```

### Manual Dev Workflow

```bash
# Terminal 1 — Aelio server
export AELIO_SDK_SECRET=change-me-in-production
AELIO_CONFIG="$(pwd)/config.yaml" pnpm --filter @aelio/server dev

# Terminal 2 — Example backend with SDK
export AELIO_SDK_SECRET=change-me-in-production
AELIO_SERVER_URL=ws://127.0.0.1:3000 pnpm --filter aelio-example-express start

# Demo: http://localhost:3000/demo.html
```

### Root Scripts

| Command | Description |
|---------|-------------|
| `pnpm install` | Install workspace dependencies |
| `pnpm build` | Build web-widget first, then all packages via Turbo |
| `pnpm start` | `scripts/start.mjs` — install, build if needed, start server + SDK |
| `pnpm dev` | `turbo run dev` (all packages in watch mode) |
| `pnpm typecheck` | TypeScript check all packages |
| `pnpm lint` | Lint all packages |
| `pnpm diagnostic` | Full health/build/shutdown/integration suite |
| `pnpm demo` | Demo with optional real LLM |
| `pnpm docker:build` | Build `aelio/server:latest` image |
| `pnpm docker:up` | `docker compose up --build` |
| `pnpm docker:verify` | Build + run container + phase tests |

### Docker

- **Dockerfile:** `docker/Dockerfile` (multi-stage: Node 22 build → distroless runtime ~190MB)
- **Volume:** `/data` for SQLite persistence
- **Config:** Uses `config.docker.yaml` with `allowed_origins: "*"`
- **Deploy templates:** `docker/railway.toml`, `docker/render.yaml`, `docker/fly.toml`

---

## 15. Testing & Verification

### Test Scripts

| Script | npm alias | Verifies |
|--------|-----------|----------|
| `run-tests.mjs` | `test:all` | Spawns isolated server+SDK, runs phases 2–5 |
| `test-phase2-widget.mjs` | `test:phase2`, `test:e2e` | Widget WS → mock LLM → SDK tool call |
| `test-phase3-whatsapp.mjs` | `test:phase3` | Webhook verify + inbound + mock outbound |
| `test-phase4-confirmation.mjs` | `test:phase4:confirmation` | Write confirmation gate |
| `test-phase4-magic-link.mjs` | `test:phase4:magic-link` | Magic link + widget session |
| `test-phase4-whatsapp-identity.mjs` | `test:phase4:identity` | Persistent phone identity |
| `test-phase5-memory.mjs` | `test:phase5` | Memory extract + recall |
| `test-phase6-lifecycle.mjs` | `test:phase6:lifecycle` | Lifecycle catalog (manual) |
| `test-sunjet-integration.mjs` | `test:sunjet` | Sunjet ll-server E2E |
| `diagnostic.mjs` | `diagnostic` | Build, artifacts, health, shutdown, all phases |
| `docker-verify.mjs` | `docker:verify` | Container build + phase tests |

### Running Tests

```bash
# All phases (auto-starts server + SDK)
AELIO_TEST_MODE=1 pnpm test:all

# Individual phase
AELIO_TEST_MODE=1 pnpm test:phase2

# Full diagnostic
pnpm diagnostic
```

---

## 16. Development Phases

The project is organized into incremental development phases, each with automated tests:

| Phase | Feature | Test Script |
|-------|---------|-------------|
| Phase 2 | Web widget → LLM → SDK tool call | `test-phase2-widget.mjs` |
| Phase 3 | WhatsApp webhook → job queue → mock send | `test-phase3-whatsapp.mjs` |
| Phase 4a | Write action confirmation flow | `test-phase4-confirmation.mjs` |
| Phase 4b | Magic link → verified widget session | `test-phase4-magic-link.mjs` |
| Phase 4c | Phone → persistent WhatsApp customer | `test-phase4-whatsapp-identity.mjs` |
| Phase 5 | Memory extract/recall in chat | `test-phase5-memory.mjs` |
| Phase 6 | SDK lifecycle catalog (states/policies/flows) | `test-phase6-lifecycle.mjs` |

---

## 17. Dependencies

### Root

| Package | Purpose |
|---------|---------|
| turbo | Monorepo build orchestration |
| typescript, tsx | Tooling |
| ws | Test scripts WebSocket client |
| drizzle-orm | Dev dep for phase 5 test |

### Server (`@aelio/server`)

| Package | Purpose |
|---------|---------|
| fastify, @fastify/websocket, @fastify/static | HTTP/WS server |
| pino | Logging |
| yaml, zod | Config parsing |

### Core (`@aelio/core`)

| Package | Purpose |
|---------|---------|
| drizzle-orm | DB queries |
| Workspace packages | db, llm, protocol, sunjet-client |

### Database (`@aelio/db`)

| Package | Purpose |
|---------|---------|
| better-sqlite3 | SQLite engine |
| sqlite-vec | Vector index extension |
| drizzle-orm, drizzle-kit | ORM + migrations |

### Widget (`@aelio/web-widget`)

| Package | Purpose |
|---------|---------|
| preact | UI framework |
| vite, @preact/preset-vite | Build tooling |

---

## 18. Related Documentation

| Document | Description |
|----------|-------------|
| [README.md](../README.md) | Quick start and overview |
| [MANUAL.md](./MANUAL.md) | Complete setup and operations guide |
| [fix-log.md](./fix-log.md) | Bug fix history (Hinglish) |
| [production-testing.md](./production-testing.md) | Client trial checklist |
| [Blueprint/v1-oss-spec.md](../Blueprint/v1-oss-spec.md) | Full OSS specification |
| [bug-list.md](./bug-list.md) | Open bugs and fix priorities |

---

*Generated from codebase analysis on 2026-07-09.*
