# V1 OSS Specification — Aelio: Conversational Runtime for SaaS

**Product name:** Aelio
**Status:** Build-ready specification
**Stack lock:** TypeScript + SQLite (with vector embeddings), single-binary deployment
**License intent:** Apache-2.0 (open core; managed cloud as commercial offering)

---

## 1. Product overview

### What it is

**Aelio** is an open-source conversational runtime that lets any SaaS product offer its customers a chat and voice experience on WhatsApp, Web, and (later) voice — by installing the **Aelio SDK** in their existing backend.

Aelio is two agents in one system:

1. **Conversational agent** — talks to the SaaS's end customer in real time, calls SDK functions, handles safety and confirmations.
2. **Analytical agent** — continuously monitors every conversation, extracts durable understanding of each user, and recalls that intelligence in future interactions.

SQLite with vector embeddings (`sqlite-vec`) is not a storage convenience — it is the **core intelligence substrate**. All conversation history, extracted facts, behavioral signals, and SDK audit data live in one embedded database. The analytical agent reads from and writes to this store on every turn and in background jobs, building a persistent model of each customer over time.

### What makes it different

The OpenAPI-to-MCP space is already crowded. Every existing tool solves the **developer-to-AI-agent** problem (Claude Desktop, Cursor, IDE agents calling APIs).

**Nobody solves the SaaS-to-end-user problem:**

- End user is a *customer*, not a developer
- Interaction is on *WhatsApp, Web, Voice* — not an AI IDE
- Needs *identity, session memory, safety rails, conversation continuity*
- Needs *persistent user understanding* — not just turn-by-turn replies
- Needs *fallback handling and graceful degradation*

This product owns that layer. The MCP-style tool wiring is just the integration mechanism; the actual product is the **runtime that hosts production-grade conversational interfaces for SaaS end-users, with an always-on analytical layer that understands each customer over time.**

### Core thesis

A SaaS dev should be able to give their customers a conversational interface on WhatsApp and Web in under 30 minutes, with one Aelio SDK install, one YAML file, and one container — and Aelio handles conversation, memory, user analysis, and channel delivery so the dev does not have to.

### The analytical agent (first-class capability)

Most chatbot frameworks treat memory as an afterthought — a optional RAG layer bolted on top. Aelio treats **user intelligence** as a core product pillar.

The analytical agent runs alongside the conversational agent on every interaction:

| When | What the analytical agent does |
|---|---|
| **During a turn** | Recalls vector-indexed facts about this customer; injects relevant context into the LLM prompt |
| **After a turn** | Extracts durable facts (preferences, recurring intents, order context, language patterns) |
| **Across sessions** | Merges understanding when the same customer returns on a different channel |
| **On SDK calls** | Records what actions the user triggered, outcomes, and patterns (e.g. "frequently checks shipping status") |
| **On idle/close** | Summarizes long sessions; compresses history while preserving key facts in vector memory |

All of this is stored in SQLite — no separate vector DB, no external analytics pipeline required for OSS self-hosters.

```
┌─────────────────────────────────────────────────────────────┐
│                     Per-customer intelligence              │
│                                                             │
│  conversations ──► extract facts ──► memory (vector-indexed)│
│       ▲                                      │              │
│       └──────── recall on next turn ◄────────┘              │
│                                                             │
│  Stored in: SQLite + sqlite-vec (embedded, single file)    │
└─────────────────────────────────────────────────────────────┘
```

The SaaS dev does not build user profiling, conversation monitoring, or cross-session recall. Aelio does — and serves that intelligence back to the end customer through better, more contextual conversations.

---

## 2. Integration model: the SDK approach

### Why SDK over OpenAPI ingestion or webhooks

- **No credential sharing.** Functions run inside the dev's own backend with their existing auth, DB connections, and session context. The platform never holds their API keys.
- **Works without a public API.** Internal tools, direct DB access, private services — all exposed-able.
- **The SDK dials out** to the platform on startup over a persistent connection. No webhook URLs to configure on the dev's side. No firewall changes.
- **Function definitions are colocated with implementations.** No drift between docs and behavior.

### What the dev writes — total surface area

```typescript
import { aelio } from '@aelio/sdk'

aelio.expose('getOrderStatus', async ({ orderId }, ctx) => {
  // ctx.customerId is the SaaS's own user ID — they scope normally
  return await db.orders.findOne({ id: orderId, userId: ctx.customerId })
}, {
  description: 'Get the status of a customer order',
  params: { orderId: 'string' },
  safety: 'read'
})

aelio.expose('cancelOrder', async ({ orderId }, ctx) => {
  return await db.orders.cancel(orderId, { userId: ctx.customerId })
}, {
  description: 'Cancel a pending order',
  params: { orderId: 'string' },
  safety: 'write'
})

aelio.listen({ secret: process.env.AELIO_SECRET })
```

That is the entire integration. The dev's auth, their DB, their business logic — untouched.

### How auth works

The dev's existing auth is fully preserved because the SDK runs inside their backend. The platform passes a `ctx.customerId` (resolved from the channel — phone number → customer mapping) and the dev scopes their queries to that ID as they would for any logged-in request.

No tokens flow from the platform to the dev's backend. No tokens flow from the dev's backend to the platform. The platform's only credential is the SDK secret used to authenticate the SDK connection.

---

## 3. Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    SaaS Customer (end user)                 │
│              WhatsApp / Web Widget / (later) Voice          │
└────────────────────────────┬────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────┐
│              Aelio Server (self-hosted or cloud)            │
│                                                             │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐    │
│  │ Channels │→ │ Identity │→ │ Runtime  │→ │   LLM    │    │
│  │ WA / Web │  │ Sessions │  │ + Safety │  │ Provider │    │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘    │
│                                    │                        │
│  ┌─────────────────────────────────┴───────────────────┐    │
│  │            SQLite (state, memory, audit)            │    │
│  └─────────────────────────────────────────────────────┘    │
│                                    │                        │
│                                    ▼                        │
│                       Persistent WS to SDK                  │
└────────────────────────────────────┬────────────────────────┘
                                     │
                                     ▼
┌─────────────────────────────────────────────────────────────┐
│                 SaaS Backend (the dev's own server)         │
│           SDK installed → exposed functions → their DB      │
└─────────────────────────────────────────────────────────────┘
```

### Why the SDK dials out

Reverses the firewall problem. The dev doesn't expose ports, doesn't configure webhooks, doesn't open inbound traffic. The SDK initiates the connection outbound to the platform, same as a Stripe webhook listener or Inngest dev server. Works on any infra.

---

## 4. Tech stack — final

| Concern | Choice | Why |
|---|---|---|
| Language | TypeScript | Ecosystem fit for AI/messaging; largest contributor pool |
| Runtime | Node.js 22 LTS | Stable, fast WS handling |
| HTTP framework | Fastify | High throughput, low overhead |
| Database | SQLite (WAL mode) | Zero external deps; embedded in deployment |
| Vector search | `sqlite-vec` extension | Embedded; no separate vector DB |
| ORM | Drizzle | Works on SQLite + Postgres → cloud migration path is `change driver` |
| Job queue | SQLite-backed (custom, ~200 lines) | No Redis dependency for OSS |
| WebSocket | `ws` library | Battle-tested |
| Validation | Zod | Type-safe schemas, shared TS types |
| Logging | Pino | Structured, fast |
| Build | tsup → single bundled JS | Easy distribution |
| Container | Distroless Node image | Small, secure |
| Widget | Preact | ~30kb bundle for embed |
| Monorepo | pnpm workspaces + Turbo | Standard, fast |

### SDK languages shipped at V1

- **TypeScript / Node.js** — primary
- **Python** — same wire protocol, ~500 LOC, doubles addressable market (FastAPI/Django shops)

Future SDKs (Go, Ruby, PHP) are mechanical to add since the protocol is JSON-over-WebSocket.

### Why not Postgres for OSS

Self-hosters should run *one container*. Postgres adds a second container, volume management, connection pooling concerns, and password handling — all friction. SQLite in WAL mode handles thousands of concurrent reads and serialized writes; way beyond what a single SaaS tenant needs for conversational state.

Cloud uses Postgres because multi-tenancy, horizontal scaling, and managed backups need a network-accessible DB. Drizzle schemas are identical — driver swap only.

### Why not Rust

The hot paths are all I/O-bound (WebSockets, LLM API calls, DB writes). Rust's CPU performance wins don't apply. TypeScript's contributor base, AI SDK ecosystem, and iteration speed matter more for an OSS V1. Rust becomes the right call later for managed-cloud hot paths once you have scale pressure.

---

## 5. Repository structure

```
aelio/
├── apps/
│   ├── server/                 # Self-hostable service
│   └── web-widget/             # Embeddable Preact widget
├── packages/
│   ├── sdk-node/               # TypeScript SDK
│   ├── sdk-python/             # Python SDK
│   ├── protocol/               # Shared wire protocol types
│   ├── core/                   # Runtime, sessions, safety, memory
│   ├── channels/               # WhatsApp + Web adapters
│   ├── llm/                    # Provider abstraction
│   └── db/                     # Drizzle schema + migrations
├── examples/
│   ├── nodejs-express/
│   ├── nextjs/
│   ├── python-fastapi/
│   └── python-django/
├── docker/
│   ├── Dockerfile
│   └── docker-compose.yml
├── docs/
└── README.md
```

---

## 6. Package-by-package detail

### `packages/protocol`

Defines the wire contract. Pure types + Zod schemas. Zero runtime dependencies. Everything else depends on this.

```typescript
// SDK → Server: registration on connect
type RegisterMessage = {
  type: 'register'
  sdkVersion: string
  language: 'node' | 'python' | 'go' | ...
  functions: FunctionDefinition[]
}

type FunctionDefinition = {
  name: string
  description: string
  params: JSONSchema7
  safety: 'read' | 'write' | 'destructive'
}

// Server → SDK: invocation
type InvokeMessage = {
  type: 'invoke'
  id: string                    // correlation ID
  function: string
  args: Record<string, unknown>
  context: InvocationContext
}

type InvocationContext = {
  customerId: string            // SaaS's user ID
  sessionId: string
  channel: 'whatsapp' | 'web'
  channelAddress: string        // phone, email, etc.
  locale?: string
  metadata?: Record<string, unknown>
}

// SDK → Server: result
type ResultMessage = {
  type: 'result'
  id: string
  ok: boolean
  data?: unknown
  error?: { code: string; message: string; retryable?: boolean }
  durationMs: number
}

// Server → SDK: heartbeat
type PingMessage = { type: 'ping'; ts: number }
type PongMessage = { type: 'pong'; ts: number }
```

### `packages/sdk-node`

Public API — only three methods:

```typescript
class Aelio {
  expose<T, R>(
    name: string,
    handler: (args: T, ctx: InvocationContext) => Promise<R>,
    schema: FunctionSchema
  ): void

  listen(opts: { secret: string; url?: string }): Promise<void>

  disconnect(): Promise<void>
}

export const aelio = new Aelio()
```

Internals:

- Persistent WebSocket to `wss://[platform]/sdk` (or self-hosted URL)
- Exponential backoff reconnect (1s → 2s → 4s → ... → 30s cap)
- Heartbeat every 30s; reconnect if no pong in 60s
- Function registration on every connect
- Concurrent invocation handling (correlation by ID)
- Graceful shutdown drains in-flight invocations

### `packages/sdk-python`

Same surface, idiomatic Python:

```python
from aelio import aelio

@aelio.expose(
    description="Get the status of a customer order",
    safety="read"
)
async def get_order_status(order_id: str, ctx) -> dict:
    return await db.orders.find_one({"id": order_id, "user_id": ctx.customer_id})

await aelio.listen(secret=os.environ["AELIO_SECRET"])
```

Uses `websockets` library. Same protocol, same behavior.

### `packages/db`

Drizzle schemas. SQLite-first, Postgres-compatible.

```typescript
// Tables (overview)
customers              // end users of the SaaS
sessions               // conversations, scoped to customer
messages               // turn history with role + tool calls
memory                 // long-term facts, vector-indexed via sqlite-vec
function_calls         // audit log of every SDK invocation
sdk_connections        // active SDK connections + their function registry
channel_addresses      // phone/email → customer mapping
channel_config         // WhatsApp credentials, web widget settings
job_queue              // SQLite-backed work queue (outbound delivery, retries)
```

Detailed schema in §8.

### `packages/core`

The brain. Stateless logic — given a message, orchestrates one full turn.

```
core/
├── runtime/
│   ├── turn.ts              # Main entry: message in → reply out
│   ├── tool-loop.ts         # LLM tool-calling loop with iteration limit
│   └── fallback.ts          # Graceful degradation when LLM fails / loops
├── identity/
│   ├── resolve.ts           # phone/email → customer
│   └── magic-link.ts        # Web channel auth
├── session/
│   ├── lifecycle.ts         # new / active / dormant / closed
│   ├── stitch.ts            # Cross-channel session merging
│   └── memory.ts            # Recall + compression
├── analyst/
│   ├── extract.ts           # Post-turn fact extraction → memory table
│   ├── recall.ts            # Vector search for relevant customer context
│   ├── profile.ts           # Rolling user understanding (preferences, patterns)
│   └── summarize.ts         # Session compression when history exceeds window
├── safety/
│   ├── policy.ts            # Read / Write / Destructive enforcement
│   └── confirmations.ts     # Write actions require user confirmation
└── sdk-bridge/
    ├── connection.ts        # SDK WebSocket lifecycle
    ├── invoke.ts            # Send invoke, await result, timeout handling
    └── registry.ts          # Track which functions are available
```

### `packages/channels`

Channel adapters implementing a shared interface:

```typescript
interface ChannelAdapter {
  name: string
  receive(rawPayload: unknown): InboundMessage | null
  send(customerId: string, content: OutboundContent): Promise<void>
  verify(payload: unknown, signature: string): boolean
}
```

**WhatsApp adapter:**
- Meta Cloud API
- Webhook signature verification
- 24-hour conversation window handling
- Template messages for re-engagement outside the window
- Media: image, audio, document handling
- Mark-as-read, typing indicators

**Web adapter:**
- WebSocket server at `/widget/ws`
- Serves the widget bundle at `/widget.js`
- Magic link generation + verification for identity
- Anonymous mode for pre-auth conversations

### `packages/llm`

Provider abstraction:

```typescript
interface LLMProvider {
  complete(opts: {
    messages: ChatMessage[]
    tools: ToolDefinition[]
    model: string
    maxTokens?: number
    temperature?: number
  }): AsyncIterable<LLMEvent>
}

type LLMEvent =
  | { type: 'text'; delta: string }
  | { type: 'tool_call'; id: string; name: string; args: unknown }
  | { type: 'finish'; reason: 'stop' | 'tool_use' | 'length' }
```

V1 providers:
- Anthropic (Claude)
- OpenAI (GPT-4o, GPT-4o-mini)
- Groq (Llama, Mixtral for fast inference)
- Ollama (local self-hosted models)

### `apps/server`

Wires everything together. Fastify app.

```
apps/server/src/
├── main.ts                  # Entry point
├── config.ts                # YAML loader + Zod validation
├── routes/
│   ├── health.ts
│   ├── whatsapp.ts          # POST /wa/webhook + GET (verification)
│   ├── widget.ts            # GET /widget.js, WS /widget/ws
│   └── sdk.ts               # WS /sdk
├── workers/
│   ├── inbound.ts           # Process incoming messages from queue
│   └── outbound.ts          # Deliver outbound messages with retry
└── shutdown.ts              # Graceful drain on SIGTERM
```

### `apps/web-widget`

Tiny Preact app. Built to single `widget.js` file. Embed:

```html
<script
  src="https://aelio.yoursaas.com/widget.js"
  data-customer-id="user_abc123"
  data-token="signed_jwt_optional"
></script>
```

Features: floating button, expandable chat panel, message history, typing indicator, attachment support (V2).

---

## 7. The YAML config — the single source of truth

```yaml
# config.yaml
name: my-saas-conversational
secret: ${AELIO_SDK_SECRET}        # SDK uses this to authenticate

# LLM configuration
llm:
  provider: anthropic                # or openai, groq, ollama
  model: claude-sonnet-4-6
  api_key: ${ANTHROPIC_API_KEY}
  max_tokens: 4096
  fallback:                          # optional secondary provider
    provider: openai
    model: gpt-4o-mini
    api_key: ${OPENAI_API_KEY}

# Channels
channels:
  whatsapp:
    enabled: true
    phone_number_id: ${WA_PHONE_ID}
    access_token: ${WA_TOKEN}
    verify_token: ${WA_VERIFY_TOKEN}
    webhook_path: /wa/webhook

  web:
    enabled: true
    allowed_origins:
      - https://myproduct.com
      - https://app.myproduct.com
    magic_link:
      sender_email: noreply@myproduct.com
      sender_name: My Product

# Safety defaults
safety:
  default_mode: read_only            # writes need explicit allow
  require_confirmation_for: [write, destructive]
  rate_limit:
    per_customer_per_minute: 20
    per_customer_per_day: 500

# Identity
identity:
  mapping_function: phone            # how to resolve channel address → customer
  allow_anonymous: false             # require known customer

# Session
session:
  idle_timeout_minutes: 60
  history_window: 20                 # turns kept in working memory
  summarize_after: 50                # turns before summarization kicks in

# Storage
storage:
  database_path: /data/aelio.db     # SQLite file location
  backup:
    enabled: true
    interval_hours: 6
    retain_count: 14

# Observability
logging:
  level: info                        # debug | info | warn | error
  format: json
```

Validation: Zod schema. Server refuses to start on invalid config with a clear error message pointing to the exact field.

---

## 8. Database schema (SQLite)

### `customers`

```sql
CREATE TABLE customers (
  id TEXT PRIMARY KEY,                    -- internal UUID
  external_id TEXT,                       -- SaaS's own user ID
  display_name TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  metadata JSON
);
CREATE UNIQUE INDEX idx_customers_external ON customers(external_id);
```

### `channel_addresses`

Maps a channel + address (phone, email) to a customer.

```sql
CREATE TABLE channel_addresses (
  id TEXT PRIMARY KEY,
  customer_id TEXT NOT NULL REFERENCES customers(id),
  channel TEXT NOT NULL,                  -- 'whatsapp' | 'web'
  address TEXT NOT NULL,                  -- phone number, email
  verified_at INTEGER,
  created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_chan_addr ON channel_addresses(channel, address);
```

### `sessions`

```sql
CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  customer_id TEXT NOT NULL REFERENCES customers(id),
  channel TEXT NOT NULL,
  status TEXT NOT NULL,                   -- 'active' | 'dormant' | 'closed'
  started_at INTEGER NOT NULL,
  last_activity_at INTEGER NOT NULL,
  closed_at INTEGER,
  summary TEXT,                           -- LLM-generated rolling summary
  metadata JSON
);
CREATE INDEX idx_sessions_customer ON sessions(customer_id, last_activity_at DESC);
CREATE INDEX idx_sessions_active ON sessions(status, last_activity_at) WHERE status = 'active';
```

### `messages`

```sql
CREATE TABLE messages (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  customer_id TEXT NOT NULL REFERENCES customers(id),
  role TEXT NOT NULL,                     -- 'user' | 'assistant' | 'tool' | 'system'
  content TEXT,                           -- text body
  tool_call JSON,                         -- {name, args} for tool calls
  tool_result JSON,                       -- result payload
  channel TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  tokens_in INTEGER,
  tokens_out INTEGER
);
CREATE INDEX idx_messages_session ON messages(session_id, created_at);
```

### `memory`

Long-term facts about a customer, vector-indexed via `sqlite-vec`.

```sql
CREATE TABLE memory (
  id TEXT PRIMARY KEY,
  customer_id TEXT NOT NULL REFERENCES customers(id),
  content TEXT NOT NULL,                  -- "User prefers metric units"
  source_session_id TEXT REFERENCES sessions(id),
  created_at INTEGER NOT NULL,
  expires_at INTEGER,                     -- optional TTL
  confidence REAL DEFAULT 1.0
);

-- Vector index via sqlite-vec
CREATE VIRTUAL TABLE memory_vec USING vec0(
  embedding FLOAT[1536]                   -- adjust per embedding model
);
```

### `function_calls`

Audit trail for every SDK invocation.

```sql
CREATE TABLE function_calls (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  customer_id TEXT NOT NULL REFERENCES customers(id),
  function_name TEXT NOT NULL,
  args JSON,
  result JSON,
  status TEXT NOT NULL,                   -- 'success' | 'error' | 'timeout'
  safety_level TEXT NOT NULL,             -- 'read' | 'write' | 'destructive'
  required_confirmation BOOLEAN DEFAULT FALSE,
  confirmed BOOLEAN,
  duration_ms INTEGER,
  error_message TEXT,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_fc_session ON function_calls(session_id, created_at);
CREATE INDEX idx_fc_customer ON function_calls(customer_id, created_at DESC);
```

### `sdk_connections`

Currently-connected SDKs and their registered functions.

```sql
CREATE TABLE sdk_connections (
  id TEXT PRIMARY KEY,
  connection_token TEXT NOT NULL,         -- hashed
  sdk_version TEXT,
  language TEXT,
  connected_at INTEGER NOT NULL,
  last_heartbeat_at INTEGER NOT NULL,
  functions JSON NOT NULL                 -- array of FunctionDefinition
);
```

### `job_queue`

Lightweight SQLite-backed queue. Two queues: `inbound` (processing) and `outbound` (delivery).

```sql
CREATE TABLE job_queue (
  id TEXT PRIMARY KEY,
  queue TEXT NOT NULL,                    -- 'inbound' | 'outbound'
  payload JSON NOT NULL,
  status TEXT NOT NULL,                   -- 'pending' | 'processing' | 'done' | 'failed'
  attempts INTEGER DEFAULT 0,
  max_attempts INTEGER DEFAULT 5,
  next_run_at INTEGER NOT NULL,
  locked_by TEXT,                         -- worker ID, for crash recovery
  locked_at INTEGER,
  created_at INTEGER NOT NULL,
  completed_at INTEGER,
  error_message TEXT
);
CREATE INDEX idx_jobs_pending ON job_queue(queue, status, next_run_at) WHERE status = 'pending';
```

Workers poll with `UPDATE ... RETURNING` for atomic claim. Failed jobs retry with exponential backoff.

---

## 9. End-to-end message flow

```
1. Customer sends "what's my order status?" via WhatsApp
   │
2. Meta posts to POST /wa/webhook
   │
3. channels/whatsapp verifies signature, parses payload
   │
4. Job pushed to `inbound` queue
   │
5. Inbound worker picks up:
   ├─ identity.resolve(phone="+91...") → customers.id
   ├─ session.findOrCreate(customer_id, channel="whatsapp") → session.id
   ├─ messages.insert(role="user", content="what's my order status?")
   ├─ session.loadHistory(session_id, last 20 turns)
   ├─ memory.recall(customer_id, query=current_msg) → relevant facts
   ├─ sdk.registry.getFunctions() → available tools
   │
6. core/runtime/turn:
   ├─ Build prompt with: system + memory + history + new message
   ├─ Call LLM with tools
   │
7. LLM returns: tool_call("getOrderStatus", {orderId: "last"})
   │
8. core/safety check:
   ├─ Function safety = "read" → no confirmation needed ✓
   │
9. core/sdk-bridge/invoke:
   ├─ Find active SDK connection
   ├─ Send InvokeMessage over WS with context {customerId, sessionId, channel}
   ├─ Await ResultMessage (timeout: 30s)
   │
10. SDK in dev's backend:
    ├─ Looks up registered handler "getOrderStatus"
    ├─ Calls handler({orderId: "last"}, ctx={customerId, ...})
    ├─ Handler runs THEIR code with THEIR auth on THEIR DB
    ├─ Returns {status: "shipped", tracking: "1Z999..."}
    ├─ Sends ResultMessage back over WS
    │
11. function_calls.insert(success, duration, args, result)
    │
12. LLM called again with tool result in context
    │
13. LLM produces final text: "Your last order shipped today! Tracking: 1Z999..."
    │
14. messages.insert(role="assistant", content=final_text)
    │
15. Job pushed to `outbound` queue for WhatsApp delivery
    │
16. Outbound worker:
    ├─ channels/whatsapp.send(customer_id, content)
    ├─ Marks job done
    │
17. analyst.extract — async background job: extract durable facts + update user profile
    ("Customer's last order was order_xyz", "Customer asks about shipping in English",
     "User frequently checks order status — proactive shipping updates may help")
```

---

## 10. The safety model

Every exposed function declares a `safety` level:

| Level | Behavior |
|---|---|
| `read` | Auto-allowed. LLM can call freely. No confirmation. |
| `write` | Requires user confirmation in conversation. Platform asks: "I'm about to update X — confirm?" |
| `destructive` | **V1: blocked entirely.** LLM never calls. Returns "this action isn't available via chat." |

This is enforced server-side, before the invocation reaches the SDK. The dev can override defaults in config:

```yaml
safety:
  default_mode: read_only
  overrides:
    cancelOrder: { mode: write, require_confirmation: true }
    deleteAccount: { mode: destructive, blocked: true }
```

This is the layer that makes the runtime *safe to put in front of real customers* — and it's exactly what the OpenAPI→MCP generators don't have.

---

## 11. Deployment

### Self-host: one container

```bash
# Pull and run
docker run -d \
  -v ./data:/data \
  -v ./config.yaml:/app/config.yaml \
  -p 3000:3000 \
  -e ANTHROPIC_API_KEY=sk-ant-... \
  -e WA_TOKEN=... \
  -e AELIO_SDK_SECRET=... \
  aelio/server:latest
```

### Self-host: single binary (V1.1)

```bash
curl -fsSL https://aelio.dev/install.sh | sh
aelio start --config config.yaml
```

Built via `pkg` or Bun's single-binary compilation. SQLite embedded. No Node install required on the host.

### One-click deploys

Templates for Railway, Render, Fly.io. Pre-configured Docker images with sensible defaults.

### Cloud (managed)

Same code; runs multi-tenant, swaps SQLite → Postgres via env var. Differentiation in §13.

---

## 12. SDK examples

### Express (Node.js)

```typescript
import express from 'express'
import { aelio } from '@aelio/sdk'

const app = express()
// ...normal Express setup, normal auth middleware, normal routes...

// Add aelio alongside
aelio.expose('getOrderStatus', async ({ orderId }, ctx) => {
  return await db.orders.findOne({
    where: { id: orderId, userId: ctx.customerId }
  })
}, {
  description: 'Get the current status of a customer order',
  params: { orderId: 'string' },
  safety: 'read'
})

await aelio.listen({ secret: process.env.AELIO_SECRET })
app.listen(8080)
```

### Next.js (App Router)

Place a `aelio.ts` file, import once in a server-only context (e.g., `instrumentation.ts`):

```typescript
// instrumentation.ts
export async function register() {
  if (process.env.NEXT_RUNTIME === 'nodejs') {
    await import('./aelio-setup')
  }
}

// aelio-setup.ts
import { aelio } from '@aelio/sdk'
import { db } from './lib/db'

aelio.expose('getInvoices', async (_, ctx) => {
  return await db.invoice.findMany({ where: { userId: ctx.customerId } })
}, { description: 'List user invoices', params: {}, safety: 'read' })

await aelio.listen({ secret: process.env.AELIO_SECRET })
```

### FastAPI (Python)

```python
from fastapi import FastAPI
from aelio import aelio

app = FastAPI()

@aelio.expose(description="Get user subscription", safety="read")
async def get_subscription(ctx) -> dict:
    return await db.subscriptions.find_one({"user_id": ctx.customer_id})

@aelio.expose(
    description="Upgrade plan",
    params={"plan": "string"},
    safety="write"
)
async def upgrade_plan(plan: str, ctx) -> dict:
    return await billing.upgrade(ctx.customer_id, plan)

import asyncio
asyncio.create_task(aelio.listen(secret=os.environ["AELIO_SECRET"]))
```

### Django (Python)

Same SDK. Initialize in `apps.py`'s `ready()` method, run in a background thread or via management command alongside the server.

---

## 13. What's in V1 vs. cloud differentiation

### V1 OSS includes

- WhatsApp + Web channels
- TypeScript + Python SDKs
- Multi-provider LLM support (Anthropic, OpenAI, Groq, Ollama)
- Identity resolution + magic-link auth
- Session memory + conversation history
- Long-term customer memory with vector recall
- Read/Write/Destructive safety model
- Confirmation flow for write actions
- SQLite-embedded storage
- Audit log of all function calls
- Single-container Docker deployment
- YAML config-driven

### V1 OSS explicitly excludes

- Voice channel (V2)
- Visual playbook editor (cloud-only)
- Multi-tenant operation (cloud-only)
- Admin dashboard (cloud-only)
- Analytics / reporting UI
- Eval harness
- Slack / Teams channels (V3)
- Compliance certifications (SOC2, HIPAA — cloud-only)

### Managed cloud value prop

| Self-hosted OSS | Aelio Cloud |
|---|---|
| Run your own container | Hosted, auto-scaled, multi-region |
| Configure WhatsApp yourself | Pre-approved templates, BYO number setup support |
| SQLite single-tenant | Multi-tenant Postgres, isolated workspaces |
| YAML config | Visual playbook editor + version control |
| Manual log inspection | Built-in analytics + conversation review |
| Self-managed backups | Automated, point-in-time recovery |
| One LLM per deployment | Per-customer LLM routing, model fallback |
| You handle compliance | SOC2 / HIPAA / GDPR audit trail |
| No team collaboration | Roles, permissions, audit logs |
| Manual eval | Built-in eval harness with regression detection |

---

## 14. Build phases

### Phase 1 — Skeleton (Week 1-2)

- Monorepo setup (pnpm, Turbo)
- `packages/protocol` types + Zod schemas
- `packages/db` schema + migrations + Drizzle setup
- `packages/sdk-node` with WebSocket connect/disconnect
- `apps/server` boot, config loader, health endpoint
- Docker build pipeline

**Demo target:** SDK connects to server, registers a function, server logs it.

### Phase 2 — Core loop (Week 3-4)

- `packages/llm` Anthropic adapter with streaming + tool calls
- `packages/core/runtime/turn.ts` — full LLM tool loop
- `packages/core/sdk-bridge` — invoke + result correlation
- `packages/core/session` — session create/load/append
- `packages/channels/web` — WS endpoint + widget bundle

**Demo target:** Type message in web widget → LLM responds → calls SDK function → response returns. End to end.

### Phase 3 — WhatsApp (Week 5)

- `packages/channels/whatsapp` — webhook + send
- Inbound/outbound job queue workers
- 24h window handling

**Demo target:** Same flow but on WhatsApp.

### Phase 4 — Safety + identity (Week 6)

- Read/Write/Destructive enforcement
- Confirmation flow for writes
- Magic-link auth for web
- Phone → customer resolution for WhatsApp

**Demo target:** Customer asks to cancel order → assistant confirms → cancellation executes only after explicit "yes".

### Phase 5 — Memory + polish (Week 7-8)

- `sqlite-vec` integration
- Long-term memory extraction (background job)
- Conversation summarization beyond window
- Multi-provider LLM (OpenAI, Groq, Ollama)
- `packages/sdk-python`
- Documentation + examples

**Demo target:** Long-running customer conversations across sessions remember context.

### Phase 6 — Launch prep (Week 9-10)

- Hardening: graceful shutdown, reconnection edge cases, timeouts
- Single-binary distribution
- Railway/Render/Fly templates
- README + Quickstart + Examples
- Docs site
- Public launch (HN, ProductHunt, Twitter)

**Launch target:** Anyone can `docker run` and have a working conversational SaaS interface in 30 minutes.

---

## 15. Open questions to resolve before build starts

1. **Project name.** **Aelio** — confirmed. Domain check pending.
2. **License.** Apache-2.0 vs MIT. Apache-2.0 recommended (patent grant matters when you have a commercial cloud).
3. **Embedding model for memory.** OpenAI `text-embedding-3-small` (1536d) vs local via Ollama. Lean toward local-by-default with override.
4. **Widget hosting.** Served from the user's own server, or CDN? V1: served from their server for self-host purity.
5. **Telemetry.** Anonymous opt-in usage telemetry to learn from real deployments? Highly recommended; defaults off, prompt on first run.
6. **MCP compatibility.** Should the exposed functions *also* be available as an MCP server endpoint (for IDE agents)? Cheap to add — would broaden appeal.

---

## 16. Success criteria

V1 is successful when:

- A new dev can clone, configure, and have a working WhatsApp conversational interface to their SaaS in **under 30 minutes**.
- The SDK installation is **under 10 lines of code**.
- The deployment is **one container** with **one config file**.
- A SaaS in production can run the OSS version **without operational anxiety**.
- The path from OSS → managed cloud is **a config change**, not a rewrite.

---

**End of specification.**
