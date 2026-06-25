# @aelio/sdk

The Node.js/TypeScript SDK for [Aelio](https://github.com/aelio-dev/aelio) — the open-source conversational runtime for SaaS products.

Install it in your existing backend, expose a few functions, and Aelio gives your
customers a chat/voice experience on WhatsApp and Web. Your auth, DB, and business
logic stay in your process — the SDK dials **out** to the Aelio server over a single
persistent WebSocket, so there are no inbound ports or webhooks to configure.

## Install

```bash
npm install @aelio/sdk
```

## Usage

```typescript
import { aelio } from '@aelio/sdk'

aelio.expose('getOrderStatus', async ({ orderId }, ctx) => {
  // ctx.customerId is your own user ID — scope your queries normally
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
  safety: 'write' // write actions are confirmed with the user before running
})

await aelio.listen({
  secret: process.env.AELIO_SECRET,
  url: process.env.AELIO_SERVER_URL ?? 'ws://127.0.0.1:3000',
})
```

That's the whole integration.

## Declaring parameters

Each parameter in `params` can be declared three interchangeable ways — mix them freely:

```typescript
aelio.expose('listOrders', handler, {
  description: "List the customer's orders, optionally filtered",
  params: {
    customerRef: 'string',                 // required string
    status: 'string?',                      // optional — trailing "?"
    limit: { type: 'number', optional: true },
    sort: {
      type: 'string',
      description: 'Sort order',            // shown to the model
      enum: ['newest', 'oldest'],          // constrains the choice
      optional: true,
    },
    tags: { type: 'array', items: 'string', optional: true },
  },
  safety: 'read',
})
```

| Form | Example | Meaning |
|---|---|---|
| Shorthand | `'string'` | required, type only |
| Optional shorthand | `'string?'` | optional (trailing `?`) |
| Object | `{ type, description?, optional?, enum?, format?, items? }` | full control |

Supported types: `string`, `number`, `integer`, `boolean`, `array`, `object`.

Two things Aelio does for you with this:
- **Required vs optional** is passed to the LLM correctly, so it won't pester the user for values you marked optional, and it *will* gather the ones you require.
- **Missing required arguments are caught** before your handler runs — Aelio asks the model to collect them instead of calling your function with `undefined`.

## Bring your own messaging channel

Aelio ships a built-in Meta WhatsApp adapter (configure credentials in `config.yaml`
and you're done). But if you use a *different* provider — Twilio, Gupshup, 360dialog,
SMS, anything — you can wire it from your own backend with two hooks, and Aelio keeps
**zero provider code and zero provider credentials**:

```typescript
// 1) Deliver replies through YOUR provider. Aelio calls this after each turn
//    (and for proactive messages). It is NOT an LLM tool — delivery is automatic.
aelio.onSend(async ({ channel, to, content }) => {
  await myProvider.messages.create({ to, body: content })
})

// 2) In your own webhook route, hand Aelio inbound messages. You own the webhook,
//    signature verification, and provider parsing; Aelio takes over identity,
//    memory, the LLM turn, safety, and the reply.
app.post('/my-whatsapp-webhook', (req, res) => {
  const { from, text } = myProvider.parse(req.body)
  aelio.ingest({ channel: 'whatsapp', from, text })
  res.sendStatus(200)
})
```

On the Aelio server, set the channel provider to `sdk`:

```yaml
channels:
  whatsapp:
    enabled: true
    provider: sdk   # inbound via ingest(), delivery via onSend — no creds on Aelio
```

Adding a new provider becomes ~20 lines in your codebase, with no Aelio changes.

## Safety levels

| Level | Behavior |
|---|---|
| `read` | Auto-allowed. The assistant can call freely. |
| `write` | Requires explicit user confirmation in the conversation before it runs. |
| `destructive` | Blocked from chat in V1. |

## API

- `aelio.expose(name, handler, schema)` — register a callable function.
- `aelio.listen({ secret, url? })` — connect to the Aelio server (auto-reconnect + heartbeat).
- `aelio.disconnect()` — drain and close.

## License

Apache-2.0
