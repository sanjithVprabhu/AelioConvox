import { randomUUID } from 'node:crypto';
import { cosineSimilarity, embed } from '../analyst/embeddings.js';
import { i64, readI64, readUtf8, readVector, utf8 } from './helpers.js';
import { normalizeEmbedding } from './messages.js';
import type { AelioDbStorageConfig } from './types.js';

export type AspectStatus = 'candidate' | 'active' | 'retired';
export type AspectSource = 'builtin' | 'discovered';

export type AspectRecord = {
  id: string;
  name: string;
  description: string;
  status: AspectStatus;
  source: AspectSource;
  hits: number;
  createdAt: number;
  lastSeenAt: number;
};

export type AspectMatch = AspectRecord & {
  /** Recomputed cosine similarity (NOT the AelioDb RRF score). */
  score: number;
};

export type AspectRegisterInput = {
  name: string;
  description: string;
  status?: AspectStatus;
  source?: AspectSource;
  /** Precomputed embedding of the aspect signature; embedded here when omitted. */
  embedding?: number[];
};

function aspectSignature(name: string, description: string): string {
  return [name, description].filter(Boolean).join(' — ');
}

function rowToRecord(values: Record<string, import('@aelio/db-client').ApiValue>): AspectRecord {
  return {
    id: readUtf8(values, 'aspect_id'),
    name: readUtf8(values, 'name'),
    description: readUtf8(values, 'description'),
    status: (readUtf8(values, 'status') as AspectStatus) || 'candidate',
    source: (readUtf8(values, 'source') as AspectSource) || 'discovered',
    hits: readI64(values, 'hits'),
    createdAt: readI64(values, 'created_at'),
    lastSeenAt: readI64(values, 'last_seen_at'),
  };
}

export class ConvoxAspectStore {
  readonly client: AelioDbStorageConfig['client'];
  readonly table: string;
  readonly embedDim: number;
  readonly tenant: string;
  /** Candidate is promoted to active once hits reaches this. */
  readonly promoteAtHits: number;

  constructor(config: AelioDbStorageConfig, tenant = 'default', promoteAtHits = 3) {
    this.client = config.client;
    this.table = config.tables.aspects;
    this.embedDim = config.embedDim;
    this.tenant = tenant;
    this.promoteAtHits = promoteAtHits;
  }

  private tenantFilter() {
    return [{ col: 'tenant', op: 'eq' as const, value: utf8(this.tenant) }];
  }

  async count(): Promise<number> {
    const scan = await this.client.scanRows(this.table, { k: 5_000, filters: this.tenantFilter() });
    return scan.rows.length;
  }

  async list(status?: AspectStatus): Promise<AspectRecord[]> {
    const scan = await this.client.scanRows(this.table, { k: 5_000, filters: this.tenantFilter() });
    const records = scan.rows.map((row) => rowToRecord(row.values));
    return status ? records.filter((record) => record.status === status) : records;
  }

  async register(input: AspectRegisterInput): Promise<AspectRecord> {
    const id = randomUUID();
    const now = Date.now();
    const vector = normalizeEmbedding(
      input.embedding ??
        (await embed(aspectSignature(input.name, input.description), {
          purpose: 'pathway_retrieval',
        })),
      this.embedDim,
    );
    const record: AspectRecord = {
      id,
      name: input.name,
      description: input.description,
      status: input.status ?? 'candidate',
      source: input.source ?? 'discovered',
      hits: 0,
      createdAt: now,
      lastSeenAt: now,
    };
    await this.client.insertRow(this.table, {
      aspect_id: utf8(id),
      tenant: utf8(this.tenant),
      name: utf8(record.name),
      description: utf8(record.description),
      embedding: { type: 'vector', value: vector },
      status: utf8(record.status),
      source: utf8(record.source),
      hits: i64(0),
      created_at: i64(now),
      last_seen_at: i64(now),
    });
    return record;
  }

  /**
   * Which known aspects are present in this message vector, by recomputed
   * cosine. Retired aspects are excluded. Used both to scope the child bucket
   * search and to decide whether the message is already "covered".
   */
  async matchByVector(queryVector: number[], k = 20, minScore = 0.2): Promise<AspectMatch[]> {
    const vector = normalizeEmbedding(queryVector, this.embedDim);
    const response = await this.client.query(this.table, {
      k,
      vector: { col: 'embedding', query: vector },
      filters: this.tenantFilter(),
    });
    if (response.results.length === 0) return [];

    const hydrated = await Promise.all(
      response.results.map(async (hit) => ({
        hit,
        row: await this.client.getRow(this.table, hit.row_id),
      })),
    );

    const matches: AspectMatch[] = [];
    for (const { row } of hydrated) {
      const record = rowToRecord(row.values);
      if (record.status === 'retired') continue;
      const stored = readVector(row.values, 'embedding');
      const score = stored ? cosineSimilarity(queryVector, stored) : 0;
      if (score < minScore) continue;
      matches.push({ ...record, score });
    }
    return matches.sort((a, b) => b.score - a.score);
  }

  /** Nearest existing aspect for dedup; null when nothing clears `threshold`. */
  async findSimilar(queryVector: number[], threshold = 0.8): Promise<AspectMatch | null> {
    const [top] = await this.matchByVector(queryVector, 5, threshold);
    return top ?? null;
  }

  private async findRow(aspectId: string) {
    const scan = await this.client.scanRows(this.table, {
      k: 1,
      filters: [
        { col: 'tenant', op: 'eq', value: utf8(this.tenant) },
        { col: 'aspect_id', op: 'eq', value: utf8(aspectId) },
      ],
    });
    return scan.rows[0] ?? null;
  }

  /** Record an observation: bump hits/last_seen and auto-promote candidates. */
  async touch(aspectId: string): Promise<void> {
    const row = await this.findRow(aspectId);
    if (!row) return;
    const hits = readI64(row.values, 'hits') + 1;
    const status = readUtf8(row.values, 'status') as AspectStatus;
    const nextStatus: AspectStatus =
      status === 'candidate' && hits >= this.promoteAtHits ? 'active' : status;
    await this.client.updateRow(this.table, row.row_id, {
      hits: i64(hits),
      last_seen_at: i64(Date.now()),
      status: utf8(nextStatus),
    });
  }

  async setStatus(aspectId: string, status: AspectStatus): Promise<void> {
    const row = await this.findRow(aspectId);
    if (!row) return;
    await this.client.updateRow(this.table, row.row_id, { status: utf8(status) });
  }
}

export function createConvoxAspectStore(
  config: AelioDbStorageConfig,
  tenant = 'default',
): ConvoxAspectStore {
  return new ConvoxAspectStore(config, tenant);
}
