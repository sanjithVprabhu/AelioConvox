import { randomUUID } from 'node:crypto';
import type { RowValueResponse } from '@aelio/db-client';
import { i64, parseJson, readI64, readUtf8, utf8 } from './helpers.js';
import type { AelioDbStorageConfig } from './types.js';

const SCAN_CAP = 2_000;

export type SessionRecord = {
  id: string;
  customerId: string;
  channel: string;
  status: string;
  summary: string | null;
  metadata: Record<string, unknown>;
  lastActivityAt: number;
  closedAt: number | null;
};

export type ClosedSessionRecord = {
  sessionId: string;
  customerId: string;
  closedAt: number;
};

export class ConvoxSessionStore {
  readonly client: AelioDbStorageConfig['client'];
  readonly table: string;

  constructor(config: AelioDbStorageConfig) {
    this.client = config.client;
    this.table = config.tables.sessions;
  }

  private toRecord(row: RowValueResponse): SessionRecord {
    const summary = readUtf8(row.values, 'summary');
    return {
      id: readUtf8(row.values, 'session_id'),
      customerId: readUtf8(row.values, 'customer_id'),
      channel: readUtf8(row.values, 'channel'),
      status: readUtf8(row.values, 'status'),
      summary: summary.length > 0 ? summary : null,
      metadata: parseJson(readUtf8(row.values, 'metadata'), {}),
      lastActivityAt: readI64(row.values, 'last_activity_at'),
      closedAt: readI64(row.values, 'closed_at') || null,
    };
  }

  private async findRow(sessionId: string): Promise<RowValueResponse | null> {
    const scan = await this.client.scanRows(this.table, {
      k: 1,
      filters: [{ col: 'session_id', op: 'eq', value: utf8(sessionId) }],
    });
    return scan.rows[0] ?? null;
  }

  async get(sessionId: string): Promise<SessionRecord | null> {
    const row = await this.findRow(sessionId);
    return row ? this.toRecord(row) : null;
  }

  async getSummary(sessionId: string): Promise<string | null> {
    const record = await this.get(sessionId);
    return record?.summary ?? null;
  }

  async updateSummary(
    sessionId: string,
    summary: string,
    metadata: Record<string, unknown>,
  ): Promise<void> {
    const row = await this.findRow(sessionId);
    if (!row) {
      return;
    }
    await this.client.updateRow(this.table, row.row_id, {
      summary: utf8(summary),
      metadata: utf8(JSON.stringify(metadata)),
    });
  }

  /** Merge metadata without disturbing the rolling summary. */
  async updateMetadata(
    sessionId: string,
    patch: Record<string, unknown>,
  ): Promise<void> {
    const row = await this.findRow(sessionId);
    if (!row) {
      return;
    }
    const current = parseJson<Record<string, unknown>>(
      readUtf8(row.values, 'metadata'),
      {},
    );
    await this.client.updateRow(this.table, row.row_id, {
      metadata: utf8(JSON.stringify({ ...current, ...patch })),
    });
  }

  /** Close active sessions for a customer whose last activity predates `cutoffMs`. */
  async closeIdle(customerId: string, cutoffMs: number): Promise<void> {
    const scan = await this.client.scanRows(this.table, {
      k: SCAN_CAP,
      filters: [
        { col: 'customer_id', op: 'eq', value: utf8(customerId) },
        { col: 'status', op: 'eq', value: utf8('active') },
        { col: 'last_activity_at', op: 'lt', value: i64(cutoffMs) },
      ],
    });
    const now = Date.now();
    for (const row of scan.rows) {
      await this.client.updateRow(this.table, row.row_id, {
        status: utf8('closed'),
        closed_at: i64(now),
      });
    }
  }

  async findOrCreate(
    customerId: string,
    channel: string,
    idleTimeoutMinutes = 60,
  ): Promise<{ id: string; customerId: string; channel: string }> {
    const cutoff = Date.now() - idleTimeoutMinutes * 60_000;
    await this.closeIdle(customerId, cutoff);

    const scan = await this.client.scanRows(this.table, {
      k: SCAN_CAP,
      filters: [
        { col: 'customer_id', op: 'eq', value: utf8(customerId) },
        { col: 'status', op: 'eq', value: utf8('active') },
      ],
    });

    if (scan.rows.length > 0) {
      const best = scan.rows.reduce((a, b) =>
        readI64(a.values, 'last_activity_at') >= readI64(b.values, 'last_activity_at') ? a : b,
      );
      return {
        id: readUtf8(best.values, 'session_id'),
        customerId: readUtf8(best.values, 'customer_id'),
        channel: readUtf8(best.values, 'channel'),
      };
    }

    const id = randomUUID();
    const now = Date.now();
    await this.client.insertRow(this.table, {
      session_id: utf8(id),
      customer_id: utf8(customerId),
      channel: utf8(channel),
      status: utf8('active'),
      started_at: i64(now),
      last_activity_at: i64(now),
      closed_at: i64(0),
      summary: utf8(''),
      metadata: utf8('{}'),
    });

    return { id, customerId, channel };
  }

  async touchActivity(sessionId: string, atMs: number = Date.now()): Promise<void> {
    const row = await this.findRow(sessionId);
    if (!row) {
      return;
    }
    await this.client.updateRow(this.table, row.row_id, {
      last_activity_at: i64(atMs),
    });
  }

  /** Closed sessions ordered by `closed_at` ascending — callers filter by reflection state. */
  async listClosed(limit: number): Promise<ClosedSessionRecord[]> {
    const scan = await this.client.scanRows(this.table, {
      k: Math.max(limit * 4, SCAN_CAP),
      filters: [{ col: 'status', op: 'eq', value: utf8('closed') }],
    });

    return scan.rows
      .map((row) => ({
        sessionId: readUtf8(row.values, 'session_id'),
        customerId: readUtf8(row.values, 'customer_id'),
        closedAt: readI64(row.values, 'closed_at'),
      }))
      .sort((a, b) => a.closedAt - b.closedAt)
      .slice(0, limit);
  }
}

export function createConvoxSessionStore(config: AelioDbStorageConfig): ConvoxSessionStore {
  return new ConvoxSessionStore(config);
}
