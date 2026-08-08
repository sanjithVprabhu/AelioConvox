import { randomUUID } from 'node:crypto';
import type {
  ApiValue,
  RowValues,
  SunjetClient,
  TransactionMutation,
  TransactionPrecondition,
} from '@aelio/sunjet-client';
import type { AelioStorageTableNames } from '../storage/types.js';
import {
  assertRuntimeArtifact,
  assertRuntimeEvent,
  type JsonValue,
  type RuntimeArtifactV1,
  type RuntimeEffectV1,
  type RuntimeEventV1,
  type RuntimeLedgerRecordV1,
  type RuntimeInstanceV1,
  type RuntimeSnapshotV1,
} from './contracts.js';

export type RuntimeOutboxEffect = {
  rowId: number;
  effectId: string;
  tenantId: string;
  subjectId: string;
  eventId: string;
  idempotencyKey: string;
  kind: string;
  status: 'pending' | 'dispatching' | 'delivered' | 'failed' | 'unknown';
  payload: JsonValue;
  attempts: number;
  availableAt: number;
  leaseToken?: string;
  leaseExpiresAt?: number;
  lastError?: string;
  revision: number;
};

export type RuntimeContinuation = {
  rowId: number;
  token: string;
  tenantId: string;
  subjectId: string;
  instanceId: string;
  status: 'waiting' | 'resumed' | 'expired' | 'cancelled';
  prompt: string;
  payload: JsonValue;
  expiresAt: number;
  revision: number;
};

export type RuntimeInstanceTransition = {
  instance: RuntimeInstanceRecord;
  eventId: string;
  status: RuntimeInstanceV1['status'];
  state: JsonValue;
  ledger: RuntimeLedgerRecordV1[];
  effects?: RuntimeEffectV1[];
};

export type RuntimeInstanceRecord = RuntimeInstanceV1 & { rowId: number };

/** A pinned, approved prompt. Runtime code composes it deterministically; a model never selects it. */
export type RuntimePromptTemplate = {
  system: string;
  user: string;
  /** Slot names the template is permitted to render. Anything else is rejected before the call. */
  slots: string[];
  maxOutputTokens?: number;
};

export type RuntimePromptArtifact = {
  promptId: string;
  version: string;
  digest: string;
  purpose: string;
  template: RuntimePromptTemplate;
};

export type RuntimeScheduledEvent = {
  rowId: number;
  scheduleId: string;
  tenantId: string;
  subjectId: string;
  dueAt: number;
  status: 'ready' | 'leased' | 'completed' | 'cancelled';
  payload: JsonValue;
  leaseToken?: string;
  leaseExpiresAt?: number;
  revision: number;
};

const MAX_CHILD_SCAN = 256;

const utf8 = (value: string): ApiValue => ({ type: 'utf8', value });
const i64 = (value: number): ApiValue => ({ type: 'i64', value });
const json = (value: JsonValue): ApiValue => ({ type: 'utf8', value: JSON.stringify(value) });

export type RuntimeCommit = {
  event: RuntimeEventV1;
  /** Undefined means retain the currently loaded state; null means no snapshot write. */
  state?: JsonValue | null;
  snapshotKey?: string;
  instanceId: string;
  ledger: RuntimeLedgerRecordV1[];
  effects?: RuntimeEffectV1[];
  /**
   * The snapshot revision the caller's decision was computed from (0 for a subject that did not
   * exist yet). Supplying it makes the commit a true optimistic write: if anything advanced the
   * subject while the decision was being made, the commit is refused instead of rebasing a
   * decision onto state it never saw. Omit it only for merge-style patches that are safe to
   * rebase.
   */
  expectedRevision?: number;
};

export type RuntimeCommitResult =
  | { applied: true; snapshot: RuntimeSnapshotV1; commitLsn: number }
  | { applied: false; reason: 'duplicate_event' | 'stale_snapshot' };

/**
 * Aelio DB repository for the runtime's transactional state transition. One call creates the
 * event claim, snapshot revision, immutable ledger records, and pending outbox effects in one
 * conditional WAL transaction. No external effect is performed here.
 */
export class AelioRuntimeStore {
  constructor(
    private readonly client: SunjetClient,
    private readonly tables: AelioStorageTableNames,
  ) {}

  async loadSnapshot(tenantId: string, subjectId: string, snapshotKey = 'subject'): Promise<RuntimeSnapshotV1 | null> {
    const rows = await this.client.scanRows(this.tables.runtimeSnapshots, {
      k: 2,
      filters: [
        { col: 'tenant_id', op: 'eq', value: utf8(tenantId) },
        { col: 'subject_id', op: 'eq', value: utf8(subjectId) },
        { col: 'snapshot_key', op: 'eq', value: utf8(snapshotKey) },
      ],
    });
    if (rows.rows.length > 1) throw new Error(`runtime snapshot uniqueness violated for ${tenantId}/${subjectId}/${snapshotKey}`);
    const row = rows.rows[0];
    if (!row) return null;
    return {
      apiVersion: 'aelio.runtime.snapshot/v1',
      tenantId,
      subjectId,
      snapshotKey,
      revision: readI64(row.values, 'revision'),
      state: parseJson(readUtf8(row.values, 'payload_json')),
      updatedAt: readI64(row.values, 'updated_at'),
    };
  }

  async commit(input: RuntimeCommit): Promise<RuntimeCommitResult> {
    assertRuntimeEvent(input.event);
    const snapshotKey = input.snapshotKey ?? 'subject';
    const current = await this.loadSnapshot(input.event.tenantId, input.event.subjectId, snapshotKey);
    if (input.state === null) throw new Error('runtime commit must persist a subject snapshot; use an explicit empty object for no state');
    if (input.expectedRevision !== undefined && (current?.revision ?? 0) !== input.expectedRevision) {
      // The subject moved on while this decision was being made. Committing anyway would drop
      // whatever landed in between; the caller must re-read and decide again.
      return { applied: false, reason: 'stale_snapshot' };
    }
    const state = input.state ?? current?.state ?? {};
    const revision = (current?.revision ?? 0) + 1;
    const now = Date.now();
    const snapshot: RuntimeSnapshotV1 = {
      apiVersion: 'aelio.runtime.snapshot/v1',
      tenantId: input.event.tenantId,
      subjectId: input.event.subjectId,
      snapshotKey,
      revision,
      state,
      updatedAt: now,
    };

    const currentRowId = current
      ? await this.snapshotRowId(input.event.tenantId, input.event.subjectId, snapshotKey)
      : undefined;
    const preconditions: TransactionPrecondition[] = [
      {
        kind: 'absent',
        table: this.tables.runtimeEvents,
        equals: {
          idempotency_key: utf8(scopedEventKey(input.event)),
        },
      },
      current
        ? {
            kind: 'row_matches',
            table: this.tables.runtimeSnapshots,
            row_id: currentRowId!,
            equals: { revision: i64(current.revision) },
          }
        : {
            kind: 'absent',
            table: this.tables.runtimeSnapshots,
            equals: snapshotIdentity(snapshot),
          },
    ];

    const mutations: TransactionMutation[] = [
      {
        op: 'insert',
        table: this.tables.runtimeEvents,
        values: {
          event_id: utf8(input.event.eventId), tenant_id: utf8(input.event.tenantId), subject_id: utf8(input.event.subjectId),
          idempotency_key: utf8(scopedEventKey(input.event)), kind: utf8(input.event.kind), payload_json: json(input.event.payload),
          received_at: i64(input.event.receivedAt),
        },
      },
      current
        ? {
            op: 'update', table: this.tables.runtimeSnapshots,
            row_id: currentRowId!,
            values: snapshotValues(snapshot),
          }
        : { op: 'insert', table: this.tables.runtimeSnapshots, values: snapshotValues(snapshot) },
      ...input.ledger.map((record) => ledgerMutation(this.tables.runtimeLedger, input.event, input.instanceId, record, now)),
      ...(input.effects ?? []).map((effect) => outboxMutation(this.tables.runtimeOutbox, input.event, effect, now)),
    ];

    const result = await this.client.transact(mutations, preconditions);
    if (!result.applied) {
      const duplicate = await this.hasEvent(input.event);
      return { applied: false, reason: duplicate ? 'duplicate_event' : 'stale_snapshot' };
    }
    if (result.commit_lsn === undefined) throw new Error('Aelio DB applied transaction without a commit LSN');
    return { applied: true, snapshot, commitLsn: result.commit_lsn };
  }

  /**
   * Merge fields into a subject snapshot outside a turn, under CAS with bounded retry.
   *
   * This is the path an SDK push takes (`set_state`, `set_flow_progress`): the tenant's own
   * backend is the authority on lifecycle, and the runtime must see that push on the very next
   * event. It deliberately writes no event claim and no ledger decision — it is not a turn.
   */
  async patchSubjectState(
    tenantId: string,
    subjectId: string,
    patch: Record<string, JsonValue>,
    snapshotKey = 'subject',
    attempts = 3,
  ): Promise<RuntimeSnapshotV1> {
    for (let attempt = 0; attempt < attempts; attempt += 1) {
      const current = await this.loadSnapshot(tenantId, subjectId, snapshotKey);
      const state = {
        ...(current?.state && typeof current.state === 'object' && !Array.isArray(current.state) ? current.state : {}),
        ...patch,
      };
      const next: RuntimeSnapshotV1 = {
        apiVersion: 'aelio.runtime.snapshot/v1',
        tenantId, subjectId, snapshotKey,
        revision: (current?.revision ?? 0) + 1,
        state,
        updatedAt: Date.now(),
      };
      const result = current
        ? await this.client.transact(
            [{
              op: 'update', table: this.tables.runtimeSnapshots,
              row_id: await this.snapshotRowId(tenantId, subjectId, snapshotKey),
              values: snapshotValues(next),
            }],
            [{
              kind: 'row_matches', table: this.tables.runtimeSnapshots,
              row_id: await this.snapshotRowId(tenantId, subjectId, snapshotKey),
              equals: { revision: i64(current.revision) },
            }],
          )
        : await this.client.transact(
            [{ op: 'insert', table: this.tables.runtimeSnapshots, values: snapshotValues(next) }],
            [{ kind: 'absent', table: this.tables.runtimeSnapshots, equals: snapshotIdentity(next) }],
          );
      if (result.applied) return next;
    }
    throw new Error(`could not apply subject state patch for ${tenantId}/${subjectId} after ${attempts} attempts`);
  }

  async installArtifact(artifact: RuntimeArtifactV1): Promise<boolean> {
    assertRuntimeArtifact(artifact);
    const result = await this.client.transact(
      [{
        op: 'insert', table: this.tables.workflowArtifacts,
        values: {
          artifact_id: utf8(artifact.artifactId), version: utf8(artifact.version), digest: utf8(artifact.digest),
          status: utf8(artifact.status), definition_json: json(artifact as unknown as JsonValue), created_at: i64(artifact.createdAt),
        },
      }],
      [{
        kind: 'absent', table: this.tables.workflowArtifacts,
        equals: { artifact_id: utf8(artifact.artifactId), version: utf8(artifact.version) },
      }],
    );
    return result.applied;
  }

  async loadArtifact(artifactId: string, version: string): Promise<RuntimeArtifactV1 | null> {
    const rows = await this.client.scanRows(this.tables.workflowArtifacts, {
      k: 2,
      filters: [
        { col: 'artifact_id', op: 'eq', value: utf8(artifactId) },
        { col: 'version', op: 'eq', value: utf8(version) },
        { col: 'status', op: 'eq', value: utf8('approved') },
      ],
    });
    if (rows.rows.length > 1) throw new Error(`runtime artifact uniqueness violated for ${artifactId}@${version}`);
    const row = rows.rows[0];
    return row ? parseJson(readUtf8(row.values, 'definition_json')) as unknown as RuntimeArtifactV1 : null;
  }

  async createInstance(instance: Omit<RuntimeInstanceV1, 'revision' | 'updatedAt'>): Promise<boolean> {
    const now = Date.now();
    const result = await this.client.transact([{
      op: 'insert', table: this.tables.workflowInstances,
      values: instanceValues({ ...instance, revision: 1, updatedAt: now }),
    }], [{
      kind: 'absent', table: this.tables.workflowInstances,
      equals: { tenant_id: utf8(instance.tenantId), instance_id: utf8(instance.instanceId) },
    }]);
    return result.applied;
  }

  async loadInstance(tenantId: string, instanceId: string): Promise<RuntimeInstanceRecord | null> {
    const rows = await this.client.scanRows(this.tables.workflowInstances, {
      k: 2,
      filters: [{ col: 'tenant_id', op: 'eq', value: utf8(tenantId) }, { col: 'instance_id', op: 'eq', value: utf8(instanceId) }],
    });
    if (rows.rows.length > 1) throw new Error(`runtime instance uniqueness violated for ${tenantId}/${instanceId}`);
    const row = rows.rows[0];
    return row ? parseInstance(row.row_id, row.values) : null;
  }

  /** CAS an instance transition while appending its ledger and child effects in the same transaction. */
  async commitInstanceTransition(input: RuntimeInstanceTransition): Promise<boolean> {
    const next: RuntimeInstanceV1 = {
      ...input.instance, status: input.status, state: input.state,
      revision: input.instance.revision + 1, updatedAt: Date.now(),
    };
    const now = Date.now();
    const result = await this.client.transact([
      { op: 'update', table: this.tables.workflowInstances, row_id: input.instance.rowId, values: instanceValues(next) },
      ...input.ledger.map((record) => instanceLedgerMutation(this.tables.runtimeLedger, next, input.eventId, record, now)),
      ...(input.effects ?? []).map((effect) => instanceOutboxMutation(this.tables.runtimeOutbox, next, input.eventId, effect, now)),
    ], [{
      kind: 'row_matches', table: this.tables.workflowInstances, row_id: input.instance.rowId,
      equals: { status: utf8(input.instance.status), revision: i64(input.instance.revision) },
    }]);
    return result.applied;
  }

  async loadPromptArtifact(promptId: string, version: string): Promise<RuntimePromptArtifact | null> {
    const rows = await this.client.scanRows(this.tables.promptArtifacts, {
      k: 2,
      filters: [
        { col: 'prompt_id', op: 'eq', value: utf8(promptId) },
        { col: 'version', op: 'eq', value: utf8(version) },
        { col: 'status', op: 'eq', value: utf8('approved') },
      ],
    });
    if (rows.rows.length > 1) throw new Error(`prompt artifact uniqueness violated for ${promptId}@${version}`);
    const row = rows.rows[0];
    if (!row) return null;
    return {
      promptId, version,
      digest: readUtf8(row.values, 'digest'),
      purpose: readUtf8(row.values, 'purpose'),
      template: parseJson(readUtf8(row.values, 'template_json')) as unknown as RuntimePromptTemplate,
    };
  }

  async installPromptArtifact(artifact: RuntimePromptArtifact): Promise<boolean> {
    const result = await this.client.transact(
      [{
        op: 'insert', table: this.tables.promptArtifacts,
        values: {
          prompt_id: utf8(artifact.promptId), version: utf8(artifact.version), digest: utf8(artifact.digest),
          status: utf8('approved'), purpose: utf8(artifact.purpose),
          template_json: json(artifact.template as unknown as JsonValue), created_at: i64(Date.now()),
        },
      }],
      [{
        kind: 'absent', table: this.tables.promptArtifacts,
        equals: { prompt_id: utf8(artifact.promptId), version: utf8(artifact.version) },
      }],
    );
    return result.applied;
  }

  /**
   * Claim the right to perform one externally-visible artifact effect.
   *
   * This is the intent/result/unknown protocol: the intent record commits *before* the effect is
   * attempted, and the result commits after. A replay therefore sees one of three states, and the
   * only one that may re-run the effect is `new`. `unknown` means a previous attempt crashed
   * between dispatch and result — it must be reconciled, never silently repeated, because the
   * effect may already have happened.
   */
  async beginEffect(input: {
    key: string;
    tenantId: string;
    subjectId: string;
    instanceId: string;
    eventId: string;
    kind: string;
    request: JsonValue;
  }): Promise<{ state: 'new' } | { state: 'completed'; result: JsonValue } | { state: 'unknown' }> {
    const intentId = `effect:${input.key}`;
    const result = await this.client.transact(
      [{
        op: 'insert', table: this.tables.runtimeLedger,
        values: {
          record_id: utf8(intentId), tenant_id: utf8(input.tenantId), subject_id: utf8(input.subjectId),
          event_id: utf8(input.eventId), instance_id: utf8(input.instanceId), kind: utf8('effect'),
          payload_json: json({ phase: 'intent', effectKind: input.kind, request: input.request }),
          created_at: i64(Date.now()),
        },
      }],
      [{ kind: 'absent', table: this.tables.runtimeLedger, equals: { record_id: utf8(intentId) } }],
    );
    if (result.applied) return { state: 'new' };

    const recorded = await this.client.scanRows(this.tables.runtimeLedger, {
      k: 1, filters: [{ col: 'record_id', op: 'eq', value: utf8(`effect:${input.key}:result`) }],
    });
    const row = recorded.rows[0];
    if (!row) return { state: 'unknown' };
    const payload = parseJson(readUtf8(row.values, 'payload_json'));
    const stored = payload && typeof payload === 'object' && !Array.isArray(payload) ? payload.result : null;
    return { state: 'completed', result: stored ?? null };
  }

  async recordEffectResult(input: {
    key: string;
    tenantId: string;
    subjectId: string;
    instanceId: string;
    eventId: string;
    result: JsonValue;
  }): Promise<void> {
    await this.client.transact(
      [{
        op: 'insert', table: this.tables.runtimeLedger,
        values: {
          record_id: utf8(`effect:${input.key}:result`), tenant_id: utf8(input.tenantId), subject_id: utf8(input.subjectId),
          event_id: utf8(input.eventId), instance_id: utf8(input.instanceId), kind: utf8('effect'),
          payload_json: json({ phase: 'result', result: input.result }), created_at: i64(Date.now()),
        },
      }],
      [{ kind: 'absent', table: this.tables.runtimeLedger, equals: { record_id: utf8(`effect:${input.key}:result`) } }],
    );
  }

  /**
   * Queue one effect outside a turn transaction, idempotently on its scoped idempotency key.
   *
   * The artifact runner uses this to start a detached child: the child instance and its start
   * effect are both derived from the parent node, so a retried parent step joins the existing
   * child instead of forking a second one.
   */
  async enqueueEffect(input: {
    tenantId: string;
    subjectId: string;
    eventId: string;
    effect: RuntimeEffectV1;
  }): Promise<boolean> {
    const now = Date.now();
    const result = await this.client.transact(
      [outboxMutation(
        this.tables.runtimeOutbox,
        { tenantId: input.tenantId, subjectId: input.subjectId, eventId: input.eventId } as RuntimeEventV1,
        input.effect,
        now,
      )],
      [{
        kind: 'absent', table: this.tables.runtimeOutbox,
        equals: { idempotency_key: utf8(`${input.tenantId}:${input.effect.idempotencyKey}`) },
      }],
    );
    return result.applied;
  }

  /** Terminal children of `parentInstanceId`, for a join. Terminal means completed, failed, or cancelled. */
  async listChildInstances(tenantId: string, parentInstanceId: string): Promise<RuntimeInstanceRecord[]> {
    const rows = await this.client.scanRows(this.tables.workflowInstances, {
      k: MAX_CHILD_SCAN,
      filters: [
        { col: 'tenant_id', op: 'eq', value: utf8(tenantId) },
        { col: 'parent_instance_id', op: 'eq', value: utf8(parentInstanceId) },
      ],
    });
    return rows.rows.map((row) => parseInstance(row.row_id, row.values));
  }

  async listPendingEffects(limit = 32, now = Date.now()): Promise<RuntimeOutboxEffect[]> {
    const rows = await this.client.scanRows(this.tables.runtimeOutbox, {
      k: Math.min(Math.max(limit, 1), 256),
      filters: [{ col: 'status', op: 'eq', value: utf8('pending') }],
    });
    return rows.rows.map((row) => parseEffect(row.row_id, row.values)).filter((effect) => effect.availableAt <= now);
  }

  /** Claim before dispatch. Only one worker can transition a ready row to a fenced lease. */
  async claimEffect(effect: RuntimeOutboxEffect, workerId = 'runtime-outbox', leaseMs = 30_000): Promise<RuntimeOutboxEffect | null> {
    const leaseToken = `${workerId}:${randomUUID()}`;
    const leaseExpiresAt = Date.now() + leaseMs;
    const result = await this.client.transact([{
      op: 'update', table: this.tables.runtimeOutbox, row_id: effect.rowId,
      values: {
        status: utf8('dispatching'), attempts: i64(effect.attempts + 1), lease_token: utf8(leaseToken),
        lease_expires_at: i64(leaseExpiresAt), revision: i64(effect.revision + 1), updated_at: i64(Date.now()),
      },
    }], [{
      kind: 'row_matches', table: this.tables.runtimeOutbox, row_id: effect.rowId,
      equals: { status: utf8('pending'), attempts: i64(effect.attempts) },
    }]);
    return result.applied
      ? { ...effect, status: 'dispatching', attempts: effect.attempts + 1, leaseToken, leaseExpiresAt, revision: effect.revision + 1 }
      : null;
  }

  async completeEffect(effect: RuntimeOutboxEffect, outcome: 'delivered' | 'failed'): Promise<boolean> {
    if (effect.status !== 'dispatching' || !effect.leaseToken) throw new Error('only a leased runtime effect may complete');
    const result = await this.client.transact([{
      op: 'update', table: this.tables.runtimeOutbox, row_id: effect.rowId,
      values: { status: utf8(outcome), lease_token: utf8(''), lease_expires_at: i64(0), revision: i64(effect.revision + 1), updated_at: i64(Date.now()) },
    }], [{
      kind: 'row_matches', table: this.tables.runtimeOutbox, row_id: effect.rowId,
      equals: { status: utf8('dispatching'), attempts: i64(effect.attempts), lease_token: utf8(effect.leaseToken), revision: i64(effect.revision) },
    }]);
    return result.applied;
  }

  /** Release a failed lease for durable exponential retry. Returns false only when the lease was lost. */
  async retryEffect(effect: RuntimeOutboxEffect, error: unknown, maxAttempts = 8): Promise<'retrying' | 'failed' | 'lost'> {
    if (effect.status !== 'dispatching' || !effect.leaseToken) throw new Error('only a leased runtime effect may retry');
    const terminal = effect.attempts >= maxAttempts;
    const delayMs = terminal ? 0 : Math.min(300_000, 1_000 * 2 ** Math.max(0, effect.attempts - 1));
    const message = error instanceof Error ? error.message.slice(0, 2_000) : String(error).slice(0, 2_000);
    const result = await this.client.transact([{
      op: 'update', table: this.tables.runtimeOutbox, row_id: effect.rowId,
      values: {
        status: utf8(terminal ? 'failed' : 'pending'), available_at: i64(Date.now() + delayMs),
        lease_token: utf8(''), lease_expires_at: i64(0), last_error: utf8(message),
        revision: i64(effect.revision + 1), updated_at: i64(Date.now()),
      },
    }], [{
      kind: 'row_matches', table: this.tables.runtimeOutbox, row_id: effect.rowId,
      equals: { status: utf8('dispatching'), lease_token: utf8(effect.leaseToken), revision: i64(effect.revision) },
    }]);
    return result.applied ? (terminal ? 'failed' : 'retrying') : 'lost';
  }

  /**
   * Recover leases that expired while an effect was in flight.
   *
   * An expired `dispatching` lease means the worker died at an unknown point — the effect may or
   * may not have reached the provider. Requeuing is only safe when a redelivery would be
   * deduplicated downstream. `isRedeliverySafe` decides that per effect; anything else moves to
   * `unknown` for reconciliation, because telling a customer something twice is worse than
   * telling an operator once.
   */
  async requeueExpiredEffects(
    now = Date.now(),
    limit = 32,
    isRedeliverySafe: (effect: RuntimeOutboxEffect) => boolean = () => true,
  ): Promise<{ requeued: number; unknown: number }> {
    const rows = await this.client.scanRows(this.tables.runtimeOutbox, {
      k: Math.min(Math.max(limit, 1), 256), filters: [{ col: 'status', op: 'eq', value: utf8('dispatching') }],
    });
    let requeued = 0;
    let unknown = 0;
    for (const row of rows.rows) {
      const effect = parseEffect(row.row_id, row.values);
      if (!effect.leaseToken || !effect.leaseExpiresAt || effect.leaseExpiresAt > now) continue;
      const safe = isRedeliverySafe(effect);
      const result = await this.client.transact([{
        op: 'update', table: this.tables.runtimeOutbox, row_id: effect.rowId,
        values: {
          status: utf8(safe ? 'pending' : 'unknown'), available_at: i64(now),
          lease_token: utf8(''), lease_expires_at: i64(0),
          ...(safe ? {} : { last_error: utf8('lease expired mid-dispatch; outcome unknown, needs reconciliation') }),
          revision: i64(effect.revision + 1), updated_at: i64(now),
        },
      }], [{
        kind: 'row_matches', table: this.tables.runtimeOutbox, row_id: effect.rowId,
        equals: { status: utf8('dispatching'), lease_token: utf8(effect.leaseToken), revision: i64(effect.revision) },
      }]);
      if (!result.applied) continue;
      if (safe) requeued += 1;
      else unknown += 1;
    }
    return { requeued, unknown };
  }

  /** Effects whose external outcome could not be determined. An operator or a reconcile job owns these. */
  async listUnknownEffects(limit = 32): Promise<RuntimeOutboxEffect[]> {
    const rows = await this.client.scanRows(this.tables.runtimeOutbox, {
      k: Math.min(Math.max(limit, 1), 256), filters: [{ col: 'status', op: 'eq', value: utf8('unknown') }],
    });
    return rows.rows.map((row) => parseEffect(row.row_id, row.values));
  }

  /** Resolve a reconciled effect. `delivered` when the provider confirms it landed, `failed` when it did not. */
  async resolveUnknownEffect(effect: RuntimeOutboxEffect, outcome: 'delivered' | 'failed', note: string): Promise<boolean> {
    if (effect.status !== 'unknown') throw new Error('only an unknown runtime effect may be reconciled');
    const result = await this.client.transact([{
      op: 'update', table: this.tables.runtimeOutbox, row_id: effect.rowId,
      values: { status: utf8(outcome), last_error: utf8(note.slice(0, 2_000)), revision: i64(effect.revision + 1), updated_at: i64(Date.now()) },
    }], [{
      kind: 'row_matches', table: this.tables.runtimeOutbox, row_id: effect.rowId,
      equals: { status: utf8('unknown'), revision: i64(effect.revision) },
    }]);
    return result.applied;
  }

  async createContinuation(input: Omit<RuntimeContinuation, 'rowId' | 'status' | 'revision'>): Promise<boolean> {
    const result = await this.client.transact([{
      op: 'insert', table: this.tables.runtimeContinuations,
      values: {
        token: utf8(input.token), tenant_id: utf8(input.tenantId), subject_id: utf8(input.subjectId),
        instance_id: utf8(input.instanceId), status: utf8('waiting'), prompt: utf8(input.prompt), payload_json: json(input.payload),
        expires_at: i64(input.expiresAt), revision: i64(1), updated_at: i64(Date.now()),
      },
    }], [{
      kind: 'absent', table: this.tables.runtimeContinuations,
      equals: { token: utf8(input.token), tenant_id: utf8(input.tenantId) },
    }]);
    return result.applied;
  }

  async consumeContinuation(tenantId: string, token: string): Promise<RuntimeContinuation | null> {
    const rows = await this.client.scanRows(this.tables.runtimeContinuations, {
      k: 2,
      filters: [{ col: 'token', op: 'eq', value: utf8(token) }, { col: 'tenant_id', op: 'eq', value: utf8(tenantId) }],
    });
    if (rows.rows.length > 1) throw new Error(`runtime continuation uniqueness violated for ${tenantId}/${token}`);
    const row = rows.rows[0];
    if (!row) return null;
    const continuation = parseContinuation(row.row_id, row.values);
    if (continuation.status !== 'waiting' || continuation.expiresAt <= Date.now()) return null;
    const result = await this.client.transact([{
      op: 'update', table: this.tables.runtimeContinuations, row_id: continuation.rowId,
      values: { status: utf8('resumed'), revision: i64(continuation.revision + 1), updated_at: i64(Date.now()) },
    }], [{
      kind: 'row_matches', table: this.tables.runtimeContinuations, row_id: continuation.rowId,
      equals: { status: utf8('waiting'), revision: i64(continuation.revision) },
    }]);
    return result.applied ? continuation : null;
  }

  async scheduleEvent(input: Omit<RuntimeScheduledEvent, 'rowId' | 'status' | 'leaseToken' | 'leaseExpiresAt' | 'revision'>): Promise<boolean> {
    const result = await this.client.transact([{
      op: 'insert', table: this.tables.scheduledEvents,
      values: scheduledEventValues({ ...input, status: 'ready', revision: 1 }),
    }], [{
      kind: 'absent', table: this.tables.scheduledEvents,
      equals: { tenant_id: utf8(input.tenantId), schedule_id: utf8(input.scheduleId) },
    }]);
    return result.applied;
  }

  async listDueScheduledEvents(now: number, limit = 32): Promise<RuntimeScheduledEvent[]> {
    const rows = await this.client.scanRows(this.tables.scheduledEvents, {
      k: Math.min(Math.max(limit, 1), 256),
      filters: [{ col: 'status', op: 'eq', value: utf8('ready') }, { col: 'due_at', op: 'le', value: i64(now) }],
    });
    return rows.rows.map((row) => parseScheduledEvent(row.row_id, row.values));
  }

  async claimScheduledEvent(event: RuntimeScheduledEvent, workerId: string, leaseMs = 30_000): Promise<RuntimeScheduledEvent | null> {
    const leaseToken = `${workerId}:${randomUUID()}`;
    const leaseExpiresAt = Date.now() + leaseMs;
    const result = await this.client.transact([{
      op: 'update', table: this.tables.scheduledEvents, row_id: event.rowId,
      values: { status: utf8('leased'), lease_token: utf8(leaseToken), lease_expires_at: i64(leaseExpiresAt), revision: i64(event.revision + 1) },
    }], [{
      kind: 'row_matches', table: this.tables.scheduledEvents, row_id: event.rowId,
      equals: { status: utf8('ready'), revision: i64(event.revision) },
    }]);
    return result.applied ? { ...event, status: 'leased', leaseToken, leaseExpiresAt, revision: event.revision + 1 } : null;
  }

  async completeScheduledEvent(event: RuntimeScheduledEvent): Promise<boolean> {
    if (event.status !== 'leased' || !event.leaseToken) throw new Error('only a leased scheduled event may complete');
    const result = await this.client.transact([{
      op: 'update', table: this.tables.scheduledEvents, row_id: event.rowId,
      values: { status: utf8('completed'), revision: i64(event.revision + 1) },
    }], [{
      kind: 'row_matches', table: this.tables.scheduledEvents, row_id: event.rowId,
      equals: { status: utf8('leased'), lease_token: utf8(event.leaseToken), revision: i64(event.revision) },
    }]);
    return result.applied;
  }

  async requeueExpiredScheduledEvents(now: number, limit = 32): Promise<number> {
    const rows = await this.client.scanRows(this.tables.scheduledEvents, {
      k: Math.min(Math.max(limit, 1), 256),
      filters: [{ col: 'status', op: 'eq', value: utf8('leased') }, { col: 'lease_expires_at', op: 'le', value: i64(now) }],
    });
    let requeued = 0;
    for (const row of rows.rows) {
      const event = parseScheduledEvent(row.row_id, row.values);
      const result = await this.client.transact([{
        op: 'update', table: this.tables.scheduledEvents, row_id: event.rowId,
        values: { status: utf8('ready'), lease_token: utf8(''), lease_expires_at: i64(0), revision: i64(event.revision + 1) },
      }], [{
        kind: 'row_matches', table: this.tables.scheduledEvents, row_id: event.rowId,
        equals: { status: utf8('leased'), lease_token: utf8(event.leaseToken ?? ''), revision: i64(event.revision) },
      }]);
      if (result.applied) requeued += 1;
    }
    return requeued;
  }

  private async snapshotRowId(tenantId: string, subjectId: string, snapshotKey: string): Promise<number> {
    const rows = await this.client.scanRows(this.tables.runtimeSnapshots, {
      k: 2,
      filters: [
        { col: 'tenant_id', op: 'eq', value: utf8(tenantId) }, { col: 'subject_id', op: 'eq', value: utf8(subjectId) },
        { col: 'snapshot_key', op: 'eq', value: utf8(snapshotKey) },
      ],
    });
    if (rows.rows.length !== 1) throw new Error('runtime snapshot disappeared or is not unique during commit');
    return rows.rows[0]!.row_id;
  }

  private async hasEvent(event: RuntimeEventV1): Promise<boolean> {
    const rows = await this.client.scanRows(this.tables.runtimeEvents, {
      k: 1,
      filters: [{ col: 'idempotency_key', op: 'eq', value: utf8(scopedEventKey(event)) }],
    });
    return rows.rows.length > 0;
  }
}

function scopedEventKey(event: RuntimeEventV1): string {
  return `${event.tenantId}:${event.idempotencyKey}`;
}

function snapshotIdentity(snapshot: RuntimeSnapshotV1): RowValues {
  return { tenant_id: utf8(snapshot.tenantId), subject_id: utf8(snapshot.subjectId), snapshot_key: utf8(snapshot.snapshotKey) };
}

function snapshotValues(snapshot: RuntimeSnapshotV1): RowValues {
  return { ...snapshotIdentity(snapshot), revision: i64(snapshot.revision), payload_json: json(snapshot.state), updated_at: i64(snapshot.updatedAt) };
}

function instanceValues(instance: RuntimeInstanceV1): RowValues {
  return {
    instance_id: utf8(instance.instanceId), tenant_id: utf8(instance.tenantId), subject_id: utf8(instance.subjectId),
    artifact_id: utf8(instance.artifactId), artifact_version: utf8(instance.artifactVersion),
    parent_instance_id: utf8(instance.parentInstanceId ?? ''), status: utf8(instance.status), state_json: json(instance.state),
    revision: i64(instance.revision), updated_at: i64(instance.updatedAt),
  };
}

function scheduledEventValues(event: Omit<RuntimeScheduledEvent, 'rowId'>): RowValues {
  return {
    schedule_id: utf8(event.scheduleId), tenant_id: utf8(event.tenantId), subject_id: utf8(event.subjectId),
    due_at: i64(event.dueAt), status: utf8(event.status), payload_json: json(event.payload),
    lease_token: utf8(event.leaseToken ?? ''), lease_expires_at: i64(event.leaseExpiresAt ?? 0),
    revision: i64(event.revision),
  };
}

function ledgerMutation(table: string, event: RuntimeEventV1, instanceId: string, record: RuntimeLedgerRecordV1, now: number): TransactionMutation {
  return {
    op: 'insert', table,
    values: {
      record_id: utf8(record.recordId || randomUUID()), tenant_id: utf8(event.tenantId), subject_id: utf8(event.subjectId),
      event_id: utf8(event.eventId), instance_id: utf8(record.instanceId || instanceId), kind: utf8(record.kind),
      payload_json: json(record.payload), created_at: i64(now),
    },
  };
}

function outboxMutation(table: string, event: RuntimeEventV1, effect: RuntimeEffectV1, now: number): TransactionMutation {
  return {
    op: 'insert', table,
    values: {
      effect_id: utf8(effect.effectId), tenant_id: utf8(event.tenantId), subject_id: utf8(event.subjectId), event_id: utf8(event.eventId),
      idempotency_key: utf8(`${event.tenantId}:${effect.idempotencyKey}`), kind: utf8(effect.kind), status: utf8('pending'),
      payload_json: json(effect.payload), attempts: i64(0), available_at: i64(now), lease_token: utf8(''), lease_expires_at: i64(0),
      last_error: utf8(''), revision: i64(1), created_at: i64(now), updated_at: i64(now),
    },
  };
}

function instanceLedgerMutation(
  table: string,
  instance: RuntimeInstanceV1,
  eventId: string,
  record: RuntimeLedgerRecordV1,
  now: number,
): TransactionMutation {
  return {
    op: 'insert', table,
    values: {
      record_id: utf8(record.recordId || randomUUID()), tenant_id: utf8(instance.tenantId), subject_id: utf8(instance.subjectId),
      event_id: utf8(eventId), instance_id: utf8(instance.instanceId), kind: utf8(record.kind),
      payload_json: json(record.payload), created_at: i64(now),
    },
  };
}

function instanceOutboxMutation(
  table: string,
  instance: RuntimeInstanceV1,
  eventId: string,
  effect: RuntimeEffectV1,
  now: number,
): TransactionMutation {
  return {
    op: 'insert', table,
    values: {
      effect_id: utf8(effect.effectId), tenant_id: utf8(instance.tenantId), subject_id: utf8(instance.subjectId), event_id: utf8(eventId),
      idempotency_key: utf8(`${instance.tenantId}:${effect.idempotencyKey}`), kind: utf8(effect.kind), status: utf8('pending'),
      payload_json: json(effect.payload), attempts: i64(0), available_at: i64(now), lease_token: utf8(''), lease_expires_at: i64(0),
      last_error: utf8(''), revision: i64(1), created_at: i64(now), updated_at: i64(now),
    },
  };
}

function readUtf8(values: RowValues, key: string): string {
  const value = values[key];
  if (!value || value.type !== 'utf8') throw new Error(`runtime storage row is missing utf8 ${key}`);
  return value.value;
}

function readI64(values: RowValues, key: string): number {
  const value = values[key];
  if (!value || value.type !== 'i64') throw new Error(`runtime storage row is missing i64 ${key}`);
  return value.value;
}

function readUtf8Default(values: RowValues, key: string): string {
  const value = values[key];
  return value?.type === 'utf8' ? value.value : '';
}

function readI64Default(values: RowValues, key: string, fallback: number): number {
  const value = values[key];
  return value?.type === 'i64' ? value.value : fallback;
}

function parseJson(value: string): JsonValue {
  try { return JSON.parse(value) as JsonValue; } catch { throw new Error('runtime storage contains invalid JSON'); }
}

function parseEffect(rowId: number, values: RowValues): RuntimeOutboxEffect {
  return {
    rowId, effectId: readUtf8(values, 'effect_id'), tenantId: readUtf8(values, 'tenant_id'), subjectId: readUtf8(values, 'subject_id'),
    eventId: readUtf8(values, 'event_id'), idempotencyKey: readUtf8(values, 'idempotency_key'), kind: readUtf8(values, 'kind'),
    status: readUtf8(values, 'status') as RuntimeOutboxEffect['status'], payload: parseJson(readUtf8(values, 'payload_json')),
    attempts: readI64(values, 'attempts'), availableAt: readI64Default(values, 'available_at', 0),
    leaseToken: readUtf8Default(values, 'lease_token') || undefined,
    leaseExpiresAt: readI64Default(values, 'lease_expires_at', 0) || undefined,
    lastError: readUtf8Default(values, 'last_error') || undefined,
    revision: readI64Default(values, 'revision', 0),
  };
}

function parseContinuation(rowId: number, values: RowValues): RuntimeContinuation {
  return {
    rowId, token: readUtf8(values, 'token'), tenantId: readUtf8(values, 'tenant_id'), subjectId: readUtf8(values, 'subject_id'),
    instanceId: readUtf8(values, 'instance_id'), status: readUtf8(values, 'status') as RuntimeContinuation['status'],
    prompt: readUtf8(values, 'prompt'), payload: parseJson(readUtf8(values, 'payload_json')), expiresAt: readI64(values, 'expires_at'),
    revision: readI64(values, 'revision'),
  };
}

function parseInstance(rowId: number, values: RowValues): RuntimeInstanceRecord {
  return {
    rowId, instanceId: readUtf8(values, 'instance_id'), tenantId: readUtf8(values, 'tenant_id'), subjectId: readUtf8(values, 'subject_id'),
    artifactId: readUtf8(values, 'artifact_id'), artifactVersion: readUtf8(values, 'artifact_version'),
    parentInstanceId: readUtf8(values, 'parent_instance_id') || undefined,
    status: readUtf8(values, 'status') as RuntimeInstanceV1['status'], revision: readI64(values, 'revision'),
    state: parseJson(readUtf8(values, 'state_json')), updatedAt: readI64(values, 'updated_at'),
  };
}

function parseScheduledEvent(rowId: number, values: RowValues): RuntimeScheduledEvent {
  return {
    rowId, scheduleId: readUtf8(values, 'schedule_id'), tenantId: readUtf8(values, 'tenant_id'),
    subjectId: readUtf8(values, 'subject_id'), dueAt: readI64(values, 'due_at'),
    status: readUtf8(values, 'status') as RuntimeScheduledEvent['status'],
    payload: parseJson(readUtf8(values, 'payload_json')),
    leaseToken: readUtf8(values, 'lease_token') || undefined,
    leaseExpiresAt: readI64(values, 'lease_expires_at') || undefined,
    revision: readI64(values, 'revision'),
  };
}
