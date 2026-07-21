import type { PendingConfirmation } from '../safety/confirmations.js';
import type { ConvoxSessionStore } from '../storage/sessions.js';

type SessionMetadata = {
  pendingConfirmation?: PendingConfirmation;
  [key: string]: unknown;
};

function requireSessionStore(sessionStore: ConvoxSessionStore | undefined): ConvoxSessionStore {
  if (!sessionStore) {
    throw new Error('Sunjet sessionStore is required');
  }
  return sessionStore;
}

export async function getPendingConfirmation(
  sessionId: string,
  sessionStore: ConvoxSessionStore,
): Promise<PendingConfirmation | null> {
  const store = requireSessionStore(sessionStore);
  const record = await store.get(sessionId);
  const metadata = (record?.metadata ?? {}) as SessionMetadata;
  return metadata.pendingConfirmation ?? null;
}

export async function setPendingConfirmation(
  sessionId: string,
  pending: PendingConfirmation,
  sessionStore: ConvoxSessionStore,
): Promise<void> {
  const store = requireSessionStore(sessionStore);
  const record = await store.get(sessionId);
  const metadata = (record?.metadata ?? {}) as SessionMetadata;
  await store.updateSummary(sessionId, record?.summary ?? '', {
    ...metadata,
    pendingConfirmation: pending,
  });
}

export async function clearPendingConfirmation(
  sessionId: string,
  sessionStore: ConvoxSessionStore,
): Promise<void> {
  const store = requireSessionStore(sessionStore);
  const record = await store.get(sessionId);
  const metadata = (record?.metadata ?? {}) as SessionMetadata;
  const { pendingConfirmation: _removed, ...rest } = metadata;
  await store.updateSummary(sessionId, record?.summary ?? '', rest);
}
