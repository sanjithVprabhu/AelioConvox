/**
 * Harness Axis store — user-specific occurrence chains per aspect (atom).
 *
 * Unified AelioDb table `convox_axis_nodes` holds:
 *   - root rows: one per (tenant, customer, aspect)
 *   - occurrence rows: each fed stance sighting, linked via `previous` edges
 *
 * Write path: recordOccurrence(s) after a turn's stance assessment.
 * Read path: traverseAxis(customer, aspect, temporalScope) walks head→previous.
 */

import { createHash } from 'node:crypto';
import { cosineSimilarity } from '../analyst/embeddings.js';
import { inTemporalScope, type TemporalScope } from '../temporal/index.js';
import { f64, i64, readF64, readI64, readUtf8, readVector, utf8 } from './helpers.js';
import { normalizeEmbedding } from './messages.js';
import type { AelioDbStorageConfig } from './types.js';
import type { ArchetypeValence } from './archetypes.js';

export type AxisNodeType = 'root' | 'occurrence';

export type AxisOccurrenceWrite = {
  customerId: string;
  aspectId: string;
  aspectName: string;
  /** Deterministic turn+span identity pieces for idempotent IDs. */
  turnId: string;
  spanIndex: number;
  messageId?: string;
  sessionId?: string;
  span?: string;
  valence: ArchetypeValence;
  positive: number;
  negative: number;
  neutral: number;
  strength: number;
  intentLabel?: string;
  flowId?: string;
  embedding: number[];
  createdAt?: number;
};

export type AxisOccurrence = {
  occurrenceId: string;
  rowId: number;
  customerId: string;
  aspectId: string;
  aspectName: string;
  turnId: string;
  messageId: string;
  sessionId: string;
  span: string;
  valence: ArchetypeValence;
  positive: number;
  negative: number;
  neutral: number;
  strength: number;
  intentLabel: string;
  flowId: string;
  createdAt: number;
  /** Cosine to the query vector when traversal was semantic-ranked. */
  score: number;
};

function occurrenceId(input: {
  tenant: string;
  turnId: string;
  spanIndex: number;
  aspectId: string;
}): string {
  return createHash('sha256')
    .update(`${input.tenant}|${input.turnId}|${input.spanIndex}|${input.aspectId}`)
    .digest('hex')
    .slice(0, 32);
}

function edges(rowIds: number[]): { type: 'edges'; value: number[] } {
  return { type: 'edges', value: rowIds };
}

function readEdges(values: Record<string, import('@aelio/db-client').ApiValue>, key: string): number[] {
  const entry = values[key];
  if (entry?.type === 'edges') return entry.value;
  return [];
}

export class ConvoxAxisStore {
  readonly client: AelioDbStorageConfig['client'];
  readonly table: string;
  readonly embedDim: number;
  readonly tenant: string;

  constructor(config: AelioDbStorageConfig, tenant = 'default') {
    this.client = config.client;
    this.table = config.tables.axisNodes;
    this.embedDim = config.embedDim;
    this.tenant = tenant;
  }

  private tenantFilter() {
    return [{ col: 'tenant', op: 'eq' as const, value: utf8(this.tenant) }];
  }

  private async findRoot(customerId: string, aspectId: string) {
    const scan = await this.client.scanRows(this.table, {
      k: 5,
      filters: [
        ...this.tenantFilter(),
        { col: 'node_type', op: 'eq', value: utf8('root') },
        { col: 'customer_id', op: 'eq', value: utf8(customerId) },
        { col: 'aspect_id', op: 'eq', value: utf8(aspectId) },
      ],
    });
    return scan.rows[0] ?? null;
  }

  private async findOccurrenceById(id: string) {
    const scan = await this.client.scanRows(this.table, {
      k: 1,
      filters: [
        ...this.tenantFilter(),
        { col: 'occurrence_id', op: 'eq', value: utf8(id) },
      ],
    });
    return scan.rows[0] ?? null;
  }

  /**
   * Idempotently record a stance occurrence and advance the axis head.
   * Returns null when the occurrence already existed (retry-safe).
   */
  async recordOccurrence(input: AxisOccurrenceWrite): Promise<AxisOccurrence | null> {
    const id = occurrenceId({
      tenant: this.tenant,
      turnId: input.turnId,
      spanIndex: input.spanIndex,
      aspectId: input.aspectId,
    });
    if (await this.findOccurrenceById(id)) {
      return null;
    }

    const now = input.createdAt ?? Date.now();
    let root = await this.findRoot(input.customerId, input.aspectId);
    let previousRowIds: number[] = [];
    if (root) {
      previousRowIds = readEdges(root.values, 'head');
    } else {
      const created = await this.client.insertRow(this.table, {
        node_id: utf8(`root:${input.customerId}:${input.aspectId}`),
        tenant: utf8(this.tenant),
        node_type: utf8('root'),
        customer_id: utf8(input.customerId),
        aspect_id: utf8(input.aspectId),
        aspect_name: utf8(input.aspectName),
        occurrence_id: utf8(''),
        turn_id: utf8(''),
        message_id: utf8(''),
        session_id: utf8(''),
        span: utf8(''),
        valence: utf8(''),
        positive: f64(0),
        negative: f64(0),
        neutral: f64(0),
        strength: f64(0),
        intent_label: utf8(''),
        flow_id: utf8(''),
        embedding: { type: 'vector', value: new Array(this.embedDim).fill(0) },
        created_at: i64(now),
        previous: edges([]),
        head: edges([]),
      });
      root = await this.client.getRow(this.table, created.row_id);
    }

    const vector = normalizeEmbedding(input.embedding, this.embedDim);
    const inserted = await this.client.insertRow(this.table, {
      node_id: utf8(`occ:${id}`),
      tenant: utf8(this.tenant),
      node_type: utf8('occurrence'),
      customer_id: utf8(input.customerId),
      aspect_id: utf8(input.aspectId),
      aspect_name: utf8(input.aspectName),
      occurrence_id: utf8(id),
      turn_id: utf8(input.turnId),
      message_id: utf8(input.messageId ?? ''),
      session_id: utf8(input.sessionId ?? ''),
      span: utf8(input.span ?? ''),
      valence: utf8(input.valence),
      positive: f64(input.positive),
      negative: f64(input.negative),
      neutral: f64(input.neutral),
      strength: f64(input.strength),
      intent_label: utf8(input.intentLabel ?? ''),
      flow_id: utf8(input.flowId ?? ''),
      embedding: { type: 'vector', value: vector },
      created_at: i64(now),
      previous: edges(previousRowIds),
      head: edges([]),
    });

    if (root) {
      await this.client.updateRow(this.table, root.row_id, {
        head: edges([inserted.row_id]),
        aspect_name: utf8(input.aspectName),
        created_at: i64(now),
      });
    }

    return {
      occurrenceId: id,
      rowId: inserted.row_id,
      customerId: input.customerId,
      aspectId: input.aspectId,
      aspectName: input.aspectName,
      turnId: input.turnId,
      messageId: input.messageId ?? '',
      sessionId: input.sessionId ?? '',
      span: input.span ?? '',
      valence: input.valence,
      positive: input.positive,
      negative: input.negative,
      neutral: input.neutral,
      strength: input.strength,
      intentLabel: input.intentLabel ?? '',
      flowId: input.flowId ?? '',
      createdAt: now,
      score: 1,
    };
  }

  private rowToOccurrence(
    rowId: number,
    values: Record<string, import('@aelio/db-client').ApiValue>,
    score = 0,
  ): AxisOccurrence | null {
    if (readUtf8(values, 'node_type') !== 'occurrence') return null;
    return {
      occurrenceId: readUtf8(values, 'occurrence_id'),
      rowId,
      customerId: readUtf8(values, 'customer_id'),
      aspectId: readUtf8(values, 'aspect_id'),
      aspectName: readUtf8(values, 'aspect_name'),
      turnId: readUtf8(values, 'turn_id'),
      messageId: readUtf8(values, 'message_id'),
      sessionId: readUtf8(values, 'session_id'),
      span: readUtf8(values, 'span'),
      valence: (readUtf8(values, 'valence') as ArchetypeValence) || 'neutral',
      positive: readF64(values, 'positive'),
      negative: readF64(values, 'negative'),
      neutral: readF64(values, 'neutral'),
      strength: readF64(values, 'strength'),
      intentLabel: readUtf8(values, 'intent_label'),
      flowId: readUtf8(values, 'flow_id'),
      createdAt: readI64(values, 'created_at'),
      score,
    };
  }

  /**
   * Walk the customer/aspect axis from head → previous within the temporal
   * window. Optional queryVector re-ranks by cosine against stored embeddings.
   */
  async traverseAxis(input: {
    customerId: string;
    aspectId: string;
    scope: TemporalScope;
    queryVector?: number[];
    maxDepth?: number;
  }): Promise<AxisOccurrence[]> {
    const root = await this.findRoot(input.customerId, input.aspectId);
    if (!root) return [];

    const headIds = readEdges(root.values, 'head');
    if (headIds.length === 0) return [];

    const maxDepth = input.maxDepth ?? 12;
    const out: AxisOccurrence[] = [];
    let frontier = headIds;
    const seen = new Set<number>();

    for (let depth = 0; depth < maxDepth && frontier.length > 0; depth += 1) {
      const next: number[] = [];
      for (const rowId of frontier) {
        if (seen.has(rowId)) continue;
        seen.add(rowId);
        const row = await this.client.getRow(this.table, rowId);
        const occ = this.rowToOccurrence(rowId, row.values);
        if (!occ) continue;
        if (!inTemporalScope(occ.createdAt, input.scope)) {
          // Chain is newest-first; once we leave the window, stop this branch.
          continue;
        }
        let score = occ.strength;
        if (input.queryVector) {
          const stored = readVector(row.values, 'embedding');
          score = stored ? cosineSimilarity(input.queryVector, stored) : 0;
        }
        out.push({ ...occ, score });
        next.push(...readEdges(row.values, 'previous'));
      }
      frontier = next;
    }

    return out.sort((a, b) => b.createdAt - a.createdAt);
  }

  /** List active aspect axes for a customer (roots that have a head). */
  async listAxes(customerId: string): Promise<Array<{ aspectId: string; aspectName: string }>> {
    const scan = await this.client.scanRows(this.table, {
      k: 500,
      filters: [
        ...this.tenantFilter(),
        { col: 'node_type', op: 'eq', value: utf8('root') },
        { col: 'customer_id', op: 'eq', value: utf8(customerId) },
      ],
    });
    return scan.rows
      .filter((row) => readEdges(row.values, 'head').length > 0)
      .map((row) => ({
        aspectId: readUtf8(row.values, 'aspect_id'),
        aspectName: readUtf8(row.values, 'aspect_name'),
      }));
  }
}

export function createConvoxAxisStore(
  config: AelioDbStorageConfig,
  tenant = 'default',
): ConvoxAxisStore {
  return new ConvoxAxisStore(config, tenant);
}
