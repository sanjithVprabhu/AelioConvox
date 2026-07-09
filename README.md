# Aelio

Open-source conversational runtime for SaaS products. Give your customers a production-grade chat experience on Web and WhatsApp — with an always-on analytical agent that understands each user over time.

**Stack:** TypeScript · Rust storage engine (Sunjet/Astrolobe `.vss`) · SQLite + `sqlite-vec` fallback · single-container deployment

## Three products, one system

Aelio ships as three cleanly separated pieces that talk to each other over WebSockets:

```
┌────────────────────────┐        outbound WS /sdk        ┌──────────────────────────┐
│  Your backend          │ ─────────(Bearer secret)─────▶ │  Aelio Server            │
│  @aelio/sdk (Convox)   │ ◀──── invoke / send frames ─── │  agentic runtime         │
│  your auth · DB · logic│                                │  tool loop · safety      │
└────────────────────────┘                                │  memory · Sunjet storage │
                                                          └──────────────────────────┘
┌────────────────────────┐        browser WS /widget/ws            ▲
│  Your website          │ ──────────────────────────────────────┘
│  @aelio/chat (widget)  │ ◀──── ready / message / typing frames
└────────────────────────┘
```

| Product | Package | Where it runs | Role |
|---|---|---|---|
| **Convox SDK** | [`@aelio/sdk`](sdk/node) (npm) · [Python](sdk/python) | Your server | Expose your backend functions as tools. Dials **out** to the Aelio server over one WebSocket (`/sdk`, `Authorization: Bearer <secret>`). Your auth, DB, and business logic stay in your process. |
| **Chat SDK** | [`@aelio/chat`](chat) (npm) | Your website (browser) | Drop-in embeddable chat widget. Connects to the server's `/widget/ws`, handles reconnect + connection status. |
| **Aelio Server** | [`@aelio/server`](server) (Docker) | Your infra | The agentic harness: LLM tool loop, safety rails, identity, memory, and the Sunjet/Astrolobe Rust storage engine. Pull the image, point the other two at it. |

> 📖 **Full setup, configuration, API-wrapping, channels, and deployment: [docs/MANUAL.md](docs/MANUAL.md)**

## Repository layout

```
AelioConvox/
├── sdk/
│   ├── node/          # @aelio/sdk — Convox SDK for Node backends (npm)
│   └── python/        # aelio-sdk — Python SDK (PyPI)
├── chat/              # @aelio/chat — embeddable browser widget (npm) → dist/{index.js, widget.js}
├── server/            # @aelio/server — Fastify runtime + Dockerfile + deploy templates
├── packages/          # internal workspace libs (not published on their own):
│   ├── protocol/      #   wire protocol types + Zod schemas
│   ├── core/          #   runtime turn, session, safety, tool loop, memory
│   ├── db/            #   Drizzle schema (SQLite)
│   ├── llm/           #   LLM providers (mock, Anthropic, OpenAI, Gemini, Groq, Ollama)
│   ├── channels/      #   WhatsApp adapter
│   └── sunjet-client/ #   HTTP client for the Sunjet (ll-server) Rust engine
├── Sunjet/Astrolobe/  # Rust storage engine (git submodule) — the .vss database + ll-server
├── examples/          # nodejs-express, sample-saas, python-fastapi, python-django
├── docs/  scripts/  Blueprint/
└── config.yaml        # single source of runtime config
```

## Quick start (local, mock LLM)

```bash
pnpm install
pnpm build

# Terminal 1 — Aelio server
export AELIO_SDK_SECRET=change-me-in-production
pnpm --filter @aelio/server dev

# Terminal 2 — example SaaS backend wired with the Convox SDK
export AELIO_SDK_SECRET=change-me-in-production
pnpm --filter aelio-example-express start

# Terminal 3 — run the automated phase tests
AELIO_TEST_MODE=1 pnpm test:all
```

Or `pnpm start` to boot the server + example backend together, then open the demo:

Open [http://localhost:3000/demo.html](http://localhost:3000/demo.html), click **Chat**, and ask:

> what is my order status?

Flow: **Web widget → Aelio runtime → LLM tool call → SDK `getOrderStatus` → reply**

## Embed the chat widget

```html
<script
  src="https://your-aelio-server.com/widget.js"
  data-customer-id="user_abc123"
  data-server-url="https://your-aelio-server.com"
></script>
```

The widget shows its connection state (Connecting → Connected, auto-reconnect on drop) and a clear error if the server rejects it — most commonly because the page's origin isn't in `channels.web.allowed_origins`. For local dev add your origin (e.g. `http://127.0.0.1:4173`) or set `allowed_origins: ["*"]`. See [Troubleshooting](docs/MANUAL.md#14-troubleshooting).

## Wrap your backend with the Convox SDK

```ts
import { aelio } from '@aelio/sdk';

aelio.expose(
  'getOrderStatus',
  async ({ orderId }, ctx) => myDb.orders.status(orderId, ctx.customerId),
  { description: 'Look up the status of an order', params: { orderId: 'string' }, safety: 'read' },
);

await aelio.listen({
  secret: process.env.AELIO_SDK_SECRET,
  url: 'ws://127.0.0.1:3000',
});
```

## Use a real LLM

Set in `config.yaml`:

```yaml
llm:
  provider: anthropic
  model: claude-sonnet-4-20250514
  api_key: ${ANTHROPIC_API_KEY}
  fallback:
    provider: openai
    model: gpt-4.1-mini
    api_key: ${OPENAI_API_KEY}
```

## Docker deploy

```bash
pnpm docker:build
pnpm docker:verify        # build image, run container, run all phase tests
pnpm docker:up
# or: docker compose -f server/docker-compose.yml up --build
```

Deploy templates for [Railway](server/railway.toml), [Render](server/render.yaml), and [Fly.io](server/fly.toml) live in [`server/`](server).

## Configuration

Everything is driven by `config.yaml`. See [docs/MANUAL.md](docs/MANUAL.md) for the full reference and [docs/production-testing.md](docs/production-testing.md) for a client-trial checklist.

## License

Apache-2.0 (intent)
