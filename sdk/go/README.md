# Aelio Go SDK

Expose Go backend functions to the [Aelio](https://github.com/sanjithVprabhu/AelioConvox)
conversational runtime over a single outbound WebSocket.

## Install

```bash
go get github.com/sanjithVprabhu/AelioConvox/sdk/go@latest
```

## Usage

```go
package main

import (
	"log"
	"os"

	aelio "github.com/sanjithVprabhu/AelioConvox/sdk/go"
)

func main() {
	aelio.Expose("getOrderStatus", aelio.FunctionSchema{
		Description: "Get the status of a customer order",
		Params:      map[string]interface{}{"orderId": "string"},
		Safety:      aelio.SafetyRead,
	}, func(args map[string]interface{}, ctx aelio.InvocationContext) (interface{}, error) {
		return map[string]interface{}{
			"orderId": args["orderId"],
			"status":  "shipped",
		}, nil
	})

	aelio.Persona("You are a helpful support agent for ShopCo.")
	aelio.Describe("ShopCo is an e-commerce SaaS for online retailers.")

	err := aelio.Listen(aelio.ListenOptions{
		Secret: os.Getenv("AELIO_SDK_SECRET"),
		URL:    envOr("AELIO_SERVER_URL", "ws://127.0.0.1:3010"),
	})
	if err != nil {
		log.Fatal(err)
	}
}

func envOr(k, d string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return d
}
```

## API

| Method | Purpose |
|--------|---------|
| `Expose` | Register a tool |
| `Persona` / `Describe` | Assistant voice + product brief |
| `State` / `Policy` / `Flow` | Lifecycle catalog |
| `SetCustomerState` / `SetFlowProgress` | Push lifecycle updates |
| `OnSend` / `Ingest` | BYO messaging |
| `Listen` | Connect (blocks, auto-reconnect) |
| `Disconnect` | Stop and close |

## License

Apache-2.0
