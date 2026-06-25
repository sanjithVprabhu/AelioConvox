import type { AelioDatabase } from '@aelio/db';
import { channelAddresses, customers, messages, proactiveMessages } from '@aelio/db';
import type { Channel } from '@aelio/protocol';
import { and, desc, eq, gte } from 'drizzle-orm';
import { randomUUID } from 'node:crypto';
import { enqueueJob } from '../job-queue/index.js';

export type ProactiveConfig = {
  enabled: boolean;
  requireOptIn: boolean;
  maxPerCustomerPerDay: number;
  windowHours: number;
};

export type ProactiveResult = {
  ok: boolean;
  status: 'sent' | 'blocked';
  reason?: string;
};

export type ProactiveInput = {
  database: AelioDatabase;
  config: ProactiveConfig;
  customerExternalId: string;
  channel: Channel;
  content: string;
  templateName?: string;
  dedupKey?: string;
};

/** Set (or clear) a customer's opt-in for proactive messages. */
export async function setProactiveOptIn(
  database: AelioDatabase,
  customerExternalId: string,
  optIn: boolean,
): Promise<boolean> {
  const rows = await database.db
    .select()
    .from(customers)
    .where(eq(customers.externalId, customerExternalId))
    .limit(1);
  const customer = rows[0];
  if (!customer) {
    return false;
  }
  const metadata = { ...(customer.metadata ?? {}), proactiveOptIn: optIn };
  await database.db
    .update(customers)
    .set({ metadata, updatedAt: new Date() })
    .where(eq(customers.id, customer.id));
  return true;
}

async function record(
  database: AelioDatabase,
  fields: {
    customerId: string;
    channel: Channel;
    to: string;
    content: string;
    dedupKey?: string;
    status: 'sent' | 'blocked';
    reason?: string;
  },
): Promise<void> {
  await database.db.insert(proactiveMessages).values({
    id: randomUUID(),
    customerId: fields.customerId,
    channel: fields.channel,
    toAddress: fields.to,
    content: fields.content,
    dedupKey: fields.dedupKey,
    status: fields.status,
    reason: fields.reason,
    createdAt: new Date(),
  });
}

/**
 * Send a proactive (system-initiated) message to a customer, enforcing every
 * guardrail server-side before anything is delivered:
 *   1. feature enabled   2. known recipient   3. opt-in
 *   4. dedup             5. daily frequency cap
 *   6. WhatsApp 24h window (free-form only inside it; template required outside)
 * On success it enqueues an outbound job (delivered by the same path as replies).
 */
export async function sendProactiveMessage(input: ProactiveInput): Promise<ProactiveResult> {
  const db = input.database.db;

  if (!input.config.enabled) {
    return { ok: false, status: 'blocked', reason: 'Proactive messaging is disabled' };
  }

  const customerRows = await db
    .select()
    .from(customers)
    .where(eq(customers.externalId, input.customerExternalId))
    .limit(1);
  const customer = customerRows[0];
  if (!customer) {
    return { ok: false, status: 'blocked', reason: 'Unknown customer' };
  }

  const addrRows = await db
    .select()
    .from(channelAddresses)
    .where(and(eq(channelAddresses.customerId, customer.id), eq(channelAddresses.channel, input.channel)))
    .limit(1);
  const address = addrRows[0]?.address;
  if (!address) {
    return { ok: false, status: 'blocked', reason: `No ${input.channel} address for customer` };
  }

  const optedIn = (customer.metadata as Record<string, unknown> | null)?.proactiveOptIn === true;
  if (input.config.requireOptIn && !optedIn) {
    await record(input.database, {
      customerId: customer.id,
      channel: input.channel,
      to: address,
      content: input.content,
      dedupKey: input.dedupKey,
      status: 'blocked',
      reason: 'Customer has not opted in',
    });
    return { ok: false, status: 'blocked', reason: 'Customer has not opted in' };
  }

  if (input.dedupKey) {
    const existing = await db
      .select({ id: proactiveMessages.id })
      .from(proactiveMessages)
      .where(and(eq(proactiveMessages.dedupKey, input.dedupKey), eq(proactiveMessages.status, 'sent')))
      .limit(1);
    if (existing[0]) {
      return { ok: false, status: 'blocked', reason: 'Duplicate (dedupKey already sent)' };
    }
  }

  const dayAgo = new Date(Date.now() - 24 * 60 * 60 * 1000);
  const sentToday = await db
    .select({ id: proactiveMessages.id })
    .from(proactiveMessages)
    .where(
      and(
        eq(proactiveMessages.customerId, customer.id),
        eq(proactiveMessages.status, 'sent'),
        gte(proactiveMessages.createdAt, dayAgo),
      ),
    );
  if (sentToday.length >= input.config.maxPerCustomerPerDay) {
    await record(input.database, {
      customerId: customer.id,
      channel: input.channel,
      to: address,
      content: input.content,
      dedupKey: input.dedupKey,
      status: 'blocked',
      reason: 'Daily proactive limit reached',
    });
    return { ok: false, status: 'blocked', reason: 'Daily proactive limit reached' };
  }

  // WhatsApp 24-hour window: outside it, Meta requires a pre-approved template.
  if (input.channel === 'whatsapp' && !input.templateName) {
    const lastInbound = await db
      .select({ createdAt: messages.createdAt })
      .from(messages)
      .where(and(eq(messages.customerId, customer.id), eq(messages.role, 'user')))
      .orderBy(desc(messages.createdAt))
      .limit(1);
    const last = lastInbound[0]?.createdAt;
    const withinWindow =
      last instanceof Date && Date.now() - last.getTime() <= input.config.windowHours * 60 * 60 * 1000;
    if (!withinWindow) {
      await record(input.database, {
        customerId: customer.id,
        channel: input.channel,
        to: address,
        content: input.content,
        dedupKey: input.dedupKey,
        status: 'blocked',
        reason: 'Outside 24h window — a template message is required',
      });
      return {
        ok: false,
        status: 'blocked',
        reason: 'Outside 24h window — a template message is required',
      };
    }
  }

  await enqueueJob(input.database, 'outbound', {
    channel: input.channel,
    to: address,
    text: input.content,
    proactive: true,
    ...(input.templateName ? { templateName: input.templateName } : {}),
  });

  await record(input.database, {
    customerId: customer.id,
    channel: input.channel,
    to: address,
    content: input.content,
    dedupKey: input.dedupKey,
    status: 'sent',
  });

  return { ok: true, status: 'sent' };
}
