import { randomUUID } from 'node:crypto';
import type { ApiValue } from '@aelio/db-client';
import { embed } from '../analyst/embeddings.js';
import { appendConversationRecord } from './conversations.js';
import { i64, readI64, readUtf8, utf8 } from './helpers.js';
import type {
  MessageStoreAppendInput,
  MessageStoreHistoryRow,
  AelioDbStorageConfig,
} from './types.js';

const L0_TIER = 0;
const HISTORY_SCAN_CAP = 500;
const RATE_SCAN_CAP = 5_000;

function text(value: string): ApiValue {
  return { type: 'utf8', value };
}

const warnedDims = new Set<string>();

export function normalizeEmbedding(vector: number[], dim: number): number[] {
  if (vector.length === dim) {
    return vector;
  }
  // A mismatch means the configured embeddings.output_dimension does not match
  // aelioDb.embed_dim — vectors are being silently reshaped, which degrades
  // similarity search. Surface it once instead of hiding it.
  const key = `${vector.length}->${dim}`;
  if (!warnedDims.has(key)) {
    warnedDims.add(key);
    console.warn(
      `[aelio] embedding dimension mismatch: provider returned ${vector.length}, table expects ${dim}. ` +
        'Align embeddings.output_dimension with aelioDb.embed_dim to avoid degraded semantic search.',
    );
  }
  if (vector.length > dim) {
    return vector.slice(0, dim);
  }
  return [...vector, ...new Array<number>(dim - vector.length).fill(0)];
}

export class ConvoxMessageStore {
  readonly client: AelioDbStorageConfig['client'];
  readonly table: string;
  readonly tables: AelioDbStorageConfig['tables'];
  readonly embedDim: number;

  constructor(config: AelioDbStorageConfig) {
    this.client = config.client;
    this.table = config.tables.messages;
    this.tables = config.tables;
    this.embedDim = config.embedDim;
  }

  async appendMessage(input: MessageStoreAppendInput): Promise<{ messageId: string }> {
    const messageId = randomUUID();
    const createdAt = Date.now();
    const content = input.content.trim();

    const values: Record<string, ApiValue> = {
      message_id: utf8(messageId),
      session_id: utf8(input.sessionId),
      customer_id: utf8(input.customerId),
      role: utf8(input.role),
      content: text(content),
      channel: utf8(input.channel),
      tier: i64(L0_TIER),
      created_at: i64(createdAt),
    };

    if (content.length > 0) {
      try {
        const embedding = normalizeEmbedding(
          await embed(content, { purpose: 'message_storage' }),
          this.embedDim,
        );
        values.embedding = { type: 'vector', value: embedding };
      } catch {
        // Embedding is optional for L0 writes; semantic recall degrades gracefully.
      }
    }

    await this.client.insertRow(this.table, values);
    await appendConversationRecord(
      {
        client: this.client,
        tables: this.tables,
        embedDim: this.embedDim,
      },
      {
        ...input,
        messageId,
        createdAt,
      },
    );
    return { messageId };
  }

  async loadHistory(sessionId: string, limit: number): Promise<MessageStoreHistoryRow[]> {
    const scan = await this.client.scanRows(this.table, {
      k: Math.max(limit * 4, HISTORY_SCAN_CAP),
      filters: [
        { col: 'session_id', op: 'eq', value: utf8(sessionId) },
        { col: 'tier', op: 'eq', value: i64(L0_TIER) },
      ],
    });

    const rows = scan.rows
      .map((row) => {
        const role = readUtf8(row.values, 'role');
        const content = readUtf8(row.values, 'content');
        const createdAt = readI64(row.values, 'created_at');
        return { role, content, createdAt };
      })
      .filter(
        (row) =>
          row.role === 'user' || row.role === 'assistant' || row.role === 'system',
      )
      .sort((a, b) => a.createdAt - b.createdAt)
      .slice(-limit);

    return rows.map((row) => ({
      role: row.role as MessageStoreHistoryRow['role'],
      content: row.content,
    }));
  }

  /** Count user messages for a customer since `sinceMs` (epoch ms). */
  async countUserMessages(customerId: string, sinceMs: number): Promise<number> {
    const scan = await this.client.scanRows(this.table, {
      k: RATE_SCAN_CAP,
      filters: [
        { col: 'customer_id', op: 'eq', value: utf8(customerId) },
        { col: 'role', op: 'eq', value: utf8('user') },
        { col: 'created_at', op: 'ge', value: i64(sinceMs) },
      ],
    });
    return scan.rows.length;
  }

  async countSessionMessages(sessionId: string): Promise<number> {
    const scan = await this.client.scanRows(this.table, {
      k: RATE_SCAN_CAP,
      filters: [
        { col: 'session_id', op: 'eq', value: utf8(sessionId) },
        { col: 'tier', op: 'eq', value: i64(L0_TIER) },
      ],
    });
    return scan.rows.length;
  }

  /** Most recent user-message timestamp for a customer, or null. */
  async lastUserMessageAt(customerId: string): Promise<number | null> {
    const scan = await this.client.scanRows(this.table, {
      k: RATE_SCAN_CAP,
      filters: [
        { col: 'customer_id', op: 'eq', value: utf8(customerId) },
        { col: 'role', op: 'eq', value: utf8('user') },
      ],
    });
    let latest = 0;
    for (const row of scan.rows) {
      const createdAt = readI64(row.values, 'created_at');
      if (createdAt > latest) {
        latest = createdAt;
      }
    }
    return latest > 0 ? latest : null;
  }

  async loadTranscript(
    sessionId: string,
    limit = 200,
  ): Promise<Array<{ role: string; content: string; createdAt: number }>> {
    const scan = await this.client.scanRows(this.table, {
      k: Math.max(limit * 2, HISTORY_SCAN_CAP),
      filters: [
        { col: 'session_id', op: 'eq', value: utf8(sessionId) },
        { col: 'tier', op: 'eq', value: i64(L0_TIER) },
      ],
    });

    return scan.rows
      .map((row) => ({
        role: readUtf8(row.values, 'role'),
        content: readUtf8(row.values, 'content'),
        createdAt: readI64(row.values, 'created_at'),
      }))
      .sort((a, b) => a.createdAt - b.createdAt)
      .slice(-limit);
  }
}

export function createConvoxMessageStore(config: AelioDbStorageConfig): ConvoxMessageStore {
  return new ConvoxMessageStore(config);
}