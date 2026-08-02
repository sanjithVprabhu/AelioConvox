import { aelio } from '@aelio/sdk';
import express from 'express';

const app = express();

aelio.state('active', {
  description: 'Standard customer with full access to order and subscription tools.',
});

aelio.policy('concise', {
  description: 'Keep replies short and actionable.',
  severity: 'soft',
});

aelio.expose(
  'getOrderStatus',
  async ({ orderId }, ctx) => {
    return {
      orderId,
      customerId: ctx.customerId,
      status: 'shipped',
      tracking: '1Z999AA10123456784',
    };
  },
  {
    description: 'Get the status of a customer order',
    params: { orderId: 'string' },
    safety: 'read',
    intent: 'get_order_status',
    output: {
      orderId: { type: 'string', meaning: 'The order identifier' },
      status: { type: 'string', meaning: 'The current fulfillment status' },
      tracking: { type: 'string', meaning: 'The shipment tracking number' },
    },
  },
);

aelio.expose(
  'cancelOrder',
  async ({ orderId }, ctx) => {
    return {
      orderId,
      customerId: ctx.customerId,
      status: 'cancelled',
      cancelledAt: new Date().toISOString(),
    };
  },
  {
    description: 'Cancel a pending customer order',
    params: { orderId: 'string' },
    safety: 'write',
    intent: 'cancel_order',
    output: {
      orderId: { type: 'string', meaning: 'The cancelled order identifier' },
      status: { type: 'string', meaning: 'The resulting order status' },
      // `cancelledAt` is intentionally not declared as evidence. The handler may return volatile
      // operational metadata, but Rust's closed output allowlist admits only stable guarantees.
    },
    outputRole: 'effect_confirmation',
  },
);

// Demonstrates optional parameters: `status` and `limit` are optional, so the
// assistant can call this with just a bare request ("show my orders") without
// asking the user for filters it doesn't need.
aelio.expose(
  'listOrders',
  async ({ status, limit }: { status?: string; limit?: number }, ctx) => {
    const all = [
      { orderId: 'A123', status: 'shipped' },
      { orderId: 'B456', status: 'pending' },
      { orderId: 'C789', status: 'shipped' },
    ];
    const filtered = status ? all.filter((o) => o.status === status) : all;
    return {
      customerId: ctx.customerId,
      count: filtered.length,
      orders: filtered.slice(0, limit ?? filtered.length),
    };
  },
  {
    description: "List the customer's orders, optionally filtered by status",
    params: {
      status: { type: 'string', description: 'Filter by order status', enum: ['shipped', 'pending'], optional: true },
      limit: 'number?',
    },
    safety: 'read',
    intent: 'list_orders',
    output: {
      count: { type: 'number', meaning: 'The number of matching orders' },
      orders: { type: 'array', meaning: 'The matching customer order records' },
    },
  },
);

await aelio.listen({
  secret: process.env.AELIO_SDK_SECRET ?? 'change-me-in-production',
  url: process.env.AELIO_SERVER_URL ?? `ws://127.0.0.1:${process.env.AELIO_PORT ?? '3010'}`,
});

app.get('/health', (_req, res) => {
  res.json({ ok: true });
});

const port = Number(process.env.PORT ?? 8080);
app.listen(port, () => {
  console.log(`Example SaaS backend listening on :${port} with Aelio SDK connected`);
});
