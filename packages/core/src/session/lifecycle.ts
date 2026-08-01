import type { Channel } from '@aelio/protocol';
import type { ConvoxCustomerStore } from '../storage/customers.js';
import type { ConvoxMessageStore } from '../storage/messages.js';
import type { ConvoxSessionStore } from '../storage/sessions.js';

export type SessionRecord = {
  id: string;
  customerId: string;
  channel: Channel;
};

export type HistoryMessage = {
  role: 'user' | 'assistant' | 'system';
  content: string;
};

/** Resolve (or create) the customer behind a channel address. AelioDb `customerStore` is required. */
export async function ensureCustomer(
  externalId: string,
  channel: Channel,
  channelAddress: string,
  customerStore: ConvoxCustomerStore,
): Promise<string> {
  if (!customerStore) {
    throw new Error('ensureCustomer requires a AelioDb customerStore');
  }
  return customerStore.ensureCustomer(externalId, channel, channelAddress);
}

export async function findOrCreateSession(
  customerId: string,
  channel: Channel,
  idleTimeoutMinutes = 60,
  sessionStore: ConvoxSessionStore,
): Promise<SessionRecord> {
  if (!sessionStore) {
    throw new Error('findOrCreateSession requires a AelioDb sessionStore');
  }
  const record = await sessionStore.findOrCreate(customerId, channel, idleTimeoutMinutes);
  return { id: record.id, customerId: record.customerId, channel: record.channel as Channel };
}

export async function appendMessage(
  input: {
    sessionId: string;
    customerId: string;
    role: 'user' | 'assistant' | 'system' | 'tool';
    content: string;
    channel: Channel;
    toolCall?: Record<string, unknown>;
    toolResult?: Record<string, unknown>;
  },
  messageStore: ConvoxMessageStore,
  sessionStore: ConvoxSessionStore,
): Promise<void> {
  if (!messageStore) {
    throw new Error('appendMessage requires a AelioDb messageStore');
  }
  await messageStore.appendMessage(input);
  await touchSessionActivity(input.sessionId, new Date(), sessionStore);
}

export async function touchSessionActivity(
  sessionId: string,
  at: Date = new Date(),
  sessionStore: ConvoxSessionStore,
): Promise<void> {
  if (!sessionStore) {
    throw new Error('touchSessionActivity requires a AelioDb sessionStore');
  }
  await sessionStore.touchActivity(sessionId, at.getTime());
}

export async function loadHistory(
  sessionId: string,
  limit: number,
  messageStore: ConvoxMessageStore,
): Promise<HistoryMessage[]> {
  if (!messageStore) {
    throw new Error('loadHistory requires a AelioDb messageStore');
  }
  return messageStore.loadHistory(sessionId, limit);
}
