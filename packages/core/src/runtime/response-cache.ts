import type { AelioDatabase } from '@aelio/db';
import { responseCache } from '@aelio/db';
import { and, eq, gt } from 'drizzle-orm';
import { randomUUID } from 'node:crypto';
import { cosineSimilarity, embed, embedText } from '../analyst/embeddings.js';

export type ResponseCacheConfig = {
  enabled: boolean;
  similarityThreshold: number;
  ttlMinutes: number;
};

/**
 * Look for a cached reply to a near-identical earlier question from the SAME
 * customer that has not expired. Returns the reply on a hit (and bumps its hit
 * count), null otherwise. Only no-tool replies are ever stored (see storeCachedResponse),
 * so a hit never serves stale account data.
 */
export async function lookupCachedResponse(input: {
  database: AelioDatabase;
  customerId: string;
  message: string;
  threshold: number;
}): Promise<string | null> {
  const db = input.database.db;
  const rows = await db
    .select()
    .from(responseCache)
    .where(
      and(eq(responseCache.customerId, input.customerId), gt(responseCache.expiresAt, new Date())),
    );
  if (rows.length === 0) {
    return null;
  }

  const queryEmbedding = await embed(input.message, { purpose: 'response_cache_lookup' });
  let best: (typeof rows)[number] | null = null;
  let bestScore = 0;
  for (const row of rows) {
    const embedding = row.embedding ?? embedText(row.query);
    const score = cosineSimilarity(queryEmbedding, embedding);
    if (score > bestScore) {
      bestScore = score;
      best = row;
    }
  }

  if (best && bestScore >= input.threshold) {
    await db
      .update(responseCache)
      .set({ hits: (best.hits ?? 0) + 1 })
      .where(eq(responseCache.id, best.id));
    return best.reply;
  }
  return null;
}

/** Cache a no-tool reply for a customer with a TTL. */
export async function storeCachedResponse(input: {
  database: AelioDatabase;
  customerId: string;
  message: string;
  reply: string;
  ttlMinutes: number;
}): Promise<void> {
  const now = Date.now();
  await input.database.db.insert(responseCache).values({
    id: randomUUID(),
    customerId: input.customerId,
    query: input.message,
    embedding: await embed(input.message, { purpose: 'response_cache_store' }),
    reply: input.reply,
    hits: 0,
    createdAt: new Date(now),
    expiresAt: new Date(now + input.ttlMinutes * 60_000),
  });
}
