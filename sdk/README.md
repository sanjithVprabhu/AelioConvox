# Aelio SDKs

Connect your application backend to a running Aelio server. The SDK dials **out**
over WebSocket — no inbound ports on your side.

| Language | Package | Install |
|----------|---------|---------|
| **Node.js** | [`@aelio/sdk`](./node) | `npm install @aelio/sdk` |
| **Python** | [`aelio-sdk`](./python) | `pip install aelio-sdk` |
| **Go** | [`sdk/go`](./go) | `go get github.com/sanjithVprabhu/AelioConvox/sdk/go` |

Full walkthrough (Docker server + any SDK): [docs/GETTING_STARTED.md](../docs/GETTING_STARTED.md)

## Minimal example (same idea in every language)

1. Run the server (`docker run …` or `pnpm start`)
2. Expose tools from your backend
3. `listen({ secret, url })`

See each language folder for full API (persona, describe, lifecycle states, BYO channels).
