/**
 * Process-local hot bag of active SDK catalog entities (tools / states /
 * policies / flows). Mirrors the durable soft-delete table
 * `convox_sdk_catalog` so the live frame of reference is always pullable
 * without a DB round-trip — RocksDB-like in spirit, in-memory for the edge.
 *
 * Authority:
 * - Durable truth: ConvoxCatalogEntityStore (active=1|0)
 * - Hot bag: this module (active-only projection)
 * - Invoke routing: ServerSdkBridge live sockets
 */

import type {
  CatalogEntityKind,
  CatalogEntityRecord,
  ConvoxCatalogEntityStore,
} from '@aelio/core/edge';

export type CatalogBagSnapshot = {
  tenant: string;
  application: string;
  updatedAt: number;
  tools: CatalogEntityRecord[];
  states: CatalogEntityRecord[];
  policies: CatalogEntityRecord[];
  flows: CatalogEntityRecord[];
};

type TenantBag = {
  byKind: Map<CatalogEntityKind, Map<string, CatalogEntityRecord>>;
  application: string;
  updatedAt: number;
};

const bags = new Map<string, TenantBag>();

function emptyKindMaps(): TenantBag['byKind'] {
  return new Map([
    ['tool', new Map()],
    ['state', new Map()],
    ['policy', new Map()],
    ['flow', new Map()],
  ]);
}

export function replaceCatalogBag(input: {
  tenant: string;
  application: string;
  tools: CatalogEntityRecord[];
  states: CatalogEntityRecord[];
  policies: CatalogEntityRecord[];
  flows: CatalogEntityRecord[];
}): CatalogBagSnapshot {
  const byKind = emptyKindMaps();
  for (const row of input.tools) byKind.get('tool')!.set(row.entityId, row);
  for (const row of input.states) byKind.get('state')!.set(row.entityId, row);
  for (const row of input.policies) byKind.get('policy')!.set(row.entityId, row);
  for (const row of input.flows) byKind.get('flow')!.set(row.entityId, row);
  const updatedAt = Date.now();
  bags.set(input.tenant, {
    byKind,
    application: input.application,
    updatedAt,
  });
  return getCatalogBag(input.tenant)!;
}

export function getCatalogBag(tenant: string): CatalogBagSnapshot | null {
  const bag = bags.get(tenant);
  if (!bag) return null;
  return {
    tenant,
    application: bag.application,
    updatedAt: bag.updatedAt,
    tools: [...bag.byKind.get('tool')!.values()],
    states: [...bag.byKind.get('state')!.values()],
    policies: [...bag.byKind.get('policy')!.values()],
    flows: [...bag.byKind.get('flow')!.values()],
  };
}

export function listActiveFromBag(tenant: string, kind?: CatalogEntityKind): CatalogEntityRecord[] {
  const snap = getCatalogBag(tenant);
  if (!snap) return [];
  if (!kind) {
    return [...snap.tools, ...snap.states, ...snap.policies, ...snap.flows];
  }
  if (kind === 'tool') return snap.tools;
  if (kind === 'state') return snap.states;
  if (kind === 'policy') return snap.policies;
  return snap.flows;
}

/** Hydrate the hot bag from durable active rows (boot / reconnect). */
export async function hydrateCatalogBagFromStore(
  store: ConvoxCatalogEntityStore,
  tenant: string,
): Promise<CatalogBagSnapshot> {
  const bag = await store.listActiveBag(tenant);
  return replaceCatalogBag({
    tenant,
    application: bag.tools[0]?.application
      ?? bag.states[0]?.application
      ?? bag.policies[0]?.application
      ?? bag.flows[0]?.application
      ?? tenant,
    tools: bag.tools,
    states: bag.states,
    policies: bag.policies,
    flows: bag.flows,
  });
}

/** After a durable sync, refresh the bag from active-only DB rows. */
export async function refreshCatalogBagAfterSync(
  store: ConvoxCatalogEntityStore,
  tenant: string,
  application: string,
): Promise<CatalogBagSnapshot> {
  const bag = await store.listActiveBag(tenant);
  return replaceCatalogBag({
    tenant,
    application,
    tools: bag.tools,
    states: bag.states,
    policies: bag.policies,
    flows: bag.flows,
  });
}
