import { randomUUID } from 'node:crypto';
import type { ApiValue, RowValues, SunjetClient } from '@aelio/sunjet-client';
import type { AelioStorageTableNames } from '../storage/types.js';
import { embed } from '../analyst/embeddings.js';
import { normalizeEmbedding } from '../storage/messages.js';
import type { RecalledMemory } from '../analyst/recall.js';

const utf8 = (value: string): ApiValue => ({ type: 'utf8', value });
const i64 = (value: number): ApiValue => ({ type: 'i64', value });
const f64 = (value: number): ApiValue => ({ type: 'f64', value });

const DEDUPE_SCAN_CAP = 500;

export type AelioMemoryRecord = {
  id: string;
  subjectId: string;
  content: string;
  category: string;
  confidence: number;
  createdAt: number;
  expiresAt: number;
};

/**
 * Durable subject memory on Aelio DB. This is the runtime's replacement for the SQLite-backed
 * analyst recall path: the same `memories` table, read through Aelio DB's vector index, with no
 * Drizzle dependency anywhere on a runtime turn.
 *
 * Writes are deduplicated on exact content per subject, so a fact restated across many turns
 * stays one row instead of drowning recall in copies of itself.
 */
export class AelioMemoryStore {
  constructor(
    private readonly client: SunjetClient,
    private readonly tables: AelioStorageTableNames,
    private readonly embedDim: number,
  ) {}

  async recall(subjectId: string, query: string, limit = 5): Promise<RecalledMemory[]> {
    const trimmed = query.trim();
    if (!trimmed || limit <= 0) return [];
    let vector: number[];
    try {
      vector = normalizeEmbedding(await embed(trimmed, { purpose: 'memory_recall' }), this.embedDim);
    } catch {
      // Recall is an enhancement, never a turn dependency: an embedding outage must not stop the
      // subject from getting an answer.
      return [];
    }
    const now = Date.now();
    const rows = await this.client.scanRows(this.tables.memories, {
      k: Math.min(Math.max(limit, 1), 64),
      vector: { col: 'embedding', query: vector },
      filters: [{ col: 'customer_id', op: 'eq', value: utf8(subjectId) }],
    });
    return rows.rows
      .map((row) => ({
        id: readUtf8(row.values, 'memory_id'),
        content: readUtf8(row.values, 'content'),
        category: readUtf8Default(row.values, 'category') || null,
        expiresAt: readI64Default(row.values, 'expires_at', 0),
      }))
      .filter((entry) => entry.expiresAt === 0 || entry.expiresAt > now)
      // Aelio DB returns top-k in rank order; map rank to a monotonically decreasing score so
      // downstream prompt composition keeps the same ordering contract as SQLite recall.
      .map((entry, index, all) => ({
        id: entry.id,
        content: entry.content,
        category: entry.category,
        score: (all.length - index) / all.length,
      }))
      .slice(0, limit);
  }

  async remember(input: {
    subjectId: string;
    content: string;
    category?: string;
    confidence?: number;
    ttlMs?: number;
  }): Promise<string | null> {
    const content = input.content.trim();
    if (!content) return null;
    // Dedupe in the client over a bounded per-subject scan: `content` is a full-text column, and
    // equality filtering a tokenized column is not a contract Aelio DB guarantees.
    const existing = await this.client.scanRows(this.tables.memories, {
      k: DEDUPE_SCAN_CAP,
      filters: [{ col: 'customer_id', op: 'eq', value: utf8(input.subjectId) }],
    });
    if (existing.rows.some((row) => readUtf8Default(row.values, 'content') === content)) return null;

    const now = Date.now();
    const values: RowValues = {
      memory_id: utf8(randomUUID()),
      customer_id: utf8(input.subjectId),
      content: utf8(content),
      category: utf8(input.category ?? 'fact'),
      source_session_id: utf8(''),
      confidence: f64(input.confidence ?? 0.8),
      created_at: i64(now),
      expires_at: i64(input.ttlMs ? now + input.ttlMs : 0),
    };
    try {
      values.embedding = {
        type: 'vector',
        value: normalizeEmbedding(await embed(content, { purpose: 'memory_extract' }), this.embedDim),
      };
    } catch {
      // Store the fact without a vector; it is still readable by exact filter and re-embeddable.
    }
    const inserted = await this.client.insertRow(this.tables.memories, values);
    return String(inserted.row_id);
  }
}

function readUtf8(values: RowValues, key: string): string {
  const value = values[key];
  if (!value || value.type !== 'utf8') throw new Error(`memory row is missing utf8 ${key}`);
  return value.value;
}
function readUtf8Default(values: RowValues, key: string): string {
  const value = values[key];
  return value?.type === 'utf8' ? value.value : '';
}
function readI64Default(values: RowValues, key: string, fallback: number): number {
  const value = values[key];
  return value?.type === 'i64' ? value.value : fallback;
}
