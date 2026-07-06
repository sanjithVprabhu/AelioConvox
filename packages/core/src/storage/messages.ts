import { randomUUID } from 'node:crypto';
import type { ApiValue } from '@aelio/sunjet-client';
import { embed } from '../analyst/embeddings.js';
import { appendConversationRecord } from './conversations.js';
import type {
  MessageStoreAppendInput,
  MessageStoreHistoryRow,
  SunjetStorageConfig,
} from './types.js';

const L0_TIER = 0;
const HISTORY_SCAN_CAP = 500;

function utf8(value: string): ApiValue {
  return { type: 'utf8', value };
}

function i64(value: number): ApiValue {
  return { type: 'i64', value };
}

function text(value: string): ApiValue {
  return { type: 'utf8', value };
}

function readUtf8(values: Record<string, ApiValue>, key: string): string {
  const entry = values[key];
  if (!entry) {
    return '';
  }
  if (entry.type === 'utf8') {
    return entry.value;
  }
  return '';
}

function readI64(values: Record<string, ApiValue>, key: string): number {
  const entry = values[key];
  if (!entry || entry.type !== 'i64') {
    return 0;
  }
  return entry.value;
}

const warnedDims = new Set<string>();

function normalizeEmbedding(vector: number[], dim: number): number[] {
  if (vector.length === dim) {
    return vector;
  }
  // A mismatch means the configured embeddings.output_dimension does not match
  // sunjet.embed_dim — vectors are being silently reshaped, which degrades
  // similarity search. Surface it once instead of hiding it.
  const key = `${vector.length}->${dim}`;
  if (!warnedDims.has(key)) {
    warnedDims.add(key);
    console.warn(
      `[aelio] embedding dimension mismatch: provider returned ${vector.length}, table expects ${dim}. ` +
        'Align embeddings.output_dimension with sunjet.embed_dim to avoid degraded semantic search.',
    );
  }
  if (vector.length > dim) {
    return vector.slice(0, dim);
  }
  return [...vector, ...new Array<number>(dim - vector.length).fill(0)];
}

export class ConvoxMessageStore {
  readonly client: SunjetStorageConfig['client'];
  readonly table: string;
  readonly tables: SunjetStorageConfig['tables'];
  readonly embedDim: number;
  readonly dualWriteSqlite: boolean;
  readonly fallbackSqliteOnError: boolean;

  constructor(config: SunjetStorageConfig) {
    this.client = config.client;
    this.table = config.tables.messages;
    this.tables = config.tables;
    this.embedDim = config.embedDim;
    this.dualWriteSqlite = config.dualWriteSqlite;
    this.fallbackSqliteOnError = config.fallbackSqliteOnError;
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
}

export function createConvoxMessageStore(config: SunjetStorageConfig): ConvoxMessageStore {
  return new ConvoxMessageStore(config);
}