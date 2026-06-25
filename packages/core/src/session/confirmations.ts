import type { AelioDatabase } from '@aelio/db';
import { sessions } from '@aelio/db';
import { eq } from 'drizzle-orm';
import type { PendingConfirmation } from '../safety/confirmations.js';

type SessionMetadata = {
  pendingConfirmation?: PendingConfirmation;
};

export async function getPendingConfirmation(
  db: AelioDatabase['db'],
  sessionId: string,
): Promise<PendingConfirmation | null> {
  const row = await db.select().from(sessions).where(eq(sessions.id, sessionId)).limit(1);
  const metadata = (row[0]?.metadata ?? {}) as SessionMetadata;
  return metadata.pendingConfirmation ?? null;
}

export async function setPendingConfirmation(
  db: AelioDatabase['db'],
  sessionId: string,
  pending: PendingConfirmation,
): Promise<void> {
  const row = await db.select().from(sessions).where(eq(sessions.id, sessionId)).limit(1);
  const metadata = (row[0]?.metadata ?? {}) as SessionMetadata;
  await db
    .update(sessions)
    .set({
      metadata: {
        ...metadata,
        pendingConfirmation: pending,
      },
    })
    .where(eq(sessions.id, sessionId));
}

export async function clearPendingConfirmation(
  db: AelioDatabase['db'],
  sessionId: string,
): Promise<void> {
  const row = await db.select().from(sessions).where(eq(sessions.id, sessionId)).limit(1);
  const metadata = (row[0]?.metadata ?? {}) as SessionMetadata;
  const { pendingConfirmation: _removed, ...rest } = metadata;
  await db
    .update(sessions)
    .set({ metadata: rest })
    .where(eq(sessions.id, sessionId));
}