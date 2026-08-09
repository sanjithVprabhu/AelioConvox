/**
 * Smoke: soft-delete + active-only for tools, states, policies, flows.
 *
 *   node scripts/verify-catalog-soft-delete.mjs
 */
import { AelioDbClient } from '@aelio/db-client';
import { createConvoxCatalogEntityStore } from '@aelio/core/edge';

const baseUrl = process.env.AELIO_DB_URL || 'http://127.0.0.1:8090';
const apiKey =
  process.env.AELIO_DB_API_KEY
  || process.env.AELIO_RUNTIME_TOKEN
  || process.env.DB_API_KEY
  || 'aelio-local-internal-token-32-chars';
const tenant = `catalog-soft-delete-${Date.now()}`;
const table = `convox_sdk_catalog_verify_${Date.now()}`;

function assert(cond, msg) {
  if (!cond) throw new Error(msg);
}

const client = new AelioDbClient({ baseUrl, apiKey, timeoutMs: 15_000 });
await client.health();

await client.ensureTable(table, [
  { name: 'tenant', kind: 'utf8' },
  { name: 'kind', kind: 'utf8' },
  { name: 'entity_id', kind: 'utf8' },
  { name: 'application', kind: 'utf8' },
  { name: 'active', kind: 'i64' },
  { name: 'payload_json', kind: 'text' },
  { name: 'connection_id', kind: 'utf8' },
  { name: 'updated_at', kind: 'i64' },
]);

const tables = {
  messages: 'x', conversations: 'x', memories: 'x', compactions: 'x',
  runtimeState: 'x', harnessTools: 'x', harnessCapabilities: 'x',
  harnessBindings: 'x', harnessSuspensions: 'x', harnessLedger: 'x',
  harnessTraces: 'x', customers: 'x', channelAddresses: 'x', sessions: 'x',
  jobQueue: 'x', responseCache: 'x', functionCalls: 'x', turnApiCalls: 'x',
  reflections: 'x', proactiveMessages: 'x', inboundDedup: 'x', magicLinks: 'x',
  sdkConnections: 'x', sdkCatalog: table, archetypes: 'x', aspects: 'x',
  axisNodes: 'x',
};

const store = createConvoxCatalogEntityStore({ client, tables, embedDim: 8 });
const app = 'verify-app';

const full = {
  tenant,
  application: app,
  connectionId: 'c1',
  tools: [
    { name: 'tool_keep', description: 'stays' },
    { name: 'tool_drop', description: 'goes' },
  ],
  states: [
    { id: 'state_keep' },
    { id: 'state_drop' },
  ],
  policies: [
    { id: 'policy_keep' },
    { id: 'policy_drop' },
  ],
  flows: [
    { id: 'flow_keep' },
    { id: 'flow_drop' },
  ],
};

const r1 = await store.syncSnapshot(full);
for (const kind of ['tools', 'states', 'policies', 'flows']) {
  assert(r1.activated[kind].length === 2, `${kind}: expected 2 activated, got ${JSON.stringify(r1.activated[kind])}`);
}

const bag1 = await store.listActiveBag(tenant);
assert(bag1.tools.length === 2, `tools active=${bag1.tools.length}`);
assert(bag1.states.length === 2, `states active=${bag1.states.length}`);
assert(bag1.policies.length === 2, `policies active=${bag1.policies.length}`);
assert(bag1.flows.length === 2, `flows active=${bag1.flows.length}`);

const r2 = await store.syncSnapshot({
  ...full,
  tools: [{ name: 'tool_keep', description: 'stays' }],
  states: [{ id: 'state_keep' }],
  policies: [{ id: 'policy_keep' }],
  flows: [{ id: 'flow_keep' }],
});

assert(r2.deactivated.tools.includes('tool_drop'), `tools deactivate: ${JSON.stringify(r2.deactivated.tools)}`);
assert(r2.deactivated.states.includes('state_drop'), `states deactivate: ${JSON.stringify(r2.deactivated.states)}`);
assert(r2.deactivated.policies.includes('policy_drop'), `policies deactivate: ${JSON.stringify(r2.deactivated.policies)}`);
assert(r2.deactivated.flows.includes('flow_drop'), `flows deactivate: ${JSON.stringify(r2.deactivated.flows)}`);

const bag2 = await store.listActiveBag(tenant);
assert(bag2.tools.map((r) => r.entityId).join() === 'tool_keep', `tools now=${bag2.tools.map((r) => r.entityId)}`);
assert(bag2.states.map((r) => r.entityId).join() === 'state_keep', `states now=${bag2.states.map((r) => r.entityId)}`);
assert(bag2.policies.map((r) => r.entityId).join() === 'policy_keep', `policies now=${bag2.policies.map((r) => r.entityId)}`);
assert(bag2.flows.map((r) => r.entityId).join() === 'flow_keep', `flows now=${bag2.flows.map((r) => r.entityId)}`);

// Inactive rows still exist in DB (soft-delete), but listActive must exclude them.
const allScan = await client.scanRows(table, {
  k: 100,
  filters: [{ col: 'tenant', op: 'eq', value: { type: 'utf8', value: tenant } }],
});
assert(allScan.rows.length === 8, `expected 8 durable rows (4 active + 4 inactive), got ${allScan.rows.length}`);
const inactive = allScan.rows.filter((row) => row.values.active?.value === 0 || row.values.active?.value === 0n);
// active is i64 — value should be 0
const inactiveCount = allScan.rows.filter((row) => Number(row.values.active?.value ?? 1) === 0).length;
assert(inactiveCount === 4, `expected 4 inactive rows, got ${inactiveCount}`);

// Reactivate a previously dropped tool
const r3 = await store.syncSnapshot({
  ...full,
  tools: [
    { name: 'tool_keep', description: 'stays' },
    { name: 'tool_drop', description: 'back' },
  ],
  states: [{ id: 'state_keep' }],
  policies: [{ id: 'policy_keep' }],
  flows: [{ id: 'flow_keep' }],
});
assert(r3.activated.tools.includes('tool_drop'), 'tool_drop should reactivate');
assert((await store.listActive(tenant, 'tool')).length === 2, 'both tools active after reactivate');

// Application isolation: other app catalog must not be soft-deleted
await store.syncSnapshot({
  tenant,
  application: 'other-app',
  connectionId: 'c2',
  tools: [{ name: 'other_tool' }],
  states: [],
  policies: [],
  flows: [],
});
await store.syncSnapshot({
  tenant,
  application: 'verify-app',
  connectionId: 'c1',
  tools: [{ name: 'tool_keep' }],
  states: [{ id: 'state_keep' }],
  policies: [{ id: 'policy_keep' }],
  flows: [{ id: 'flow_keep' }],
});
const otherStillActive = (await store.listActive(tenant, 'tool')).some((r) => r.entityId === 'other_tool');
assert(otherStillActive, 'other-app tool must remain active when verify-app syncs');

console.log('PASS: tools + states + policies + flows soft-delete, active-only pull, reactivate, app isolation');
process.exit(0);
