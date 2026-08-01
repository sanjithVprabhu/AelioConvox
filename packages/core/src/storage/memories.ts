import { randomUUID } from 'node:crypto';
import { cosineSimilarity, embed } from '../analyst/embeddings.js';
import { f64, i64, readI64, readUtf8, readVector, utf8 } from './helpers.js';
import { normalizeEmbedding } from './messages.js';
import type { AelioDbStorageConfig } from './types.js';

const DEDUP_SCAN_CAP = 2_000;

export type MemoryRecallHit = {
  id: string;
  content: string;
  score: number;
  category: string | null;
};

export type MemoryStoreWriteInput = {
  customerId: string;
  content: string;
  category: string | null;
  sourceSessionId: string;
  confidence?: number;
  embedding: number[];
  expiresAtMs?: number | null;
  /** Skip insert when an identical fact already exists for this customer. */
  dedupe?: boolean;
};

export class ConvoxMemoryStore {
  readonly client: AelioDbStorageConfig['client'];
  readonly table: string;
  readonly embedDim: number;

  constructor(config: AelioDbStorageConfig) {
    this.client = config.client;
    this.table = config.tables.memories;
    this.embedDim = config.embedDim;
  }

  async hasContent(customerId: string, content: string): Promise<boolean> {
    const scan = await this.client.scanRows(this.table, {
      k: DEDUP_SCAN_CAP,
      filters: [{ col: 'customer_id', op: 'eq', value: utf8(customerId) }],
    });
    return scan.rows.some((row) => readUtf8(row.values, 'content') === content);
  }

  async store(input: MemoryStoreWriteInput): Promise<{ memoryId: string } | null> {
    const content = input.content.trim();
    if (!content) {
      return null;
    }

    if (input.dedupe !== false && (await this.hasContent(input.customerId, content))) {
      return null;
    }

    const memoryId = randomUUID();
    const createdAt = Date.now();
    const embedding = normalizeEmbedding(input.embedding, this.embedDim);

    await this.client.insertRow(this.table, {
      memory_id: utf8(memoryId),
      customer_id: utf8(input.customerId),
      content: utf8(content),
      embedding: { type: 'vector', value: embedding },
      category: utf8(input.category ?? ''),
      source_session_id: utf8(input.sourceSessionId),
      confidence: f64(input.confidence ?? 1),
      created_at: i64(createdAt),
      expires_at: i64(input.expiresAtMs ?? 0),
    });

    return { memoryId };
  }

  /**
   * Semantic recall via Aelio database vector search (VSS). Scores are cosine
   * similarity against the query embedding so callers keep a stable threshold.
   */
  async recall(
    customerId: string,
    query: string,
    limit = 5,
    minScore = 0.05,
  ): Promise<MemoryRecallHit[]> {
    const queryEmbedding = await embed(query, { purpose: 'memory_recall' });
    return this.recallByVector(customerId, queryEmbedding, limit, minScore);
  }

  /**
   * Recall using a vector already generated for this turn. The Semantic
   * Pathway Engine uses this to fan one message embedding out to every
   * retrieval target without another provider call.
   */
  async recallByVector(
    customerId: string,
    queryEmbedding: number[],
    limit = 5,
    minScore = 0.05,
  ): Promise<MemoryRecallHit[]> {
    const vector = normalizeEmbedding(queryEmbedding, this.embedDim);

    const response = await this.client.query(this.table, {
      k: Math.max(limit * 4, 20),
      vector: { col: 'embedding', query: vector },
      filters: [{ col: 'customer_id', op: 'eq', value: utf8(customerId) }],
    });

    if (response.results.length === 0) {
      return [];
    }

    const now = Date.now();
    const scored: MemoryRecallHit[] = [];
    // Query results contain row ids only. Hydrate candidates concurrently so
    // top-K recall pays one network round trip wave instead of N serial RTTs.
    const hydrated = await Promise.all(
      response.results.map(async (hit) => ({
        hit,
        row: await this.client.getRow(this.table, hit.row_id),
      })),
    );

    for (const { hit, row } of hydrated) {
      const expiresAt = readI64(row.values, 'expires_at');
      if (expiresAt > 0 && expiresAt <= now) {
        continue;
      }

      const content = readUtf8(row.values, 'content');
      if (!content) {
        continue;
      }

      const stored = readVector(row.values, 'embedding');
      const score = stored ? cosineSimilarity(queryEmbedding, stored) : 0;
      if (score < minScore) {
        continue;
      }

      const category = readUtf8(row.values, 'category');
      scored.push({
        id: readUtf8(row.values, 'memory_id') || String(hit.row_id),
        content,
        category: category.length > 0 ? category : null,
        score,
      });
    }

    return scored.sort((a, b) => b.score - a.score).slice(0, limit);
  }
}

export function createConvoxMemoryStore(config: AelioDbStorageConfig): ConvoxMemoryStore {
  return new ConvoxMemoryStore(config);
}
