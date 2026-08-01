import { randomUUID } from 'node:crypto';
import type { AelioDbClient } from '@aelio/db-client';
import { i64, readI64, readUtf8, utf8 } from '../storage/helpers.js';
import {
  SuspendedPlanPayloadSchema,
  type SuspendedPlanPayload,
  type SuspensionReason,
} from './schema.js';

export type SuspensionStoreConfig = {
  aelioDb: {
    client: AelioDbClient;
    table: string;
    tenant: string;
  };
  ttlMinutes?: number;
};

export type SuspendedPlanRecord = {
  id: string;
  sessionId: string;
  reason: SuspensionReason;
  payload: SuspendedPlanPayload;
  expiresAt: number;
};

const DEFAULT_TTL_MINUTES = 24 * 60;
const SCAN_CAP = 100;

/**
 * The suspended-plan store: one parked plan per session, awaiting user input
 * (recoil) or a write confirmation. AelioDb is the sole source of truth
 * (Aelio database stays fully authoritative for runtime state).
 */
export class SuspensionStore {
  private readonly aelioDb: SuspensionStoreConfig['aelioDb'];

  constructor(private readonly config: SuspensionStoreConfig) {
    if (!config.aelioDb) {
      throw new Error('SuspensionStore requires aelioDb configuration');
    }
    this.aelioDb = config.aelioDb;
  }

  async suspend(
    sessionId: string,
    reason: SuspensionReason,
    payload: SuspendedPlanPayload,
  ): Promise<SuspendedPlanRecord> {
    const now = Date.now();
    const ttl = (this.config.ttlMinutes ?? DEFAULT_TTL_MINUTES) * 60_000;
    const record: SuspendedPlanRecord = {
      id: randomUUID(),
      sessionId,
      reason,
      payload,
      expiresAt: now + ttl,
    };

    await this.replaceAelioDb(record, now);
    return record;
  }

  async get(sessionId: string): Promise<SuspendedPlanRecord | null> {
    return this.getAelioDb(sessionId);
  }

  async clear(sessionId: string): Promise<void> {
    await this.clearAelioDb(sessionId);
  }

  private async findAelioDbRows(sessionId: string) {
    const aelioDb = this.aelioDb;
    const scan = await aelioDb.client.scanRows(aelioDb.table, {
      k: SCAN_CAP,
      filters: [{ col: 'session_id', op: 'eq', value: utf8(sessionId) }],
    });
    return scan.rows;
  }

  private async getAelioDb(sessionId: string): Promise<SuspendedPlanRecord | null> {
    const rows = await this.findAelioDbRows(sessionId);
    if (rows.length === 0) {
      return null;
    }
    // Most recent row wins if duplicates ever slip through (AelioDb has no
    // unique-constraint enforcement over HTTP).
    const row = rows.reduce((a, b) =>
      readI64(a.values, 'created_at') >= readI64(b.values, 'created_at') ? a : b,
    );
    const expiresAt = readI64(row.values, 'expires_at');
    if (expiresAt <= Date.now()) {
      await this.clear(sessionId);
      return null;
    }
    let payloadRaw: unknown;
    try {
      payloadRaw = JSON.parse(readUtf8(row.values, 'payload'));
    } catch {
      await this.clear(sessionId);
      return null;
    }
    const parsed = SuspendedPlanPayloadSchema.safeParse(payloadRaw);
    if (!parsed.success) {
      // A payload from an incompatible older build — drop it rather than wedge
      // the session on every subsequent message.
      await this.clear(sessionId);
      return null;
    }
    return {
      id: String(row.row_id),
      sessionId,
      reason: readUtf8(row.values, 'reason') as SuspensionReason,
      payload: parsed.data,
      expiresAt,
    };
  }

  private async replaceAelioDb(record: SuspendedPlanRecord, now: number): Promise<void> {
    const aelioDb = this.aelioDb;
    // One suspension per session: clear any existing rows first (best effort;
    // AelioDb has no transactional delete+insert over HTTP).
    await this.clearAelioDb(record.sessionId);
    await aelioDb.client.insertRow(aelioDb.table, {
      session_id: utf8(record.sessionId),
      tenant: utf8(aelioDb.tenant),
      reason: utf8(record.reason),
      payload: utf8(JSON.stringify(record.payload)),
      created_at: i64(now),
      expires_at: i64(record.expiresAt),
    });
  }

  private async clearAelioDb(sessionId: string): Promise<void> {
    const aelioDb = this.aelioDb;
    const rows = await this.findAelioDbRows(sessionId);
    for (const row of rows) {
      await aelioDb.client.deleteRow(aelioDb.table, row.row_id);
    }
  }
}
