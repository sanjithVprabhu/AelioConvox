import { randomUUID } from 'node:crypto';
import { eq } from 'drizzle-orm';
import type { AelioDatabase } from '@aelio/db';
import { suspendedPlans } from '@aelio/db';
import type { ApiValue, SunjetClient } from '@aelio/sunjet-client';
import {
  SuspendedPlanPayloadSchema,
  type SuspendedPlanPayload,
  type SuspensionReason,
} from './schema.js';

export type SuspensionStoreConfig = {
  database: AelioDatabase;
  /** Optional Sunjet mirror (keeps all runtime state visible in Astrolobe). */
  sunjet?: {
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

function utf8(value: string): ApiValue {
  return { type: 'utf8', value };
}
function i64(value: number): ApiValue {
  return { type: 'i64', value };
}

/**
 * The suspended-plan store: one parked plan per session, awaiting user input
 * (recoil) or a write confirmation. SQLite is the transactional record the
 * resume path reads (it is always present, even with Sunjet disabled); when
 * Sunjet is configured every write is mirrored there so the whole runtime
 * state remains inspectable in Astrolobe.
 */
export class SuspensionStore {
  constructor(private readonly config: SuspensionStoreConfig) {}

  async suspend(
    sessionId: string,
    reason: SuspensionReason,
    payload: SuspendedPlanPayload,
  ): Promise<SuspendedPlanRecord> {
    const db = this.config.database.db;
    const now = Date.now();
    const ttl = (this.config.ttlMinutes ?? DEFAULT_TTL_MINUTES) * 60_000;
    const record: SuspendedPlanRecord = {
      id: randomUUID(),
      sessionId,
      reason,
      payload,
      expiresAt: now + ttl,
    };

    // One suspension per session: replace any existing one.
    await db.delete(suspendedPlans).where(eq(suspendedPlans.sessionId, sessionId));
    await db.insert(suspendedPlans).values({
      id: record.id,
      sessionId,
      reason,
      payload: payload as unknown as Record<string, unknown>,
      createdAt: new Date(now),
      expiresAt: new Date(record.expiresAt),
    });

    void this.mirror(record, now);
    return record;
  }

  async get(sessionId: string): Promise<SuspendedPlanRecord | null> {
    const db = this.config.database.db;
    const rows = await db
      .select()
      .from(suspendedPlans)
      .where(eq(suspendedPlans.sessionId, sessionId))
      .limit(1);
    const row = rows[0];
    if (!row) {
      return null;
    }
    if (row.expiresAt.getTime() <= Date.now()) {
      await this.clear(sessionId);
      return null;
    }
    const parsed = SuspendedPlanPayloadSchema.safeParse(row.payload);
    if (!parsed.success) {
      // A payload from an incompatible older build — drop it rather than wedge
      // the session on every subsequent message.
      await this.clear(sessionId);
      return null;
    }
    return {
      id: row.id,
      sessionId: row.sessionId,
      reason: row.reason as SuspensionReason,
      payload: parsed.data,
      expiresAt: row.expiresAt.getTime(),
    };
  }

  async clear(sessionId: string): Promise<void> {
    const db = this.config.database.db;
    await db.delete(suspendedPlans).where(eq(suspendedPlans.sessionId, sessionId));
  }

  private async mirror(record: SuspendedPlanRecord, now: number): Promise<void> {
    const sunjet = this.config.sunjet;
    if (!sunjet) {
      return;
    }
    try {
      await sunjet.client.insertRow(sunjet.table, {
        session_id: utf8(record.sessionId),
        tenant: utf8(sunjet.tenant),
        reason: utf8(record.reason),
        payload: utf8(JSON.stringify(record.payload)),
        created_at: i64(now),
        expires_at: i64(record.expiresAt),
      });
    } catch {
      // Mirror only — the SQLite record is what resume reads.
    }
  }
}
