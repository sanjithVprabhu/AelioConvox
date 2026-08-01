/**
 * Sample SaaS backend — "ShopCo".
 *
 * This is a tiny but realistic backend: it has its own in-memory "database" and
 * exposes its business logic both as normal REST endpoints (your existing API) AND
 * as Aelio functions (so the conversational runtime can call them). The same
 * functions back both — that's the point: you wrap what you already have.
 *
 * State is mutable, so the demo feels alive: cancel an order, then ask to list
 * orders, and you'll see it reflected.
 *
 * Run:  pnpm --filter aelio-sample-saas start
 */
import { aelio } from '@aelio/sdk';
import express from 'express';

// ---------------------------------------------------------------------------
// In-memory "database", keyed by customer id (lazily seeded so ANY customer works).
// ---------------------------------------------------------------------------
type Order = {
  id: string;
  item: string;
  status: 'shipped' | 'pending' | 'cancelled';
  total: number;
  tracking?: string;
};
type Customer = {
  name: string;
  orders: Order[];
  subscription: { plan: 'starter' | 'pro' | 'enterprise'; renewsOn: string };
  invoices: Array<{ id: string; amount: number; paid: boolean }>;
};

const db = new Map<string, Customer>();

function customer(id: string): Customer {
  if (!db.has(id)) {
    db.set(id, {
      name: 'Demo Customer',
      orders: [
        { id: 'A-1001', item: 'Wireless Headphones', status: 'shipped', total: 129, tracking: '1Z999AA10123456784' },
        { id: 'A-1002', item: 'USB-C Charger', status: 'pending', total: 25 },
        { id: 'A-1003', item: 'Laptop Stand', status: 'shipped', total: 49, tracking: '1Z999AA10987654321' },
      ],
      subscription: { plan: 'starter', renewsOn: '2026-08-01' },
      invoices: [
        { id: 'INV-501', amount: 129, paid: true },
        { id: 'INV-502', amount: 25, paid: false },
      ],
    });
  }
  return db.get(id)!;
}

// ---------------------------------------------------------------------------
// Business logic — plain functions. These ARE your "APIs".
// ---------------------------------------------------------------------------
const api = {
  listOrders(customerId: string, status?: string) {
    const orders = customer(customerId).orders;
    return { orders: status ? orders.filter((o) => o.status === status) : orders };
  },
  getOrderStatus(customerId: string, orderId: string) {
    const order = customer(customerId).orders.find((o) => o.id === orderId);
    if (!order) return { error: `No order ${orderId} found` };
    return order;
  },
  cancelOrder(customerId: string, orderId: string) {
    const order = customer(customerId).orders.find((o) => o.id === orderId);
    if (!order) return { error: `No order ${orderId} found` };
    if (order.status === 'shipped') return { error: `Order ${orderId} already shipped and can't be cancelled` };
    order.status = 'cancelled';
    return { orderId, status: 'cancelled' };
  },
  getSubscription(customerId: string) {
    return customer(customerId).subscription;
  },
  upgradePlan(customerId: string, plan: 'starter' | 'pro' | 'enterprise') {
    customer(customerId).subscription.plan = plan;
    return { plan, status: 'active' };
  },
  listInvoices(customerId: string) {
    return { invoices: customer(customerId).invoices };
  },
};

// ---------------------------------------------------------------------------
// 1) Your normal REST API (what you already have today).
// ---------------------------------------------------------------------------
const app = express();
const cid = (req: express.Request) => String(req.query.customerId ?? 'demo-user-123');
app.get('/api/orders', (req, res) => res.json(api.listOrders(cid(req), req.query.status as string | undefined)));
app.get('/api/orders/:id', (req, res) => res.json(api.getOrderStatus(cid(req), req.params.id)));
app.get('/api/subscription', (req, res) => res.json(api.getSubscription(cid(req))));
app.get('/api/invoices', (req, res) => res.json(api.listInvoices(cid(req))));
app.get('/health', (_req, res) => res.json({ ok: true }));

// ---------------------------------------------------------------------------
// 2) Lifecycle catalog — states, policies, and guided flows (SDK → Aelio server).
//    Your backend decides each customer's current state via setCustomerState().
// ---------------------------------------------------------------------------
aelio.persona(
  'You are the ShopCo assistant. Be friendly and efficient; refer to the product as ShopCo. Use tools for any account-specific data.',
);

aelio.state('onboarding', {
  description:
    'New customer setting up ShopCo for the first time. Help with setup only: exploring orders, checking subscription, listing invoices. Do NOT discuss plan upgrades, cancellations, or churn retention offers.',
  allowedTools: ['listOrders', 'getOrderStatus', 'getSubscription', 'listInvoices'],
  blockedTools: ['cancelOrder', 'upgradePlan'],
});

aelio.state('active', {
  description:
    'Fully onboarded paying customer. Full product support: orders, subscription, upgrades, and cancellations (with confirmation).',
});

aelio.state('churn_risk', {
  description:
    'Customer may be leaving. Be empathetic and retention-focused. Do NOT push new feature upsells. Focus on understanding issues and keeping them.',
  blockedTools: ['upgradePlan'],
});

aelio.policy('stay-in-lifecycle', {
  description: 'Only discuss topics appropriate for the customer\'s current lifecycle state.',
  severity: 'hard',
});

aelio.policy('pricing-clarity', {
  description: 'When discussing plans, be transparent and avoid inventing prices.',
  severity: 'soft',
});

aelio.flow('onboarding_setup', {
  state: 'onboarding',
  description: 'Guide the customer through initial setup: review orders → check subscription → explore invoices.',
  steps: {
    review_orders: { goal: 'Review their existing orders', tool: 'listOrders' },
    check_plan: { goal: 'Check their current subscription plan', tool: 'getSubscription' },
    view_invoices: { goal: 'Review invoice status', tool: 'listInvoices' },
  },
});

// ---------------------------------------------------------------------------
// 3) Wrap the SAME functions with Aelio so the assistant can call them.
//    ctx.customerId is resolved by Aelio from the channel — you just scope to it.
// ---------------------------------------------------------------------------
aelio.expose('listOrders', async ({ status }, ctx) => api.listOrders(ctx.customerId, status as string | undefined), {
  description: "List the customer's orders, optionally filtered by status",
  params: { status: { type: 'string', enum: ['shipped', 'pending', 'cancelled'], optional: true } },
  safety: 'read',
  intent: 'list_orders',
  output: {
    orders: { type: 'array', meaning: 'The customer orders matching the requested filter' },
  },
});

aelio.expose('getOrderStatus', async ({ orderId }, ctx) => api.getOrderStatus(ctx.customerId, orderId as string), {
  description: 'Get the status and tracking of a specific order by its id (e.g. A-1002)',
  params: { orderId: 'string' },
  safety: 'read',
  intent: 'get_order_status',
  output: {
    id: { type: 'string', meaning: 'The order identifier' },
    item: { type: 'string', meaning: 'The ordered item name' },
    status: { type: 'string', meaning: 'The current fulfillment status' },
    total: { type: 'number', meaning: 'The order total' },
  },
});

aelio.expose('getSubscription', async (_args, ctx) => api.getSubscription(ctx.customerId), {
  description: "Get the customer's current subscription plan and renewal date",
  params: {},
  safety: 'read',
  intent: 'get_subscription',
  output: {
    plan: { type: 'string', meaning: 'The active subscription plan' },
    renewsOn: { type: 'string', meaning: 'The subscription renewal date' },
  },
});

aelio.expose('listInvoices', async (_args, ctx) => api.listInvoices(ctx.customerId), {
  description: "List the customer's invoices and whether they are paid",
  params: {},
  safety: 'read',
  intent: 'list_invoices',
  output: {
    invoices: { type: 'array', meaning: 'The customer invoice records' },
  },
});

aelio.expose('cancelOrder', async ({ orderId }, ctx) => api.cancelOrder(ctx.customerId, orderId as string), {
  description: 'Cancel a pending order by its id',
  params: { orderId: 'string' },
  safety: 'write', // Aelio asks the customer to confirm before this runs
  intent: 'cancel_order',
  output: {
    orderId: { type: 'string', meaning: 'The cancelled order identifier' },
    status: { type: 'string', meaning: 'The resulting order status' },
  },
  outputRole: 'effect_confirmation',
});

aelio.expose('upgradePlan', async ({ plan }, ctx) => api.upgradePlan(ctx.customerId, plan as 'starter' | 'pro' | 'enterprise'), {
  description: 'Upgrade or change the subscription plan',
  params: { plan: { type: 'string', enum: ['starter', 'pro', 'enterprise'], description: 'Target plan' } },
  safety: 'write',
  intent: 'upgrade_subscription',
  output: {
    plan: { type: 'string', meaning: 'The resulting subscription plan' },
    status: { type: 'string', meaning: 'The resulting subscription status' },
  },
  outputRole: 'effect_confirmation',
});

const PORT = Number(process.env.PORT ?? 8081);
const DEMO_CUSTOMER_ID = 'demo-user-123';

await aelio.listen({
  secret: process.env.AELIO_SDK_SECRET ?? 'change-me-in-production',
  url: process.env.AELIO_SERVER_URL ?? `ws://127.0.0.1:${process.env.AELIO_PORT ?? '3010'}`,
});

// Demo users start in onboarding — your real app would set this from your DB on login/events.
aelio.setCustomerState(DEMO_CUSTOMER_ID, 'onboarding', 'demo_seed');

app.listen(PORT, () => {
  console.log(`ShopCo sample backend on :${PORT} — Aelio SDK connected (6 tools, 3 states, 2 policies, 1 flow)`);
});
