import { randomUUID } from 'node:crypto';
import { cosineSimilarity, embed } from '../analyst/embeddings.js';
import { i64, readI64, readUtf8, readVector, utf8 } from './helpers.js';
import { normalizeEmbedding } from './messages.js';
import type { SunjetStorageConfig } from './types.js';

const CANDIDATE_K = 20;

export class ConvoxResponseCacheStore {
  readonly client: SunjetStorageConfig['client'];
  readonly table: string;
  readonly embedDim: number;

  constructor(config: SunjetStorageConfig) {
    this.client = config.client;
    this.table = config.tables.responseCache;
    this.embedDim = config.embedDim;
  }

  /**
   * Look for a cached reply to a near-identical earlier question from the
   * same customer that has not expired. Returns the reply on a hit (and
   * bumps its hit count), null otherwise.
   */
  async lookup(
    customerId: string,
    message: string,
    threshold = 0.85,
    embedDim: number = this.embedDim,
  ): Promise<string | null> {
    const now = Date.now();
    const queryEmbedding = await embed(message, { purpose: 'response_cache_lookup' });
    const vector = normalizeEmbedding(queryEmbedding, embedDim);

    const response = await this.client.query(this.table, {
      k: CANDIDATE_K,
      vector: { col: 'embedding', query: vector },
      filters: [{ col: 'customer_id', op: 'eq', value: utf8(customerId) }],
    });

    if (response.results.length === 0) {
      return null;
    }

    let bestScore = 0;
    let bestRowId = -1;
    let bestReply: string | null = null;

    for (const hit of response.results) {
      const row = await this.client.getRow(this.table, hit.row_id);
      const expiresAt = readI64(row.values, 'expires_at');
      if (expiresAt > 0 && expiresAt <= now) {
        continue;
      }

      const stored = readVector(row.values, 'embedding');
      const score = stored ? cosineSimilarity(queryEmbedding, stored) : hit.score;
      if (score > bestScore) {
        bestScore = score;
        bestRowId = hit.row_id;
        bestReply = readUtf8(row.values, 'reply');
      }
    }

    if (bestReply && bestScore >= threshold && bestRowId >= 0) {
      const row = await this.client.getRow(this.table, bestRowId);
      const hits = readI64(row.values, 'hits') + 1;
      await this.client.updateRow(this.table, bestRowId, { hits: i64(hits) });
      return bestReply;
    }

    return null;
  }

  /** Cache a no-tool reply for a customer with a TTL. */
  async store(
    customerId: string,
    message: string,
    reply: string,
    ttlMinutes: number,
    embedDim: number = this.embedDim,
  ): Promise<void> {
    const now = Date.now();
    const embedding = normalizeEmbedding(
      await embed(message, { purpose: 'response_cache_store' }),
      embedDim,
    );

    await this.client.insertRow(this.table, {
      cache_id: utf8(randomUUID()),
      customer_id: utf8(customerId),
      query: utf8(message),
      embedding: { type: 'vector', value: embedding },
      reply: utf8(reply),
      hits: i64(0),
      created_at: i64(now),
      expires_at: i64(now + ttlMinutes * 60_000),
    });
  }
}

export function createConvoxResponseCacheStore(
  config: SunjetStorageConfig,
): ConvoxResponseCacheStore {
  return new ConvoxResponseCacheStore(config);
}
