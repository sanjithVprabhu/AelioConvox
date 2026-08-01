import type { ConvoxMemoryStore } from '../storage/memories.js';

export type RecalledMemory = {
  id: string;
  content: string;
  score: number;
  category: string | null;
};

/**
 * Semantic memory recall — Aelio DB VSS only. SQLite is not used.
 */
export async function recallMemories(
  memoryStore: ConvoxMemoryStore | undefined,
  customerId: string,
  query: string,
  limit = 5,
  minScore = 0.05,
): Promise<RecalledMemory[]> {
  if (!memoryStore) {
    return [];
  }
  return memoryStore.recall(customerId, query, limit, minScore);
}
