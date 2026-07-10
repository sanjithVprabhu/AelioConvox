import type { AelioDatabase } from '@aelio/db';
import { channelAddresses, customers, messages, sessions } from '@aelio/db';
import type { Channel } from '@aelio/protocol';
import { and, desc, eq, lt } from 'drizzle-orm';
import { randomUUID } from 'node:crypto';

export type SessionRecord = {
  id: string;
  customerId: string;
  channel: Channel;
};

export type HistoryMessage = {
  role: 'user' | 'assistant' | 'system';
  content: string;
};

export async function ensureCustomer(
  db: AelioDatabase['db'],
  externalId: string,
  channel: Channel,
  channelAddress: string,
): Promise<string> {
  const existingAddress = await db
    .select()
    .from(channelAddresses)
    .where(and(eq(channelAddresses.channel, channel), eq(channelAddresses.address, channelAddress)))
    .limit(1);

  if (existingAddress[0]) {
    return existingAddress[0].customerId;
  }

  const existingCustomer = await db
    .select()
    .from(customers)
    .where(eq(customers.externalId, externalId))
    .limit(1);

  const now = new Date();
  const customerId = existingCustomer[0]?.id ?? randomUUID();

  if (!existingCustomer[0]) {
    try {
      await db.insert(customers).values({
        id: customerId,
        externalId,
        displayName: externalId,
        createdAt: now,
        updatedAt: now,
      });
    } catch {
      const raced = await db
        .select()
        .from(customers)
        .where(eq(customers.externalId, externalId))
        .limit(1);
      if (!raced[0]) {
        throw new Error(`Failed to create customer for externalId "${externalId}"`);
      }
      return ensureChannelAddress(db, raced[0].id, channel, channelAddress, now);
    }
  }

  return ensureChannelAddress(db, customerId, channel, channelAddress, now);
}

async function ensureChannelAddress(
  db: AelioDatabase['db'],
  customerId: string,
  channel: Channel,
  channelAddress: string,
  now: Date,
): Promise<string> {
  const existing = await db
    .select()
    .from(channelAddresses)
    .where(and(eq(channelAddresses.channel, channel), eq(channelAddresses.address, channelAddress)))
    .limit(1);
  if (existing[0]) {
    return existing[0].customerId;
  }

  try {
    await db.insert(channelAddresses).values({
      id: randomUUID(),
      customerId,
      channel,
      address: channelAddress,
      verifiedAt: now,
      createdAt: now,
    });
  } catch {
    const raced = await db
      .select()
      .from(channelAddresses)
      .where(and(eq(channelAddresses.channel, channel), eq(channelAddresses.address, channelAddress)))
      .limit(1);
    if (!raced[0]) {
      throw new Error(`Failed to create channel address "${channelAddress}"`);
    }
    return raced[0].customerId;
  }

  return customerId;
}

export async function findOrCreateSession(
  db: AelioDatabase['db'],
  customerId: string,
  channel: Channel,
  idleTimeoutMinutes = 60,
): Promise<SessionRecord> {
  const cutoff = new Date(Date.now() - idleTimeoutMinutes * 60_000);

  await db
    .update(sessions)
    .set({ status: 'closed', closedAt: new Date() })
    .where(
      and(
        eq(sessions.customerId, customerId),
        eq(sessions.channel, channel),
        eq(sessions.status, 'active'),
        lt(sessions.lastActivityAt, cutoff),
      ),
    );

  const active = await db
    .select()
    .from(sessions)
    .where(
      and(
        eq(sessions.customerId, customerId),
        eq(sessions.channel, channel),
        eq(sessions.status, 'active'),
      ),
    )
    .orderBy(desc(sessions.lastActivityAt))
    .limit(1);

  if (active[0]) {
    return {
      id: active[0].id,
      customerId: active[0].customerId,
      channel: active[0].channel as Channel,
    };
  }

  const now = new Date();
  const sessionId = randomUUID();
  await db.insert(sessions).values({
    id: sessionId,
    customerId,
    channel,
    status: 'active',
    startedAt: now,
    lastActivityAt: now,
  });

  return { id: sessionId, customerId, channel };
}

export async function appendMessage(
  db: AelioDatabase['db'],
  input: {
    sessionId: string;
    customerId: string;
    role: 'user' | 'assistant' | 'system' | 'tool';
    content: string;
    channel: Channel;
    toolCall?: Record<string, unknown>;
    toolResult?: Record<string, unknown>;
  },
): Promise<void> {
  const now = new Date();
  await db.insert(messages).values({
    id: randomUUID(),
    sessionId: input.sessionId,
    customerId: input.customerId,
    role: input.role,
    content: input.content,
    toolCall: input.toolCall,
    toolResult: input.toolResult,
    channel: input.channel,
    createdAt: now,
  });

  await touchSessionActivity(db, input.sessionId, now);
}

export async function touchSessionActivity(
  db: AelioDatabase['db'],
  sessionId: string,
  at: Date = new Date(),
): Promise<void> {
  await db
    .update(sessions)
    .set({ lastActivityAt: at })
    .where(eq(sessions.id, sessionId));
}

export async function loadHistory(
  db: AelioDatabase['db'],
  sessionId: string,
  limit: number,
): Promise<HistoryMessage[]> {
  const rows = await db
    .select()
    .from(messages)
    .where(eq(messages.sessionId, sessionId))
    .orderBy(desc(messages.createdAt))
    .limit(limit);

  return rows
    .reverse()
    .filter((row) => row.role === 'user' || row.role === 'assistant' || row.role === 'system')
    .map((row) => ({
      role: row.role as HistoryMessage['role'],
      content: row.content ?? '',
    }));
}
