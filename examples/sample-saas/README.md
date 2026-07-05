# ShopCo — sample SaaS backend (AelioConvox demo)

A tiny but realistic backend that shows how to **wrap your existing APIs with the Aelio
SDK**. It keeps an in-memory store (orders, subscription, invoices) and exposes the same
business logic two ways:

1. as a **normal REST API** (`GET /api/orders`, `/api/subscription`, …) — your app today
2. as **Aelio functions** (`aelio.expose(...)`) — so the conversational runtime can call them

State is mutable, so the demo feels real: cancel an order, then list orders, and you'll
see it updated.

## Exposed functions

| Function | Safety | What it does |
|---|---|---|
| `listOrders(status?)` | read | List orders, optional status filter (enum) |
| `getOrderStatus(orderId)` | read | One order's status + tracking |
| `getSubscription()` | read | Current plan + renewal |
| `listInvoices()` | read | Invoices and paid status |
| `cancelOrder(orderId)` | write | Cancel a pending order (asks to confirm) |
| `upgradePlan(plan)` | write | Change plan (enum; asks to confirm) |

## Run the full live demo (server + this backend + chat UI)

From the repo root — one command:

```bash
node scripts/demo.mjs       # or: pnpm demo
```

It boots the Aelio server (OpenAI if a key is in `secrets.md`/`OPENAI_API_KEY`, else the
offline mock), starts this backend, and prints the chat URL:

> 💬 http://localhost:3000/demo.html

Open it, click the chat bubble, and try: *"show me my pending orders"*, *"cancel order
A-1002"* → confirm, *"what plan am I on?"*, *"upgrade me to pro"*.

## Run it manually

```bash
# terminal 1 — Aelio server (real LLM for best tool selection)
AELIO_CONFIG="$(pwd)/config.openai.yaml" OPENAI_API_KEY=sk-... \
  AELIO_SDK_SECRET=change-me-in-production AELIO_TEST_MODE=1 \
  pnpm --filter @aelio/server dev

# terminal 2 — this backend
AELIO_SDK_SECRET=change-me-in-production AELIO_SERVER_URL=ws://127.0.0.1:3000 \
  pnpm --filter aelio-sample-saas start
```
