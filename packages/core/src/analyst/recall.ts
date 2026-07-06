import type { AelioDatabase } from '@aelio/db';
import { memory } from '@aelio/db';
import { and, eq, gt, inArray, isNull, or } from 'drizzle-orm';
import { cosineSimilarity, embed, embedText } from './embeddings.js';

export type RecalledMemory = {
  id: string;
  content: string;
  score: number;
  category: string | null;
};

export async function recallMemories(
  database: AelioDatabase,
  customerId: string,
  query: string,
  limit = 5,
  minScore = 0.05,
): Promise<RecalledMemory[]> {
  const queryEmbedding = await embed(query, { purpose: 'memory_recall' });
  if (database.vectorEnabled) {
    const vectorRows = database.searchMemoryVectors({
      customerId,
      embedding: queryEmbedding,
      limit,
    });

    const memoryIds = vectorRows.map((row) => row.memoryId);
    if (memoryIds.length === 0) {
      return [];
    }

    const now = new Date();
    const records = await database.db
      .select()
      .from(memory)
      .where(
        and(
          inArray(memory.id, memoryIds),
          or(isNull(memory.expiresAt), gt(memory.expiresAt, now)),
        ),
      );

    const recordMap = new Map(records.map((row) => [row.id, row]));

    return vectorRows
      .map((row) => {
        const record = recordMap.get(row.memoryId);
        if (!record) {
          return null;
        }
        return {
          id: record.id,
          content: record.content,
          category: record.category,
          score: 1 / (1 + row.distance),
        };
      })
      .filter((entry): entry is RecalledMemory => {
        if (!entry) {
          return false;
        }
        return entry.score >= minScore;
      });
  }

  const rows = await database.db
    .select()
    .from(memory)
    .where(
      and(
        eq(memory.customerId, customerId),
        or(isNull(memory.expiresAt), gt(memory.expiresAt, new Date())),
      ),
    );

  const scored = rows
    .map((row) => {
      const embedding = row.embedding ?? embedText(row.content);
      return {
        id: row.id,
        content: row.content,
        category: row.category,
        score: cosineSimilarity(queryEmbedding, embedding),
      };
    })
    .filter((entry) => entry.score >= minScore)
    .sort((a, b) => b.score - a.score)
    .slice(0, limit);

  return scored;
}
