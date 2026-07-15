# aelio-sdk (Python)

Expose your backend functions to the [Aelio](https://github.com/sanjithVprabhu/AelioConvox)
conversational runtime over a single outbound WebSocket.

## Install

```bash
pip install aelio-sdk
```

## Usage

```python
import asyncio
import os
from aelio import Aelio

aelio = Aelio()

@aelio.expose(
    "get_order_status",
    description="Get the status of a customer order",
    params={"orderId": "string"},
    safety="read",
)
async def get_order_status(args, ctx):
    return {"status": "shipped", "orderId": args["orderId"]}

async def main():
    await aelio.listen(
        secret=os.environ["AELIO_SDK_SECRET"],
        url=os.environ.get("AELIO_SERVER_URL", "ws://127.0.0.1:3010"),
    )

asyncio.run(main())
```

## Bring-your-own channel

```python
async def deliver(msg):
    await my_provider.send(to=msg["to"], body=msg["content"])

aelio.on_send(deliver)

# In your webhook:
await aelio.ingest(channel="whatsapp", from_=from_number, text=body)
```

## API

| Method | Purpose |
|--------|---------|
| `expose(name, description, params, safety)` | Register a tool (decorator) |
| `persona(text)` / `describe(text)` | Assistant voice + product brief |
| `state` / `policy` / `flow` | Lifecycle catalog |
| `set_customer_state` / `set_flow_progress` | Push lifecycle updates |
| `on_send` / `ingest` | BYO messaging |
| `listen(secret=..., url=...)` | Connect (auto-reconnect) |
| `disconnect()` | Stop reconnecting and close |

## License

Apache-2.0
