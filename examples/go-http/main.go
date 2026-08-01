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
		Output: map[string]aelio.OutputField{
			"orderId": {Type: "string", Meaning: "The order identifier"},
			"status":  {Type: "string", Meaning: "The current fulfillment status"},
		},
	}, func(args map[string]interface{}, ctx aelio.InvocationContext) (interface{}, error) {
		return map[string]interface{}{
			"orderId":    args["orderId"],
			"status":     "shipped",
			"customerId": ctx.CustomerID,
		}, nil
	})

	aelio.Expose("cancelOrder", aelio.FunctionSchema{
		Description: "Cancel a pending order",
		Params:      map[string]interface{}{"orderId": "string"},
		Safety:      aelio.SafetyWrite,
		Output: map[string]aelio.OutputField{
			"cancelled": {Type: "boolean", Meaning: "Whether cancellation succeeded"},
			"orderId":   {Type: "auto", Meaning: "The cancelled order identifier"},
		},
		OutputRole: "effect_confirmation",
	}, func(args map[string]interface{}, ctx aelio.InvocationContext) (interface{}, error) {
		return map[string]interface{}{"cancelled": true, "orderId": args["orderId"]}, nil
	})

	aelio.Persona("You are a helpful ShopCo support agent.")
	aelio.Describe("ShopCo helps online retailers manage orders and shipping.")

	url := os.Getenv("AELIO_SERVER_URL")
	if url == "" {
		url = "ws://127.0.0.1:3010"
	}
	secret := os.Getenv("AELIO_SDK_SECRET")
	if secret == "" {
		secret = "change-me-in-production"
	}

	log.Printf("connecting to Aelio at %s", url)
	if err := aelio.Listen(aelio.ListenOptions{Secret: secret, URL: url}); err != nil {
		log.Fatal(err)
	}
}
