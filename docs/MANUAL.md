# Aelio — Complete Instruction Manual

Aelio is an open-source **conversational runtime for SaaS products**. You install the
Aelio SDK in your existing backend, expose a few functions, and Aelio gives your
customers a chat experience on **Web** and **WhatsApp** — with tool-calling, a
read/write/destructive safety model, long-term per-customer memory, a self-improving
reflection daemon, and (opt-in) proactive outreach.

This manual covers everything: setup, configuration, wrapping your APIs, channels,
deployment, and operations.

---

## Table of contents

1. [How it works (architecture)](#1-how-it-works)
2. [Prerequisites](#2-prerequisites)
3. [Quick start (local, ~5 minutes)](#3-quick-start)
4. [Wrapping your APIs with the SDK](#4-wrapping-your-apis-with-the-sdk)
5. [Configuration reference (`config.yaml`)](#5-configuration-reference)
6. [LLM providers & embeddings](#6-llm-providers--embeddings)
7. [Channels](#7-channels)
8. [The safety model](#8-the-safety-model)
9. [Memory, reflection daemon, proactive outreach, cache](#9-intelligence-features)
10. [HTTP & WebSocket endpoints](#10-endpoints)
11. [Deployment](#11-deployment)
12. [Publishing the SDKs](#12-publishing-the-sdks)
13. [Testing & verification](#13-testing)
14. [Troubleshooting](#14-troubleshooting)

---

## 1. How it works

Two processes, connected by one persistent WebSocket:

```
  Customer (WhatsApp / Web widget)
        │
        ▼
  ┌─────────────────────────────────────────┐
  │ Aelio server (you run this)              │
  │  channels · identity · runtime + safety  │
  │  LLM providers · SQLite + sqlite-vec     │
  │  memory · daemon · proactive · cache     │
  └─────────────────────────────────────────┘
        ▲   persistent WebSocket (SDK dials OUT)
        │
  ┌─────────────────────────────────────────┐
  │ Your backend (you add the SDK)           │
  │  aelio.expose('getOrderStatus', ...)     │
  │  runs YOUR code with YOUR auth + DB       │
  └─────────────────────────────────────────┘
```

**Key idea:** the SDK dials *out* to the Aelio server. The server never calls your
API directly and never holds your credentials. When the LLM decides to call one of
your functions, the server sends an `invoke` message down the WebSocket; your SDK runs
the function in your process and returns the result.

**Three separate secrets — don't conflate them:**

| Secret | Who issues it | Purpose |
|---|---|---|
| `AELIO_SDK_SECRET` | You make it up | Authenticates your SDK ↔ Aelio server |
| LLM API key | Anthropic / OpenAI / Groq | Aelio ↔ the model provider |
| WhatsApp credentials | Meta (or your provider) | Aelio ↔ WhatsApp |

---

## 2. Prerequisites

- **Node.js ≥ 20** (22 LTS recommended)
- **pnpm 9** (`corepack enable` then `corepack prepare pnpm@9 --activate`)
- An **LLM API key** (Anthropic, OpenAI, or Groq) — or **Ollama** for fully local, or
  the built-in **`mock`** provider for offline demos
- (Optional) **Docker** for container deployment
- (Optional) **Python ≥ 3.9** if you use the Python SDK

---

## 3. Quick start

From the repo root:

```bash
pnpm install
pnpm build          # builds the widget + all packages
```

Set a shared secret (you invent this):

```bash
export AELIO_SDK_SECRET=change-me-in-production
```

**Terminal 1 — start the Aelio server** (uses the bundled `config.yaml`, which defaults
to the offline `mock` LLM so it runs with no API key):

```bash
AELIO_CONFIG="$(pwd)/config.yaml" AELIO_SDK_SECRET=$AELIO_SDK_SECRET \
  pnpm --filter @aelio/server dev
```

> Always set `AELIO_CONFIG` to the absolute path of your config — the server resolves
> `./config.yaml` relative to its own working directory otherwise.

**Terminal 2 — start the example backend with the SDK:**

```bash
AELIO_SDK_SECRET=$AELIO_SDK_SECRET AELIO_SERVER_URL=ws://127.0.0.1:3000 \
  pnpm --filter aelio-example-express start
```

**Try it:** open the widget demo at **http://localhost:3000/demo.html**, click **Chat**,
and ask:

> what is my order status?

You'll see: web widget → Aelio runtime → LLM tool call → SDK `getOrderStatus` → reply.

**Health checks:**

```bash
curl localhost:3000/health   # liveness
curl localhost:3000/ready    # db, migrations, sdk, memory, channels, llm
```

To use a real model instead of mock, edit `config.yaml` (see §6) and set the key:

```bash
export ANTHROPIC_API_KEY=sk-ant-...   # or OPENAI_API_KEY / GROQ_API_KEY
```

---

## 4. Wrapping your APIs with the SDK

This is the core of the integration. You expose functions; Aelio offers them to the
LLM as tools and runs them in your backend on demand.

### Node / TypeScript (`@aelio/sdk`)

```bash
npm install @aelio/sdk
```

```typescript
import { aelio } from '@aelio/sdk'

// A read function — the assistant can call it freely.
aelio.expose('getOrderStatus', async ({ orderId }, ctx) => {
  // ctx.customerId is YOUR user id — scope queries exactly as you normally would.
  return await db.orders.findOne({ id: orderId, userId: ctx.customerId })
}, {
  description: 'Get the current status of a customer order',
  params: { orderId: 'string' },
  safety: 'read',
})

// A write function — Aelio asks the customer to confirm before it runs.
aelio.expose('cancelOrder', async ({ orderId }, ctx) => {
  return await db.orders.cancel(orderId, { userId: ctx.customerId })
}, {
  description: 'Cancel a pending order',
  params: { orderId: 'string' },
  safety: 'write',
})

await aelio.listen({
  secret: process.env.AELIO_SDK_SECRET,
  url: process.env.AELIO_SERVER_URL ?? 'ws://127.0.0.1:3000',
})
```

That's the whole integration. The SDK auto-reconnects (exponential backoff) and
heartbeats; concurrent invocations are correlated by id.

**The handler signature** is `(args, ctx) => Promise<result>`, where `ctx` is:

```typescript
{
  customerId: string       // your user id, resolved from the channel
  sessionId: string
  channel: string          // 'whatsapp' | 'web' | your custom channel
  channelAddress: string   // phone / email / etc.
  locale?: string
  metadata?: Record<string, unknown>
}
```

### Declaring parameters

Each entry in `params` can be one of three interchangeable forms:

| Form | Example | Meaning |
|---|---|---|
| Shorthand | `'string'` | required, type only |
| Optional shorthand | `'string?'` | optional (trailing `?`) |
| Object | `{ type, description?, optional?, enum?, format?, items? }` | full control |

```typescript
params: {
  customerRef: 'string',                                   // required
  status: 'string?',                                       // optional
  limit: { type: 'number', optional: true },
  sort: { type: 'string', enum: ['newest', 'oldest'], description: 'Sort order', optional: true },
  tags: { type: 'array', items: 'string', optional: true },
}
```

Supported types: `string`, `number`, `integer`, `boolean`, `array`, `object`.

Aelio passes required-vs-optional to the model correctly (it won't pester users for
optional values), **validates and coerces** arguments before calling your handler
(e.g. `"5"` → `5`, `"true"` → `true`), and never invokes your function with a required
argument missing.

### Python (`aelio-sdk`)

```bash
pip install aelio-sdk          # or: pip install websockets  (single-module dev use)
```

```python
import os
from sdk import Aelio

aelio = Aelio(secret=os.environ["AELIO_SDK_SECRET"], url="ws://127.0.0.1:3000")

@aelio.expose("getOrderStatus", description="Get the status of a customer order",
              params={"orderId": "string"}, safety="read")
async def get_order_status(args, ctx):
    return {"orderId": args["orderId"], "status": "shipped", "customerId": ctx["customerId"]}

aelio.run()   # or, inside an existing async app: asyncio.create_task(aelio.listen())
```

Same wire protocol, same param forms, same safety levels as the Node SDK.

### API surface

| Method | Purpose |
|---|---|
| `expose(name, handler, schema)` | Register a callable function/tool |
| `listen({ secret, url? })` | Connect to the server (auto-reconnect + heartbeat) |
| `onSend(handler)` | Deliver outbound messages via your own provider (BYO channel — §7) |
| `ingest({ channel, from, text })` | Push an inbound message from your own webhook (§7) |
| `disconnect()` | Drain and close |

---

## 5. Configuration reference

Everything is driven by one YAML file. The server validates it on boot and refuses to
start on an invalid config, pointing at the exact field. `${VARS}` are resolved from the
environment.

```yaml
name: my-saas-conversational
secret: ${AELIO_SDK_SECRET}          # the SDK uses this to authenticate

# ---- LLM ----
llm:
  provider: anthropic                # anthropic | openai | groq | ollama | mock
  model: claude-sonnet-4-6
  api_key: ${ANTHROPIC_API_KEY}      # not needed for mock/ollama
  max_tokens: 4096
  base_url: ...                      # optional (OpenAI-compatible gateways)
  fallback:                          # optional secondary provider
    provider: openai
    model: gpt-4o-mini
    api_key: ${OPENAI_API_KEY}

# ---- Embeddings (memory recall + response cache) ----
embeddings:
  provider: hash                     # hash (built-in, no deps) | openai | ollama
  model: text-embedding-3-small
  api_key: ${OPENAI_API_KEY}
  base_url: ...

# ---- Channels ----
channels:
  whatsapp:
    enabled: true
    provider: meta                   # meta (built-in adapter) | sdk (bring-your-own)
    mock_mode: false                 # true = don't actually call Meta (local testing)
    phone_number_id: ${WA_PHONE_ID}
    access_token: ${WA_TOKEN}
    verify_token: ${WA_VERIFY_TOKEN}
    app_secret: ${WA_APP_SECRET}     # verifies inbound webhook signatures
    webhook_path: /wa/webhook
  web:
    enabled: true
    allowed_origins:
      - https://app.myproduct.com
    magic_link:
      enabled: true
      ttl_minutes: 15

# ---- Safety ----
safety:
  default_mode: read_only            # read_only | full
  require_confirmation_for: [write, destructive]
  overrides:
    cancelOrder: { mode: write, require_confirmation: true }
    deleteAccount: { mode: destructive, blocked: true }
  rate_limit:
    per_customer_per_minute: 20
    per_customer_per_day: 500

# ---- Identity ----
identity:
  mapping_function: phone            # phone | email
  allow_anonymous: false

# ---- Session ----
session:
  idle_timeout_minutes: 60
  history_window: 20                 # turns kept in working memory
  summarize_after: 50                # turns before rolling summarization

# ---- Memory ----
memory:
  enabled: true
  recall_limit: 5

# ---- Reflection daemon (off by default) ----
daemon:
  enabled: false
  interval_minutes: 15
  max_per_cycle: 5
  reflect_min_messages: 2
  proactive_followup: false          # auto follow-up on unresolved sessions

# ---- Proactive outreach (off by default) ----
proactive:
  enabled: false
  require_opt_in: true
  max_per_customer_per_day: 5
  window_hours: 24                   # WhatsApp 24h window

# ---- Response cache (off by default) ----
cache:
  enabled: false
  similarity_threshold: 0.92
  ttl_minutes: 60

# ---- Storage ----
storage:
  database_path: /data/aelio.db
  backup:
    enabled: true
    interval_hours: 6
    retain_count: 14

# ---- Server / logging ----
logging:
  level: info                        # debug | info | warn | error
  format: json                       # json | pretty
server:
  host: 0.0.0.0
  port: 3000
```

Sample configs in the repo: `config.yaml` (mock), `config.docker.yaml`,
`config.openai.yaml`, `config.byo.yaml`, `config.daemon.yaml`, `config.proactive.yaml`,
`config.cache.yaml`, `config.followup.yaml`.

---

## 6. LLM providers & embeddings

**Chat providers:** `anthropic`, `openai`, `groq`, `ollama`, `mock`. Set a `fallback`
to automatically retry on a second provider if the primary fails.

```yaml
llm:
  provider: openai
  model: gpt-4o-mini
  api_key: ${OPENAI_API_KEY}
  fallback: { provider: groq, model: llama-3.3-70b-versatile, api_key: ${GROQ_API_KEY} }
```

- **Ollama** (local, no key): `provider: ollama`, `model: llama3.1`, optional `base_url`.
- **`mock`**: deterministic offline replies for demos/tests.

**Embeddings** power memory recall and the response cache:

- `hash` (default) — built-in, zero dependencies, matches near-identical wording.
- `openai` — `text-embedding-3-small` (1536-dim), real semantic recall.
- `ollama` — e.g. `nomic-embed-text`.

> Switching embedding providers on an existing deployment puts old vectors in a
> different space than new queries — re-embed memory after a switch. Fresh deploys are
> seamless.

---

## 7. Channels

### Web widget

Embed on your site:

```html
<script
  src="https://your-aelio-server.com/widget.js"
  data-customer-id="user_abc123"
  data-server-url="https://your-aelio-server.com"
></script>
```

- The widget connects to `wss://.../widget/ws`.
- `allowed_origins` gates which sites may connect (`"*"` to allow all — dev only).
- **Identity:** use **magic links** (`/auth/magic-link` → email link → `/auth/verify`)
  to authenticate a customer, or set `identity.allow_anonymous: true` for pre-auth chat.

### WhatsApp — built-in Meta adapter (`provider: meta`)

1. Create a Meta WhatsApp Business app; get a **phone number id** and a **permanent
   access token**.
2. Set `phone_number_id`, `access_token`, `verify_token` (a string you choose), and
   `app_secret` in `config.yaml`.
3. Point Meta's webhook at `https://your-aelio-server.com/wa/webhook` using your
   `verify_token`. Aelio answers the GET verification handshake and receives POSTs.

Set `mock_mode: true` to develop without calling Meta (outbound messages are captured
in-memory instead of sent).

### WhatsApp / anything — bring your own provider (`provider: sdk`)

Use any provider (Twilio, Gupshup, 360dialog…) or any channel (Telegram, SMS, custom).
You own the webhook + parsing + sending; Aelio owns the intelligence. Set
`channels.whatsapp.provider: sdk` and, in your backend:

```typescript
// Deliver outbound replies through YOUR provider (called automatically, not a tool).
aelio.onSend(async ({ channel, to, content }) => {
  await myProvider.messages.create({ to, body: content })
})

// In your own webhook route, hand Aelio the inbound message:
app.post('/my-hook', (req, res) => {
  const { from, text } = myProvider.parse(req.body)   // your provider's format
  aelio.ingest({ channel: 'whatsapp', from, text })
  res.sendStatus(200)
})
```

Aelio holds no provider credentials and the channel string is free-form, so the same
mechanism works for `telegram`, `sms`, etc.

---

## 8. The safety model

Every exposed function declares a `safety` level, enforced server-side **before** the
call ever reaches your SDK:

| Level | Behavior |
|---|---|
| `read` | Auto-allowed. The assistant calls it freely. |
| `write` | Aelio asks the customer to confirm ("Reply **yes** to confirm"), then runs it only on an explicit yes. |
| `destructive` | Blocked from chat in V1. |

- `safety.default_mode: read_only` blocks all writes unless explicitly allowed in
  `overrides`. Use `full` to allow writes (still gated by confirmation).
- `overrides` can change a function's mode, force confirmation, or `blocked: true`.
- **Rate limiting** (`safety.rate_limit`) caps messages per customer per minute/day.

Every invocation — success, error, blocked, pending, confirmed — is written to the
`function_calls` audit table.

---

## 9. Intelligence features

### Memory (always on when `memory.enabled`)

After each turn Aelio extracts durable facts about the customer and stores them in
SQLite, vector-indexed via `sqlite-vec`. On the next turn it recalls the most relevant
facts and injects them into the prompt — so the assistant remembers preferences and
context across sessions and channels. Recall quality scales with the embedding model
(§6).

### Reflection daemon (`daemon.enabled`)

A scheduled, budgeted, idempotent background loop that reviews finished sessions with
the LLM: *was the customer's intent resolved? what was learned?* Verdicts go to the
`reflections` table; durable insights flow back into memory. It reflects each session
once, at most `max_per_cycle` per wake-up.

### Proactive outreach (`proactive.enabled`)

System-initiated messages with guardrails enforced server-side: feature-enabled →
known recipient → **opt-in** → **dedup** → **daily cap** → **WhatsApp 24h window**
(free-form inside it, a `templateName` required outside). Trigger from your backend:

```bash
# opt a customer in
curl -X POST https://your-aelio-server.com/proactive/opt-in \
  -H "authorization: Bearer $AELIO_SDK_SECRET" -H 'content-type: application/json' \
  -d '{"customerExternalId":"user_abc123","optIn":true}'

# send a proactive message
curl -X POST https://your-aelio-server.com/proactive \
  -H "authorization: Bearer $AELIO_SDK_SECRET" -H 'content-type: application/json' \
  -d '{"customerExternalId":"user_abc123","channel":"whatsapp","content":"Your order shipped!","dedupKey":"ship-A123"}'
```

With `daemon.proactive_followup: true`, an `unresolved` reflection auto-enqueues a
follow-up through this same guard-railed path (one per session, ever).

### Response cache (`cache.enabled`)

Per-customer semantic cache that serves a near-identical recent reply without an LLM
call. Staleness-safe by design: **only no-tool replies are cached** (live data is always
fetched fresh), high similarity threshold, TTL-bounded.

---

## 10. Endpoints

| Method | Path | Purpose |
|---|---|---|
| GET | `/health` | Liveness |
| GET | `/ready` | Readiness (db, migrations, sdk, memory, channels, llm) |
| GET | `/diagnostics` | Process/config diagnostics (`AELIO_TEST_MODE=1` or `AELIO_DIAGNOSTICS=1`) |
| GET | `/widget.js` | The embeddable widget bundle |
| GET | `/demo.html` | Local demo page that embeds the widget |
| WS  | `/widget/ws` | Web chat transport |
| GET/POST | `/wa/webhook` | WhatsApp verification (GET) + inbound (POST) |
| WS  | `/sdk` | SDK connection (`Authorization: Bearer <secret>`; `?secret=` deprecated) |
| POST | `/auth/magic-link` | Issue a magic link (Bearer `secret` + rate limited) |
| GET | `/auth/verify` | Verify a magic-link token (returns `sessionToken` for widget init) |
| POST | `/proactive` | Send a proactive message (Bearer `secret`; requires `proactive.enabled`) |
| POST | `/proactive/opt-in` | Set a customer's proactive opt-in (Bearer `secret`) |
| GET | `/telemetry` | Telemetry HTML UI (auth required in production) |
| GET | `/api/v1/telemetry/events` | Conversation telemetry events (Bearer `secret`; Sunjet) |
| GET | `/api/v1/telemetry/turn-calls` | Per-turn LLM/embed call log (Bearer `secret`) |

Test-only endpoints (`/__test__/...`) are enabled when `AELIO_TEST_MODE=1`.

---

## 11. Deployment

### One container (Docker)

```bash
docker run -d \
  -v "$(pwd)/data:/data" \
  -p 3000:3000 \
  -e ANTHROPIC_API_KEY=sk-ant-... \
  -e WA_TOKEN=... \
  -e AELIO_SDK_SECRET=... \
  aelio/server:latest
```

Build locally with `pnpm docker:build`. The image is **distroless** (~190 MB), runs
`node dist/main.js`, embeds SQLite + sqlite-vec, persists to the `/data` volume, and
ships a `HEALTHCHECK` on `/health`. Inside the container the config is
`config.docker.yaml`.

### docker-compose

```bash
AELIO_SDK_SECRET=... docker compose -f server/docker-compose.yml up --build
```

(Volume-mounted `/data`, env-driven secret + `ANTHROPIC_API_KEY`.)

### Environment variables

| Var | Purpose |
|---|---|
| `AELIO_CONFIG` | Path to the config file (default `./config.yaml`) |
| `AELIO_SDK_SECRET` | Shared SDK secret (referenced as `${AELIO_SDK_SECRET}` in config) |
| `AELIO_MIGRATIONS_PATH` / `AELIO_PUBLIC_PATH` | Override migration/widget locations (set in the image) |
| `AELIO_TEST_MODE` | `1` enables `/__test__` endpoints |
| LLM/WhatsApp keys | Whatever your config references via `${...}` |

### One-click templates

`server/railway.toml`, `server/render.yaml`, `server/fly.toml`.

---

## 12. Publishing the SDKs

- **Node** (`@aelio/sdk`): `pnpm --filter @aelio/sdk build` then `pnpm pack` /
  `npm publish` (the published tarball has no workspace deps — only `ws` + `zod`).
- **Python** (`aelio-sdk`): build from `sdk/python/pyproject.toml`
  (`python -m build`) and publish to PyPI.

---

## 13. Testing & verification

```bash
pnpm typecheck                 # all packages
pnpm build                     # all packages + widget

# Full end-to-end phase suite (boots its own server + example SDK)
AELIO_TEST_MODE=1 pnpm test:all

# Individual phases
AELIO_TEST_MODE=1 pnpm test:phase2   # web widget → LLM → SDK
AELIO_TEST_MODE=1 pnpm test:phase3   # WhatsApp webhook → queue → send
AELIO_TEST_MODE=1 pnpm test:phase4:confirmation
AELIO_TEST_MODE=1 pnpm test:phase5   # memory extract + recall

pnpm diagnostic                # build + health + graceful shutdown checks
pnpm docker:verify             # build image, run container, run phases
```

---

## 14. Troubleshooting

| Symptom | Fix |
|---|---|
| Server exits on boot with a config error | Read the message — it names the exact invalid field. Check `${VARS}` are exported. |
| `listen EADDRINUSE: 0.0.0.0:3000` | Another server holds the port. Stop it (or change `server.port`). |
| SDK never appears / tools not called | Ensure `AELIO_SERVER_URL` and `AELIO_SDK_SECRET` match the server; check `/ready` shows `sdk.connected: true`. |
| Widget shows **"Connection failed: Origin not allowed"** | The page's origin (scheme+host+port) isn't in `channels.web.allowed_origins`. Add it (e.g. `http://127.0.0.1:4173`), or use `"*"` for local dev. Origins are matched exactly — `localhost` and `127.0.0.1` are distinct. |
| Widget stuck on **"Connecting…"** then **"Connection failed"** | The server didn't complete the handshake within ~8s. Check `data-server-url` points at the server, the server is up (`/health`), and `channels.web.enabled: true`. |
| LLM `429 insufficient_quota` | Provider account has no credits/billing — add credits or switch provider. |
| WhatsApp webhook verification fails | `verify_token` in config must match the one entered in Meta. |
| Proactive `blocked: not opted in` | Call `/proactive/opt-in` for that customer first. |
| Proactive `blocked: outside 24h window` | Provide a `templateName` (Meta requires templates outside the 24h window). |
| Memory recall feels weak | Switch `embeddings.provider` to `openai`/`ollama` for semantic matching. |
| Vector index disabled (`/ready` shows `vectorIndex: false`) | `sqlite-vec` failed to load; Aelio falls back to brute-force cosine. Rebuild native deps. |

---

For the product vision and design specs, see `Blueprint/v1-oss-spec.md` and
`Blueprint/v1-daemon-spec.md`.
