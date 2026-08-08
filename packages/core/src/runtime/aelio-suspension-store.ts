import { randomUUID } from 'node:crypto';
import type { ApiValue, RowValues, SunjetClient } from '@aelio/sunjet-client';
import type { AelioStorageTableNames } from '../storage/types.js';
import {
  SuspendedPlanPayloadSchema,
  type SuspendedPlanPayload,
  type SuspensionReason,
} from '../harness/schema.js';
import type { SuspendedPlanRecord, SuspensionStorePort } from '../harness/suspension.js';

const utf8 = (value: string): ApiValue => ({ type: 'utf8', value });
const i64 = (value: number): ApiValue => ({ type: 'i64', value });

const DEFAULT_TTL_MINUTES = 24 * 60;

/** One parked plan per subject; the token namespace keeps it distinct from workflow waits. */
function planToken(sessionId: string): string {
  return `plan:${sessionId}`;
}

/**
 * The Aelio DB parked-plan store. It writes into the same `runtime_continuations` table the
 * workflow engine uses, so a suspended harness plan and a workflow wait are one durable concept
 * with one expiry/inspection surface — and no part of a runtime turn depends on SQLite.
 *
 * Replacement and consumption are conditional (CAS on revision), so two workers racing on the
 * same subject cannot both resume the same parked plan.
 */
export class AelioSuspensionStore implements SuspensionStorePort {
  constructor(
    private readonly client: SunjetClient,
    private readonly tables: AelioStorageTableNames,
    private readonly tenantId: string,
    private readonly ttlMinutes: number = DEFAULT_TTL_MINUTES,
  ) {}

  async suspend(
    sessionId: string,
    reason: SuspensionReason,
    payload: SuspendedPlanPayload,
  ): Promise<SuspendedPlanRecord> {
    const now = Date.now();
    const record: SuspendedPlanRecord = {
      id: randomUUID(),
      sessionId,
      reason,
      payload,
      expiresAt: now + this.ttlMinutes * 60_000,
    };
    const token = planToken(sessionId);
    const values: RowValues = {
      token: utf8(token),
      tenant_id: utf8(this.tenantId),
      subject_id: utf8(sessionId),
      instance_id: utf8(record.id),
      status: utf8('waiting'),
      prompt: utf8(reason),
      payload_json: utf8(JSON.stringify(payload)),
      expires_at: i64(record.expiresAt),
      updated_at: i64(now),
    };

    const existing = await this.findRow(token);
    const applied = existing
      ? (await this.client.transact(
          [{
            op: 'update',
            table: this.tables.runtimeContinuations,
            row_id: existing.rowId,
            values: { ...values, revision: i64(existing.revision + 1) },
          }],
          [{
            kind: 'row_matches',
            table: this.tables.runtimeContinuations,
            row_id: existing.rowId,
            equals: { revision: i64(existing.revision) },
          }],
        )).applied
      : (await this.client.transact(
          [{ op: 'insert', table: this.tables.runtimeContinuations, values: { ...values, revision: i64(1) } }],
          [{
            kind: 'absent',
            table: this.tables.runtimeContinuations,
            equals: { token: utf8(token), tenant_id: utf8(this.tenantId) },
          }],
        )).applied;

    if (!applied) {
      // Another worker parked or resumed this subject first. Failing loudly is correct: silently
      // dropping the plan would strand the user's half-finished write with no way to resume it.
      throw new Error(`another runtime worker changed the parked plan for ${sessionId}`);
    }
    return record;
  }

  async get(sessionId: string): Promise<SuspendedPlanRecord | null> {
    const row = await this.findRow(planToken(sessionId));
    if (!row || row.status !== 'waiting') return null;
    if (row.expiresAt <= Date.now()) {
      await this.clear(sessionId);
      return null;
    }
    let parsedJson: unknown;
    try {
      parsedJson = JSON.parse(row.payloadJson);
    } catch {
      await this.clear(sessionId);
      return null;
    }
    const parsed = SuspendedPlanPayloadSchema.safeParse(parsedJson);
    if (!parsed.success) {
      // A payload written by an incompatible build — drop it rather than wedge the subject on
      // every subsequent message.
      await this.clear(sessionId);
      return null;
    }
    return {
      id: row.instanceId,
      sessionId,
      reason: row.prompt as SuspensionReason,
      payload: parsed.data,
      expiresAt: row.expiresAt,
    };
  }

  async clear(sessionId: string): Promise<void> {
    const row = await this.findRow(planToken(sessionId));
    if (!row || row.status !== 'waiting') return;
    // Terminal-mark rather than delete: the ledger and any operator replay need to see that a
    // plan existed and how it ended.
    await this.client.transact(
      [{
        op: 'update',
        table: this.tables.runtimeContinuations,
        row_id: row.rowId,
        values: { status: utf8('resumed'), revision: i64(row.revision + 1), updated_at: i64(Date.now()) },
      }],
      [{
        kind: 'row_matches',
        table: this.tables.runtimeContinuations,
        row_id: row.rowId,
        equals: { status: utf8('waiting'), revision: i64(row.revision) },
      }],
    );
  }

  private async findRow(token: string): Promise<{
    rowId: number;
    revision: number;
    status: string;
    prompt: string;
    payloadJson: string;
    instanceId: string;
    expiresAt: number;
  } | null> {
    const rows = await this.client.scanRows(this.tables.runtimeContinuations, {
      k: 2,
      filters: [
        { col: 'token', op: 'eq', value: utf8(token) },
        { col: 'tenant_id', op: 'eq', value: utf8(this.tenantId) },
      ],
    });
    if (rows.rows.length > 1) throw new Error(`parked-plan uniqueness violated for ${this.tenantId}/${token}`);
    const row = rows.rows[0];
    if (!row) return null;
    return {
      rowId: row.row_id,
      revision: readI64(row.values, 'revision'),
      status: readUtf8(row.values, 'status'),
      prompt: readUtf8(row.values, 'prompt'),
      payloadJson: readUtf8(row.values, 'payload_json'),
      instanceId: readUtf8(row.values, 'instance_id'),
      expiresAt: readI64(row.values, 'expires_at'),
    };
  }
}

function readUtf8(values: RowValues, key: string): string {
  const value = values[key];
  if (!value || value.type !== 'utf8') throw new Error(`parked-plan row is missing utf8 ${key}`);
  return value.value;
}

function readI64(values: RowValues, key: string): number {
  const value = values[key];
  if (!value || value.type !== 'i64') throw new Error(`parked-plan row is missing i64 ${key}`);
  return value.value;
}
