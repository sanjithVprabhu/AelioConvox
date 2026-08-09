import type { ApiValue } from '@aelio/db-client';
import { i64, parseJson, readI64, readUtf8, utf8 } from './helpers.js';
import type { AelioDbStorageConfig } from './types.js';

const SCAN_CAP = 5_000;

export type CatalogEntityKind = 'tool' | 'state' | 'policy' | 'flow';

export type CatalogEntityRecord = {
  kind: CatalogEntityKind;
  entityId: string;
  tenant: string;
  application: string;
  active: boolean;
  payload: Record<string, unknown>;
  connectionId: string;
  updatedAt: number;
};

export type CatalogSyncInput = {
  tenant: string;
  application: string;
  connectionId: string;
  tools: Array<Record<string, unknown> & { name: string }>;
  states: Array<Record<string, unknown> & { id: string }>;
  policies: Array<Record<string, unknown> & { id: string }>;
  flows: Array<Record<string, unknown> & { id: string }>;
};

export type CatalogSyncResult = {
  activated: { tools: string[]; states: string[]; flows: string[]; policies: string[] };
  deactivated: { tools: string[]; states: string[]; flows: string[]; policies: string[] };
};

/**
 * One durable row per (tenant, kind, entity_id). Soft-delete via `active`:
 * present in the latest SDK register → active=1; missing → active=0.
 * Reads for the live frame of reference always filter active=1.
 */
export class ConvoxCatalogEntityStore {
  readonly client: AelioDbStorageConfig['client'];
  readonly table: string;

  constructor(config: AelioDbStorageConfig) {
    this.client = config.client;
    this.table = config.tables.sdkCatalog;
  }

  async syncSnapshot(input: CatalogSyncInput): Promise<CatalogSyncResult> {
    const now = Date.now();
    const desired = new Map<string, CatalogEntityRecord>();

    for (const tool of input.tools) {
      desired.set(key('tool', tool.name), {
        kind: 'tool',
        entityId: tool.name,
        tenant: input.tenant,
        application: input.application,
        active: true,
        payload: tool,
        connectionId: input.connectionId,
        updatedAt: now,
      });
    }
    for (const state of input.states) {
      desired.set(key('state', state.id), {
        kind: 'state',
        entityId: state.id,
        tenant: input.tenant,
        application: input.application,
        active: true,
        payload: state,
        connectionId: input.connectionId,
        updatedAt: now,
      });
    }
    for (const policy of input.policies) {
      desired.set(key('policy', policy.id), {
        kind: 'policy',
        entityId: policy.id,
        tenant: input.tenant,
        application: input.application,
        active: true,
        payload: policy,
        connectionId: input.connectionId,
        updatedAt: now,
      });
    }
    for (const flow of input.flows) {
      desired.set(key('flow', flow.id), {
        kind: 'flow',
        entityId: flow.id,
        tenant: input.tenant,
        application: input.application,
        active: true,
        payload: flow,
        connectionId: input.connectionId,
        updatedAt: now,
      });
    }

    const existing = await this.listAllForTenant(input.tenant);
    const activated = emptyBuckets();
    const deactivated = emptyBuckets();

    for (const [mapKey, record] of desired) {
      const prev = existing.get(mapKey);
      await this.upsert(record);
      if (!prev?.active) {
        pushBucket(activated, record.kind, record.entityId);
      }
    }

    for (const [mapKey, prev] of existing) {
      if (!prev.active) continue;
      if (desired.has(mapKey)) continue;
      // Soft-delete only this application's prior catalog so multi-app tenants
      // do not clobber each other on register.
      if (prev.application !== input.application) continue;
      await this.upsert({
        ...prev,
        active: false,
        application: input.application,
        connectionId: input.connectionId,
        updatedAt: now,
      });
      pushBucket(deactivated, prev.kind, prev.entityId);
    }

    return { activated, deactivated };
  }

  async listActive(tenant: string, kind?: CatalogEntityKind): Promise<CatalogEntityRecord[]> {
    const filters: Array<{ col: string; op: 'eq'; value: ReturnType<typeof utf8> | ReturnType<typeof i64> }> = [
      { col: 'tenant', op: 'eq', value: utf8(tenant) },
      { col: 'active', op: 'eq', value: i64(1) },
    ];
    if (kind) {
      filters.push({ col: 'kind', op: 'eq', value: utf8(kind) });
    }
    const scan = await this.client.scanRows(this.table, { k: SCAN_CAP, filters });
    return scan.rows.map((row) => rowToRecord(row.values)).filter((r): r is CatalogEntityRecord => Boolean(r));
  }

  async listActiveBag(tenant: string): Promise<{
    tools: CatalogEntityRecord[];
    states: CatalogEntityRecord[];
    policies: CatalogEntityRecord[];
    flows: CatalogEntityRecord[];
  }> {
    const all = await this.listActive(tenant);
    return {
      tools: all.filter((r) => r.kind === 'tool'),
      states: all.filter((r) => r.kind === 'state'),
      policies: all.filter((r) => r.kind === 'policy'),
      flows: all.filter((r) => r.kind === 'flow'),
    };
  }

  private async listAllForTenant(tenant: string): Promise<Map<string, CatalogEntityRecord>> {
    const scan = await this.client.scanRows(this.table, {
      k: SCAN_CAP,
      filters: [{ col: 'tenant', op: 'eq', value: utf8(tenant) }],
    });
    const out = new Map<string, CatalogEntityRecord>();
    for (const row of scan.rows) {
      const record = rowToRecord(row.values);
      if (!record) continue;
      out.set(key(record.kind, record.entityId), record);
    }
    return out;
  }

  private async find(tenant: string, kind: CatalogEntityKind, entityId: string) {
    const scan = await this.client.scanRows(this.table, {
      k: 1,
      filters: [
        { col: 'tenant', op: 'eq', value: utf8(tenant) },
        { col: 'kind', op: 'eq', value: utf8(kind) },
        { col: 'entity_id', op: 'eq', value: utf8(entityId) },
      ],
    });
    return scan.rows[0] ?? null;
  }

  private async upsert(record: CatalogEntityRecord): Promise<void> {
    const values = {
      tenant: utf8(record.tenant),
      kind: utf8(record.kind),
      entity_id: utf8(record.entityId),
      application: utf8(record.application),
      active: i64(record.active ? 1 : 0),
      payload_json: utf8(JSON.stringify(record.payload)),
      connection_id: utf8(record.connectionId),
      updated_at: i64(record.updatedAt),
    };
    const existing = await this.find(record.tenant, record.kind, record.entityId);
    if (existing) {
      await this.client.updateRow(this.table, existing.row_id, values);
    } else {
      await this.client.insertRow(this.table, values);
    }
  }
}

export function createConvoxCatalogEntityStore(
  config: AelioDbStorageConfig,
): ConvoxCatalogEntityStore {
  return new ConvoxCatalogEntityStore(config);
}

function key(kind: CatalogEntityKind, entityId: string): string {
  return `${kind}:${entityId}`;
}

function emptyBuckets() {
  return { tools: [] as string[], states: [] as string[], flows: [] as string[], policies: [] as string[] };
}

function pushBucket(
  buckets: ReturnType<typeof emptyBuckets>,
  kind: CatalogEntityKind,
  entityId: string,
): void {
  if (kind === 'tool') buckets.tools.push(entityId);
  else if (kind === 'state') buckets.states.push(entityId);
  else if (kind === 'flow') buckets.flows.push(entityId);
  else buckets.policies.push(entityId);
}

function rowToRecord(values: Record<string, ApiValue>): CatalogEntityRecord | null {
  const kindRaw = readUtf8(values, 'kind');
  if (kindRaw !== 'tool' && kindRaw !== 'state' && kindRaw !== 'policy' && kindRaw !== 'flow') {
    return null;
  }
  return {
    kind: kindRaw,
    entityId: readUtf8(values, 'entity_id'),
    tenant: readUtf8(values, 'tenant'),
    application: readUtf8(values, 'application'),
    active: readI64(values, 'active') === 1,
    payload: parseJson(readUtf8(values, 'payload_json'), {}),
    connectionId: readUtf8(values, 'connection_id'),
    updatedAt: readI64(values, 'updated_at'),
  };
}
