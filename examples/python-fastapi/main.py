import os

from fastapi import FastAPI

from sdk import Aelio

app = FastAPI()
aelio = Aelio(secret=os.getenv("AELIO_SDK_SECRET", "change-me-in-production"))


@aelio.expose(
    "getOrderStatus",
    description="Get the status of a customer order",
    params={"orderId": "string"},
    safety="read",
)
async def get_order_status(args, ctx):
    return {
        "orderId": args.get("orderId"),
        "customerId": ctx.get("customerId"),
        "status": "shipped",
    }


@app.get("/health")
async def health():
    return {"ok": True}


if __name__ == "__main__":
    aelio.run()
