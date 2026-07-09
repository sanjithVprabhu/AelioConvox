# Aelio Python SDK

Minimal Python SDK for exposing backend functions to the Aelio server over the same WebSocket protocol as `@aelio/sdk`.

## Install

```bash
pip install websockets
```

## Usage

```python
from sdk import Aelio

aelio = Aelio(secret="change-me-in-production", url="ws://127.0.0.1:3000")

@aelio.expose(
    "getOrderStatus",
    description="Get the status of a customer order",
    params={"orderId": "string"},
    safety="read",
)
async def get_order_status(args, ctx):
    return {"orderId": args["orderId"], "status": "shipped", "customerId": ctx["customerId"]}

aelio.run()
```

## Bring your own messaging channel

Same wire protocol as the Node SDK — deliver replies through your own provider and
push inbound messages from your own webhook. Aelio holds no provider credentials.

```python
# Deliver outbound replies through your provider (called automatically, not a tool).
async def deliver(msg):
    await my_provider.send(to=msg["to"], body=msg["content"])

aelio.on_send(deliver)

# In your async webhook handler, hand Aelio the inbound message:
await aelio.ingest(channel="whatsapp", from_="+15551234567", text="where's my order?")
```

On the Aelio server set `channels.whatsapp.provider: sdk`. Any channel string works
(`telegram`, `slack`, `sms`, …), not just `whatsapp`/`web`.
