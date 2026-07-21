import { randomUUID } from 'node:crypto';
import type { SunjetClient } from '@aelio/sunjet-client';
import { i64, readI64, readUtf8, utf8 } from '../storage/helpers.js';
import {
  SuspendedPlanPayloadSchema,
  type SuspendedPlanPayload,
  type SuspensionReason,
} from './schema.js';

export type SuspensionStoreConfig = {
  sunjet: {
    client: SunjetClient;
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
 * (recoil) or a write confirmation. Sunjet is the sole source of truth
 * (Astrolobe stays fully authoritative for runtime state).
 */
export class SuspensionStore {
  private readonly sunjet: SuspensionStoreConfig['sunjet'];

  constructor(private readonly config: SuspensionStoreConfig) {
    if (!config.sunjet) {
      throw new Error('SuspensionStore requires sunjet configuration');
    }
    this.sunjet = config.sunjet;
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

    await this.replaceSunjet(record, now);
    return record;
  }

  async get(sessionId: string): Promise<SuspendedPlanRecord | null> {
    return this.getSunjet(sessionId);
  }

  async clear(sessionId: string): Promise<void> {
    await this.clearSunjet(sessionId);
  }

  private async findSunjetRows(sessionId: string) {
    const sunjet = this.sunjet;
    const scan = await sunjet.client.scanRows(sunjet.table, {
      k: SCAN_CAP,
      filters: [{ col: 'session_id', op: 'eq', value: utf8(sessionId) }],
    });
    return scan.rows;
  }

  private async getSunjet(sessionId: string): Promise<SuspendedPlanRecord | null> {
    const rows = await this.findSunjetRows(sessionId);
    if (rows.length === 0) {
      return null;
    }
    // Most recent row wins if duplicates ever slip through (Sunjet has no
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

  private async replaceSunjet(record: SuspendedPlanRecord, now: number): Promise<void> {
    const sunjet = this.sunjet;
    // One suspension per session: clear any existing rows first (best effort;
    // Sunjet has no transactional delete+insert over HTTP).
    await this.clearSunjet(record.sessionId);
    await sunjet.client.insertRow(sunjet.table, {
      session_id: utf8(record.sessionId),
      tenant: utf8(sunjet.tenant),
      reason: utf8(record.reason),
      payload: utf8(JSON.stringify(record.payload)),
      created_at: i64(now),
      expires_at: i64(record.expiresAt),
    });
  }

  private async clearSunjet(sessionId: string): Promise<void> {
    const sunjet = this.sunjet;
    const rows = await this.findSunjetRows(sessionId);
    for (const row of rows) {
      await sunjet.client.deleteRow(sunjet.table, row.row_id);
    }
  }
}
