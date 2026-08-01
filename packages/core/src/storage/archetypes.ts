import { randomUUID } from 'node:crypto';
import { cosineSimilarity, embed } from '../analyst/embeddings.js';
import { i64, readUtf8, readVector, utf8 } from './helpers.js';
import { normalizeEmbedding } from './messages.js';
import type { AelioDbStorageConfig } from './types.js';

export type ArchetypeValence = 'positive' | 'negative' | 'neutral';

export type ArchetypeExemplar = {
  category: string;
  valence: ArchetypeValence;
  /** Links this bucket to its aspect in the mother collection. '' for legacy rows. */
  aspectId?: string;
  keyword: string;
  /** What this bucket is. */
  description: string;
  /** How it is used / conveyed in language. */
  usage: string;
  /** How it is inferred from a message. */
  inference: string;
  /** How the assistant should respond when this bucket wins — fed to the prompt. */
  guidance: string;
};

export type ArchetypeMatch = ArchetypeExemplar & {
  id: string;
  aspectId: string;
  /** Recomputed cosine similarity (NOT the AelioDb RRF score). */
  score: number;
  /** AelioDb hybrid RRF score when BM25+vector was used; 0 for vector-only. */
  rrfScore: number;
};

/**
 * The archetype exemplar signature that gets embedded. Keyword first, then the
 * semantics of what/how so the vector captures the bucket's meaning, not just
 * the surface keyword.
 */
export function archetypeSignature(exemplar: ArchetypeExemplar): string {
  return [
    exemplar.keyword,
    exemplar.description,
    exemplar.usage,
    exemplar.inference,
  ]
    .filter(Boolean)
    .join(' — ');
}

export class ConvoxArchetypeStore {
  readonly client: AelioDbStorageConfig['client'];
  readonly table: string;
  readonly embedDim: number;
  readonly tenant: string;

  constructor(config: AelioDbStorageConfig, tenant = 'default') {
    this.client = config.client;
    this.table = config.tables.archetypes;
    this.embedDim = config.embedDim;
    this.tenant = tenant;
  }

  private async countForTenant(): Promise<number> {
    const scan = await this.client.scanRows(this.table, {
      k: 5_000,
      filters: [{ col: 'tenant', op: 'eq', value: utf8(this.tenant) }],
    });
    return scan.rows.length;
  }

  /** Replace this tenant's archetype rows with `exemplars`. */
  async replaceAll(exemplars: ArchetypeExemplar[]): Promise<number> {
    const existing = await this.client.scanRows(this.table, {
      k: 5_000,
      filters: [{ col: 'tenant', op: 'eq', value: utf8(this.tenant) }],
    });
    for (const row of existing.rows) {
      await this.client.deleteRow(this.table, row.row_id);
    }
    for (const exemplar of exemplars) {
      await this.insert(exemplar);
    }
    return exemplars.length;
  }

  /** Seed defaults only when this tenant has no archetypes yet. */
  async seedIfEmpty(exemplars: ArchetypeExemplar[]): Promise<number> {
    if ((await this.countForTenant()) > 0) {
      return 0;
    }
    for (const exemplar of exemplars) {
      await this.insert(exemplar);
    }
    return exemplars.length;
  }

  async insert(exemplar: ArchetypeExemplar): Promise<{ id: string }> {
    const id = randomUUID();
    const vector = normalizeEmbedding(
      await embed(archetypeSignature(exemplar), { purpose: 'pathway_retrieval' }),
      this.embedDim,
    );
    await this.client.insertRow(this.table, {
      archetype_id: utf8(id),
      tenant: utf8(this.tenant),
      aspect_id: utf8(exemplar.aspectId ?? ''),
      category: utf8(exemplar.category),
      valence: utf8(exemplar.valence),
      keyword: utf8(exemplar.keyword),
      description: utf8(exemplar.description),
      usage: utf8(exemplar.usage),
      inference: utf8(exemplar.inference),
      guidance: utf8(exemplar.guidance),
      embedding: { type: 'vector', value: vector },
      created_at: i64(Date.now()),
    });
    return { id };
  }

  /** Insert every valence bucket of one aspect, stamping its aspect_id. */
  async insertAspectBuckets(
    aspectId: string,
    exemplars: ArchetypeExemplar[],
  ): Promise<number> {
    for (const exemplar of exemplars) {
      await this.insert({ ...exemplar, aspectId });
    }
    return exemplars.length;
  }

  /**
   * Semantic search over archetype buckets using a precomputed message vector.
   * AelioDb's query score is RRF rank fusion, so we hydrate the candidate rows
   * and recompute cosine — the only score comparable across valence buckets.
   * Pass `text` to also run BM25 over `description` and fuse via RRF.
   */
  async matchByVector(
    queryVector: number[],
    k = 30,
    options?: { text?: string },
  ): Promise<ArchetypeMatch[]> {
    const vector = normalizeEmbedding(queryVector, this.embedDim);
    const response = await this.client.query(this.table, {
      k,
      vector: { col: 'embedding', query: vector },
      ...(options?.text
        ? { text: { col: 'description', query: options.text.slice(0, 500) } }
        : {}),
      filters: [{ col: 'tenant', op: 'eq', value: utf8(this.tenant) }],
    });
    if (response.results.length === 0) {
      return [];
    }

    const hydrated = await Promise.all(
      response.results.map(async (hit) => ({
        hit,
        row: await this.client.getRow(this.table, hit.row_id),
      })),
    );

    const matches: ArchetypeMatch[] = [];
    for (const { hit, row } of hydrated) {
      const stored = readVector(row.values, 'embedding');
      const score = stored ? cosineSimilarity(queryVector, stored) : 0;
      const valence = readUtf8(row.values, 'valence') as ArchetypeValence;
      matches.push({
        id: readUtf8(row.values, 'archetype_id') || String(hit.row_id),
        aspectId: readUtf8(row.values, 'aspect_id'),
        category: readUtf8(row.values, 'category'),
        valence,
        keyword: readUtf8(row.values, 'keyword'),
        description: readUtf8(row.values, 'description'),
        usage: readUtf8(row.values, 'usage'),
        inference: readUtf8(row.values, 'inference'),
        guidance: readUtf8(row.values, 'guidance'),
        score,
        rrfScore: hit.score ?? 0,
      });
    }
    return matches.sort((a, b) => b.score - a.score);
  }
}

export function createConvoxArchetypeStore(
  config: AelioDbStorageConfig,
  tenant = 'default',
): ConvoxArchetypeStore {
  return new ConvoxArchetypeStore(config, tenant);
}
