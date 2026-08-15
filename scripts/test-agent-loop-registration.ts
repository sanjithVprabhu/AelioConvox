import assert from 'node:assert/strict';
import type { RegisterMessage, ResultMessage } from '@aelio/protocol';
import { registrationFingerprint } from '../sdk/node/src/index.ts';
import { buildAgentCatalog } from '../server/src/aelio-agent-catalog.ts';
import { agentUserIdFromSdkCustomerId, stableAgentUserId } from '../server/src/conversation-turn.ts';
import { catalogContentFingerprint } from '../server/src/sdk-registration-log.ts';
import { ServerSdkBridge } from '../server/src/sdk-bridge.ts';

async function main(): Promise<void> {
function registration(description = 'Start a shopping session'): RegisterMessage {
  return {
    type: 'register',
    sdkVersion: 'test',
    language: 'node',
    application: 'Shop test',
    functions: [
      {
        name: 'startShoppingSession',
        description,
        params: { email: 'string' },
        safety: 'write',
      },
      {
        name: 'addToCart',
        description: 'Add a product to the active cart',
        params: { productId: 'string' },
        safety: 'write',
      },
    ],
    states: [
      {
        id: 'anonymous',
        description: 'May start a session',
        allowedTools: ['startShoppingSession'],
        transitions: [{ on_tool_success: 'startShoppingSession', to: 'browsing' }],
      },
      {
        id: 'browsing',
        description: 'May add products',
        allowedTools: ['addToCart'],
      },
    ],
  };
}

const first = registration();
const changed = registration('Create or resume a verified shopping session');
assert.notEqual(registrationFingerprint(first), registrationFingerprint(changed));
assert.notEqual(catalogContentFingerprint(first), catalogContentFingerprint(changed));

const catalog = buildAgentCatalog('shop-test', first) as {
  states: Array<{ id: string; exit_edges: Array<Record<string, unknown>> }>;
  policies: Array<{ id: string; action?: { transition?: string } }>;
};
const anonymous = catalog.states.find((state) => state.id === 'anonymous');
assert.ok(anonymous);
assert.equal(anonymous.exit_edges.length, 1);
assert.equal(anonymous.exit_edges[0]?.to, 'browsing');
assert.ok(
  catalog.policies.some((policy) => policy.action?.transition === 'anonymous->browsing'),
  'declared lifecycle transition must receive an exact allow policy',
);

const externalId = 'shopper@example.test';
const agentId = stableAgentUserId(externalId);
assert.equal(agentUserIdFromSdkCustomerId(agentId), agentId, 'agent id must not be hashed twice');
assert.equal(agentUserIdFromSdkCustomerId(externalId), agentId);

const calls: string[] = [];
let bridge: ServerSdkBridge;
const socket = {
  readyState: 1,
  close() {},
  send(raw: string) {
    const invoke = JSON.parse(raw) as { id: string; function: string };
    calls.push(invoke.function);
    const result: ResultMessage = {
      type: 'result',
      id: invoke.id,
      ok: true,
      data: { function: invoke.function },
      durationMs: 1,
    };
    if (invoke.id.startsWith('agent-tool-new-session') && invoke.function === 'startShoppingSession') {
      bridge.trackUserStateCommand(agentId, {
        commandId: 'sdk-state:test',
        stateId: 'browsing',
        reason: 'Shopping session started',
      });
    }
    queueMicrotask(() => bridge.handleResult(result));
  },
};
bridge = new ServerSdkBridge({
  async pruneStale() {},
  async upsert() {},
  async remove() {},
} as never);
bridge.register({
  id: 'shop-connection',
  socket: socket as never,
  application: 'Shop test',
  functions: first.functions,
  states: first.states ?? [],
  policies: [],
  flows: [],
  persona: null,
  productBrief: null,
  sdkVersion: 'test',
  language: 'node',
  canSend: false,
  catalogFingerprint: catalogContentFingerprint(first),
  connectedAt: Date.now(),
  lastHeartbeatAt: Date.now(),
});

const sharedLegacyCorrelation = 't0:proxy_call';
const [start, cart] = await Promise.all([
  bridge.invokeCorrelated(
    'startShoppingSession',
    { email: externalId },
    { customerId: agentId, sessionId: agentId, channel: 'web', channelAddress: `web:${agentId}` },
    sharedLegacyCorrelation,
  ),
  bridge.invokeCorrelated(
    'addToCart',
    { productId: 'prod-1' },
    { customerId: agentId, sessionId: agentId, channel: 'web', channelAddress: `web:${agentId}` },
    sharedLegacyCorrelation,
  ),
]);
assert.deepEqual(calls.sort(), ['addToCart', 'startShoppingSession']);
assert.deepEqual(start.data, { function: 'startShoppingSession' });
assert.deepEqual(cart.data, { function: 'addToCart' });

const lifecycleBoundResult = await bridge.invokeCorrelated(
  'startShoppingSession',
  { email: externalId },
  { customerId: agentId, sessionId: agentId, channel: 'web', channelAddress: `web:${agentId}` },
  'agent-tool-new-session',
);
assert.equal(lifecycleBoundResult.ok, true);
assert.deepEqual(lifecycleBoundResult.lifecycleCommand, {
  commandId: 'sdk-state:test',
  stateId: 'browsing',
  reason: 'Shopping session started',
});
assert.deepEqual(
  (await bridge.invokeCorrelated(
    'startShoppingSession',
    { email: externalId },
    { customerId: agentId, sessionId: agentId, channel: 'web', channelAddress: `web:${agentId}` },
    'agent-tool-new-session',
  )).lifecycleCommand,
  lifecycleBoundResult.lifecycleCommand,
  'successful replay must retain the lifecycle command until Rust commits it idempotently',
);
bridge.shutdown();

console.log('agent-loop SDK registration/lifecycle/correlation contract: PASS');
}

void main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
