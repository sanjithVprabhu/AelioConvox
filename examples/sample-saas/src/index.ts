/**
 * Sample SaaS backend — "ShopCo" (advanced pipeline demo).
 *
 * Every new customer id starts at pipeline stage `unverified` and walks through:
 *   unverified → verified → onboarding → active
 *
 * Run:  pnpm --filter aelio-sample-saas start
 * Demo: http://127.0.0.1:3010/demo.html?customer=new-user-001
 */
import { aelio } from '@aelio/sdk';
import express from 'express';
import { loadShopCoManifest } from './load-manifest.js';

// ---------------------------------------------------------------------------
// In-memory "database"
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
  email?: string;
  shoppingPreference?: string;
  orders: Order[];
  subscription: { plan: 'starter' | 'pro' | 'enterprise'; renewsOn: string };
  invoices: Array<{ id: string; amount: number; paid: boolean }>;
};

const db = new Map<string, Customer>();

function customer(id: string): Customer {
  if (!db.has(id)) {
    db.set(id, {
      name: 'New Shopper',
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
// REST API
// ---------------------------------------------------------------------------
const app = express();
app.use(express.json());

const cid = (req: express.Request) => String(req.query.customerId ?? req.params.customerId ?? 'demo-user-123');

app.get('/api/orders', (req, res) => res.json(api.listOrders(cid(req), req.query.status as string | undefined)));
app.get('/api/orders/:id', (req, res) => res.json(api.getOrderStatus(cid(req), req.params.id)));
app.get('/api/subscription', (req, res) => res.json(api.getSubscription(cid(req))));
app.get('/api/invoices', (req, res) => res.json(api.listInvoices(cid(req))));
app.get('/api/profile/:customerId', (req, res) => {
  const row = customer(req.params.customerId);
  res.json({ name: row.name, email: row.email, shoppingPreference: row.shoppingPreference });
});
app.get('/health', (_req, res) => res.json({ ok: true }));

// ---------------------------------------------------------------------------
// Aelio persona + pipeline (from YAML manifest)
// ---------------------------------------------------------------------------
loadShopCoManifest();

// SaaS tools — implemented here; referenced by flow steps in the YAML manifest.
// syncProfile, listOrders, etc. are YOUR backend functions, not Aelio internals.
aelio.expose('syncProfile', async (_args, ctx) => {
  const row = customer(ctx.customerId);
  return {
    synced: true,
    profile: { name: row.name, email: row.email, shoppingPreference: row.shoppingPreference },
    note: 'Intake fields are persisted by Aelio; this tool marks the profile checkpoint in the tour.',
  };
}, {
  description: 'Persist collected profile fields (name, email, preference) to the ShopCo account',
  params: {},
  safety: 'read',
  intent: 'onboarding',
});

aelio.expose('listOrders', async ({ status }, ctx) => api.listOrders(ctx.customerId, status as string | undefined), {
  description: "List the customer's orders, optionally filtered by status",
  params: { status: { type: 'string', enum: ['shipped', 'pending', 'cancelled'], optional: true } },
  safety: 'read',
  intent: 'order_inquiry',
});

aelio.expose('getOrderStatus', async ({ orderId }, ctx) => api.getOrderStatus(ctx.customerId, orderId as string), {
  description: 'Get the status and tracking of a specific order by its id (e.g. A-1002)',
  params: { orderId: 'string' },
  safety: 'read',
  intent: 'order_inquiry',
});

aelio.expose('getSubscription', async (_args, ctx) => api.getSubscription(ctx.customerId), {
  description: "Get the customer's current subscription plan and renewal date",
  params: {},
  safety: 'read',
  intent: 'subscription',
});

aelio.expose('listInvoices', async (_args, ctx) => api.listInvoices(ctx.customerId), {
  description: "List the customer's invoices and whether they are paid",
  params: {},
  safety: 'read',
  intent: 'billing',
});

aelio.expose('cancelOrder', async ({ orderId }, ctx) => api.cancelOrder(ctx.customerId, orderId as string), {
  description: 'Cancel a pending order by its id',
  params: { orderId: 'string' },
  safety: 'write',
  intent: 'cancellation',
});

aelio.expose('upgradePlan', async ({ plan }, ctx) => api.upgradePlan(ctx.customerId, plan as 'starter' | 'pro' | 'enterprise'), {
  description: 'Upgrade or change the subscription plan',
  params: { plan: { type: 'string', enum: ['starter', 'pro', 'enterprise'], description: 'Target plan' } },
  safety: 'write',
  intent: 'subscription',
});

// ---------------------------------------------------------------------------
// Boot — do NOT pre-seed pipeline stage; every new customer id starts at `unverified`
// ---------------------------------------------------------------------------
const PORT = Number(process.env.PORT ?? 8081);

await aelio.listen({
  secret: process.env.AELIO_SDK_SECRET ?? 'change-me-in-production',
  url: process.env.AELIO_SERVER_URL ?? `ws://127.0.0.1:${process.env.AELIO_PORT ?? '3010'}`,
});

app.listen(PORT, () => {
  console.log(`ShopCo pipeline demo on :${PORT}`);
  console.log('  Stages: unverified → verified → onboarding → active');
  console.log('  Demo UI: http://127.0.0.1:3010/demo.html?customer=new-user-001');
});
