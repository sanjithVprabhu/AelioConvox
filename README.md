# Aelio

Open-source conversational runtime for SaaS products. Give your customers a production-grade chat experience on WhatsApp and Web — with an always-on analytical agent that understands each user over time.

**Stack:** TypeScript · SQLite + `sqlite-vec` memory index · single-container deployment

## What Aelio does

1. **Conversational agent** — real-time chat, SDK function calls, safety rails
2. **Analytical agent** — monitors conversations, extracts user understanding, indexes memory in `sqlite-vec`, recalls context, and rolls up session summaries

The **Aelio SDK** runs inside your backend and dials out to the Aelio server. Your auth, DB, and business logic stay untouched.

> 📖 **Full setup, configuration, API-wrapping, channels, and deployment guide: [docs/MANUAL.md](docs/MANUAL.md)**

## End-to-end demo (Phase 2)

```bash
pnpm install
pnpm build

# Terminal 1 — Aelio server (mock LLM by default)
export AELIO_SDK_SECRET=change-me-in-production
pnpm --filter @aelio/server dev

# Terminal 2 — Example SaaS backend with SDK
export AELIO_SDK_SECRET=change-me-in-production
pnpm --filter aelio-example-express start

# Terminal 3 — Run all automated tests
AELIO_TEST_MODE=1 pnpm test:all
```

### Individual phase tests

With server + SDK running (as above), in another terminal:

```bash
AELIO_TEST_MODE=1 pnpm test:phase2   # Web widget → LLM → SDK
AELIO_TEST_MODE=1 pnpm test:phase3   # WhatsApp webhook → queue → mock send
AELIO_TEST_MODE=1 pnpm test:phase4:confirmation  # Write action confirmation flow
AELIO_TEST_MODE=1 pnpm test:phase4:magic-link    # Magic link → verified widget session
AELIO_TEST_MODE=1 pnpm test:phase4:identity      # Phone → persistent WhatsApp customer
AELIO_TEST_MODE=1 pnpm test:phase5   # Analyst extract/recall + memory in chat
```

`pnpm test:all` starts the server and SDK automatically, then runs all phase tests.

### Full diagnostic (Phase 6)

Runs build verification, health/ready checks, graceful shutdown, and all integration tests:

```bash
pnpm diagnostic
```

### Docker deploy

```bash
pnpm docker:build
pnpm docker:verify       # build image, run container, run all phase tests
pnpm docker:up
# or: docker compose -f docker/docker-compose.yml up --build
```

Deploy templates for [Railway](docker/railway.toml), [Render](docker/render.yaml), and [Fly.io](docker/fly.toml) are in `docker/`.

Open the web widget demo at [http://localhost:3000/demo.html](http://localhost:3000/demo.html), click **Chat**, and ask:

> what is my order status?

Flow: **Web widget → Aelio runtime → mock LLM tool call → SDK `getOrderStatus` → reply**

### Use a real LLM

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

## Monorepo layout

```
aelio/
├── apps/
│   ├── server/           # Fastify service (health, SDK WS, widget WS)
│   └── web-widget/       # Embeddable Preact chat widget → widget.js
├── packages/
│   ├── protocol/         # Wire protocol types + Zod schemas
│   ├── db/               # Drizzle schema (SQLite)
│   ├── llm/              # LLM providers (mock, Anthropic, OpenAI, Groq, Ollama)
│   ├── core/             # Runtime turn, session, safety, tool loop
│   ├── sdk-node/         # @aelio/sdk
│   └── sdk-python/       # Minimal Python SDK
├── examples/
│   ├── nodejs-express/
│   ├── python-fastapi/
│   └── python-django/
├── docs/
├── Blueprint/v1-oss-spec.md
└── config.yaml
```

## Widget embed

```html
<script
  src="https://your-aelio-server.com/widget.js"
  data-customer-id="user_abc123"
  data-server-url="https://your-aelio-server.com"
></script>
```

## Configuration

Everything is driven by `config.yaml`. See `Blueprint/v1-oss-spec.md` for the full specification and [docs/production-testing.md](docs/production-testing.md) for a client-trial checklist.

## License

Apache-2.0 (intent)
