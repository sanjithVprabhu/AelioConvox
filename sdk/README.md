# Aelio Convox SDK

The SDK that runs **inside your backend** and exposes your functions as tools to the Aelio conversational runtime. It opens a single **outbound** WebSocket to the Aelio server at `/sdk`, authenticated with `Authorization: Bearer <secret>`. Your auth, database, and business logic never leave your process — the server only ever asks the SDK to *invoke* a registered function or *send* an outbound message.

Two implementations, one protocol:

| | Package | Path | Install |
|---|---|---|---|
| **Node / TypeScript** | `@aelio/sdk` | [`node/`](node) | `npm i @aelio/sdk` |
| **Python** | `aelio-sdk` | [`python/`](python) | build from [`python/pyproject.toml`](python/pyproject.toml) |

## How it connects

- Transport: WebSocket to `${AELIO_SERVER_URL}/sdk` (default `ws://127.0.0.1:3000`).
- Auth: the shared secret travels in the `Authorization` header, never in the URL.
- Keepalive: server pings every 30s; the SDK pongs and auto-reconnects with exponential backoff.
- Registration: on connect the SDK sends a `register` frame declaring its functions, states, policies, flows, and persona. The server drives the LLM tool loop and calls back with `invoke`/`send` frames.

The wire protocol (message types + Zod schemas) lives in [`packages/protocol`](../packages/protocol).

## Node quick start

```ts
import { aelio } from '@aelio/sdk';

aelio.expose(
  'getOrderStatus',
  async ({ orderId }, ctx) => myDb.orders.status(orderId, ctx.customerId),
  { description: 'Look up the status of an order', params: { orderId: 'string' }, safety: 'read' },
);

await aelio.listen({ secret: process.env.AELIO_SDK_SECRET, url: 'ws://127.0.0.1:3000' });
```

See [`node/README.md`](node/README.md) and [`python/README.md`](python/README.md) for the full API, and [docs/MANUAL.md](../docs/MANUAL.md) for the complete guide.
