import type { Channel } from '@aelio/protocol';
import { enqueueJob } from '../job-queue/index.js';
import type { ConvoxCustomerStore } from '../storage/customers.js';
import type { ConvoxJobStore } from '../storage/jobs.js';
import type { ConvoxMessageStore } from '../storage/messages.js';
import type { ConvoxProactiveStore } from '../storage/proactive-store.js';

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
  config: ProactiveConfig;
  customerExternalId: string;
  channel: Channel;
  content: string;
  templateName?: string;
  dedupKey?: string;
  customerStore: ConvoxCustomerStore;
  messageStore: ConvoxMessageStore;
  proactiveStore: ConvoxProactiveStore;
  jobStore: ConvoxJobStore;
};

/** Set (or clear) a customer's opt-in for proactive messages. */
export async function setProactiveOptIn(
  customerExternalId: string,
  optIn: boolean,
  customerStore: ConvoxCustomerStore,
): Promise<boolean> {
  if (!customerStore) {
    throw new Error('AelioDb customerStore is required');
  }
  const customer = await customerStore.getByExternalId(customerExternalId);
  if (!customer) {
    return false;
  }
  await customerStore.updateMetadata(customer.id, { ...customer.metadata, proactiveOptIn: optIn });
  return true;
}

async function record(
  fields: {
    customerId: string;
    channel: Channel;
    to: string;
    content: string;
    dedupKey?: string;
    status: 'sent' | 'blocked';
    reason?: string;
  },
  proactiveStore: ConvoxProactiveStore,
): Promise<void> {
  await proactiveStore.record(fields);
}

/**
 * Send a proactive (system-initiated) message to a customer, enforcing every
 * guardrail server-side before anything is delivered:
 *   1. feature enabled   2. known recipient   3. opt-in
 *   4. dedup             5. daily frequency cap
 *   6. WhatsApp 24h window (free-form only inside it; template required outside)
 * On success it enqueues an outbound job (delivered by the same path as replies).
 * Requires AelioDb customerStore/messageStore/proactiveStore/jobStore.
 */
export async function sendProactiveMessage(input: ProactiveInput): Promise<ProactiveResult> {
  if (!input.customerStore) {
    throw new Error('AelioDb customerStore is required');
  }
  if (!input.messageStore) {
    throw new Error('AelioDb messageStore is required');
  }
  if (!input.proactiveStore) {
    throw new Error('AelioDb proactiveStore is required');
  }
  if (!input.jobStore) {
    throw new Error('AelioDb jobStore is required');
  }

  if (!input.config.enabled) {
    return { ok: false, status: 'blocked', reason: 'Proactive messaging is disabled' };
  }

  const customer = await input.customerStore.getByExternalId(input.customerExternalId);
  if (!customer) {
    return { ok: false, status: 'blocked', reason: 'Unknown customer' };
  }
  const customerId = customer.id;
  const address = (await input.customerStore.getChannelAddress(customerId, input.channel)) ?? undefined;
  const optedIn = customer.metadata?.proactiveOptIn === true;

  if (!address) {
    return { ok: false, status: 'blocked', reason: `No ${input.channel} address for customer` };
  }

  if (input.config.requireOptIn && !optedIn) {
    await record(
      {
        customerId,
        channel: input.channel,
        to: address,
        content: input.content,
        dedupKey: input.dedupKey,
        status: 'blocked',
        reason: 'Customer has not opted in',
      },
      input.proactiveStore,
    );
    return { ok: false, status: 'blocked', reason: 'Customer has not opted in' };
  }

  if (input.dedupKey) {
    const existing = await input.proactiveStore.findSentByDedup(input.dedupKey);
    if (existing) {
      return { ok: false, status: 'blocked', reason: 'Duplicate (dedupKey already sent)' };
    }
  }

  const dayAgo = Date.now() - 24 * 60 * 60 * 1000;
  const sentTodayCount = await input.proactiveStore.countSentSince(customerId, dayAgo);
  if (sentTodayCount >= input.config.maxPerCustomerPerDay) {
    await record(
      {
        customerId,
        channel: input.channel,
        to: address,
        content: input.content,
        dedupKey: input.dedupKey,
        status: 'blocked',
        reason: 'Daily proactive limit reached',
      },
      input.proactiveStore,
    );
    return { ok: false, status: 'blocked', reason: 'Daily proactive limit reached' };
  }

  // WhatsApp 24-hour window: outside it, Meta requires a pre-approved template.
  if (input.channel === 'whatsapp' && !input.templateName) {
    const lastInboundAt = await input.messageStore.lastUserMessageAt(customerId);
    const withinWindow =
      typeof lastInboundAt === 'number' &&
      Date.now() - lastInboundAt <= input.config.windowHours * 60 * 60 * 1000;
    if (!withinWindow) {
      await record(
        {
          customerId,
          channel: input.channel,
          to: address,
          content: input.content,
          dedupKey: input.dedupKey,
          status: 'blocked',
          reason: 'Outside 24h window — a template message is required',
        },
        input.proactiveStore,
      );
      return {
        ok: false,
        status: 'blocked',
        reason: 'Outside 24h window — a template message is required',
      };
    }
  }

  await enqueueJob(
    'outbound',
    {
      channel: input.channel,
      to: address,
      text: input.content,
      proactive: true,
      ...(input.templateName ? { templateName: input.templateName } : {}),
    },
    input.jobStore,
  );

  await record(
    {
      customerId,
      channel: input.channel,
      to: address,
      content: input.content,
      dedupKey: input.dedupKey,
      status: 'sent',
    },
    input.proactiveStore,
  );

  return { ok: true, status: 'sent' };
}
