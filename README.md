# Aelio

Open-source conversational runtime for SaaS products. Give your customers a production-grade chat experience on Web and WhatsApp — with an always-on analytical agent that understands each user over time.

**Stack:** TypeScript decision plane · Rust storage engine (Sunjet/Astrolobe `.vss` — vectors, BM25, graph; Sunjet-only, no SQLite) · single-container deployment

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
| **Convox SDK** | [`@aelio/sdk`](sdk/node) · [`aelio-sdk`](sdk/python) · [`sdk/go`](sdk/go) | Your server | Expose backend functions as tools over outbound WebSocket `/sdk`. |
| **Chat SDK** | [`@aelio/chat`](chat) (npm) | Your website | Embeddable chat widget → `/widget/ws`. |
| **Aelio Server** | Docker `aelio-server` | Your infra | Agentic harness — pull the image, point SDKs at it. |

> 📖 **Docker + SDK quickstart:** [docs/GETTING_STARTED.md](docs/GETTING_STARTED.md) · Full manual: [docs/MANUAL.md](docs/MANUAL.md)

## Repository layout

```
AelioConvox/
├── sdk/
│   ├── node/          # @aelio/sdk — Convox SDK for Node backends (npm)
│   ├── python/        # aelio-sdk — Python SDK (PyPI)
│   └── go/            # Go SDK module
├── chat/              # @aelio/chat — embeddable browser widget (npm) → dist/{index.js, widget.js}
├── server/            # @aelio/server — Fastify runtime + Dockerfile + deploy templates
├── packages/          # internal workspace libs (not published on their own):
│   ├── protocol/      #   wire protocol types + Zod schemas
│   ├── core/          #   turn pipeline, harness, context/pathway/stance engines, decision journal, memory
│   ├── llm/           #   LLM providers (mock, Anthropic, OpenAI, Gemini, Groq, Ollama)
│   ├── channels/      #   WhatsApp adapter
│   └── sunjet-client/ #   HTTP client for the Sunjet (ll-server) Rust engine
├── Sunjet/Astrolobe/  # Rust storage engine (git submodule) — the .vss database + ll-server
├── examples/          # nodejs-express, sample-saas, python-fastapi, python-django, go-http
├── docs/  scripts/  Blueprint/
└── config.yaml        # single source of runtime config
```

## Quick start (Docker)

```bash
docker pull sanjithvprabhu/aelio-server:latest

docker run -d --name aelio -p 3010:3000 -v aelio-data:/data \
  -e AELIO_SDK_SECRET=change-me \
  -e AELIO_LLM_PROVIDER=openai \
  -e OPENAI_API_KEY=sk-... \
  sanjithvprabhu/aelio-server:latest
```

One image runs **Aelio + Sunjet**. Swap LLM with `AELIO_LLM_PROVIDER` / provider API key;
cloud `.vss` with `AELIO_SUNJET_SEGMENT_BACKEND=s3` + `AELIO_SUNJET_S3_*`.
Full matrix: [docs/GETTING_STARTED.md](docs/GETTING_STARTED.md).


## Quick start (local)

The default LLM provider is **OpenAI**. Set `OPENAI_API_KEY` for real responses;
without it the server degrades to a built-in mock LLM (with a warning) so you can
try everything with zero config.

```bash
pnpm install
pnpm build

# Terminal 1 — Aelio server
export AELIO_SDK_SECRET=change-me-in-production
export OPENAI_API_KEY=sk-...        # optional; omit to run on the mock LLM
pnpm --filter @aelio/server dev

# Terminal 2 — example SaaS backend wired with the Convox SDK
export AELIO_SDK_SECRET=change-me-in-production
pnpm --filter aelio-example-express start

# Terminal 3 — run the automated phase tests
AELIO_TEST_MODE=1 pnpm test:all
```

Or `pnpm start` to boot the server + example backend together, then open the demo:

Open [http://localhost:3010/demo.html](http://localhost:3010/demo.html), click **Chat**, and ask:

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
  url: 'ws://127.0.0.1:3010',
});
```

## Choosing an LLM provider

OpenAI is the default (`config.yaml`). To use a different provider, either point
`AELIO_CONFIG` at a ready-made profile — `config.openai.yaml`, `config.anthropic.yaml`,
or `config.gemini.yaml` — or edit the `llm` block:

```yaml
llm:
  provider: openai            # openai | anthropic | gemini | groq | ollama | mock
  model: gpt-4o-mini
  api_key: ${OPENAI_API_KEY}
  fallback:                   # optional — takes over if the primary fails
    provider: anthropic
    model: claude-sonnet-4-6
    api_key: ${ANTHROPIC_API_KEY}
```

A keyless real provider degrades to the mock LLM with a warning; set
`AELIO_REQUIRE_LLM_KEY=1` to make a missing key a hard boot error instead. See
[`packages/llm/README.md`](packages/llm/README.md) for the provider matrix.

## Docker deploy

```bash
pnpm docker:build
pnpm docker:verify        # build image, run container, run all phase tests
pnpm docker:up
# or: docker compose -f server/docker-compose.yml up --build
```

Deploy templates for [Railway](server/railway.toml), [Render](server/render.yaml), and [Fly.io](server/fly.toml) live in [`server/`](server).

## Conversational intelligence engines

Every turn runs through a Sunjet-backed decision layer before the LLM sees a word:

- **Generic gate** — bare greetings/thanks/farewells answered from templates: zero embeddings, zero LLM calls.
- **Immediate Context Engine** — hot 5-minute verbatim window sliding through condensed 15m/30m/1h/24h tiers, per customer, across sessions and channels.
- **Semantic Pathway Engine** — one message embedding fanned out in parallel to tool/memory/policy/flow ranking; picks the intent and response strategy (`reply`/`execute`/`guide`/`resume`/`disengage`).
- **Archetype stance engine** — positive/negative/neutral valence per category with sentence-span attribution, plus a self-learning aspect taxonomy (discovered aspects are staged as candidates and only steer replies once promoted).
- **Decision journal** — every decision (`pathway`, `stance`, `prompt`, `reply`, `cache`, `generic`, `confirmation`, `proactive`) is journaled to Sunjet; reconstruct any turn via `GET /api/v1/admin/harness/turns/:turnId`. Set `logging.trace_prompt: redacted` to keep prompt PII out of traces.

The LLM only words the completed decision — hard policies, confirmations, and lifecycle gates stay deterministic. Roadmap and audit map: [docs/harness-recall-audit-checklist.md](docs/harness-recall-audit-checklist.md).

## Configuration

Everything is driven by `config.yaml`. See [docs/MANUAL.md](docs/MANUAL.md) for the full reference and [docs/production-testing.md](docs/production-testing.md) for a client-trial checklist.

## License

Apache-2.0 (intent)
