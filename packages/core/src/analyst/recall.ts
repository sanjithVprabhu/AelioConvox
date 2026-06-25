import type { AelioDatabase } from '@aelio/db';
import { memory } from '@aelio/db';
import { eq, inArray } from 'drizzle-orm';
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
  const queryEmbedding = await embed(query);
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

    const records = await database.db
      .select()
      .from(memory)
      .where(inArray(memory.id, memoryIds));

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
    .where(eq(memory.customerId, customerId));

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
