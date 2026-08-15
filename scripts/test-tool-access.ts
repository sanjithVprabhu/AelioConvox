import assert from 'node:assert/strict';
import {
  RegisterMessageSchema,
  expandToolAccess,
  type FunctionDefinition,
  type StateDefinition,
} from '../packages/protocol/src/index.ts';
import { evaluateGate, filterFunctionsByState } from '@aelio/core';
import { Aelio } from '../sdk/node/src/index.ts';

const functions: FunctionDefinition[] = [
  { name: 'listOrders', description: 'List orders', params: {}, safety: 'read', intent: 'order_inquiry' },
  { name: 'getSubscription', description: 'Read subscription', params: {}, safety: 'read', intent: 'subscription' },
  { name: 'upgradePlan', description: 'Upgrade subscription', params: {}, safety: 'write', intent: 'subscription' },
  { name: 'cancelOrder', description: 'Cancel an order', params: {}, safety: 'destructive', intent: 'cancellation' },
  { name: 'specialRead', description: 'One exact exception', params: {}, safety: 'read', intent: 'internal' },
  { name: 'browseCatalog', description: 'Browse catalog', params: {}, safety: 'read', intent: 'catalog' },
];

const onboarding: StateDefinition = {
  id: 'onboarding',
  description: 'Read-only account exploration',
  allowedTools: ['specialRead'],
  allowedIntents: ['order_inquiry', 'subscription'],
  blockedSafety: ['write', 'destructive'],
};

assert.deepEqual(
  filterFunctionsByState(functions, onboarding).map((fn) => fn.name),
  ['listOrders', 'getSubscription', 'specialRead'],
  'name and intent allow rules should compose, while safety blocks win',
);

for (const fn of functions) {
  const selected = filterFunctionsByState([fn], onboarding).length === 1;
  const verdict = evaluateGate(fn, {}, {
    state: onboarding,
    safety: { defaultMode: 'full', requireConfirmationFor: [] },
  });
  assert.equal(
    verdict.verdict === 'allow',
    selected,
    `retrieval and executor gates disagree for ${fn.name}`,
  );
}

const protocolResult = RegisterMessageSchema.safeParse({
  type: 'register',
  sdkVersion: 'test',
  language: 'node',
  functions,
  states: [onboarding],
  pipeline: {
    initial_stage: 'onboarding',
    stages: {
      onboarding: {
        id: 'onboarding',
        description: 'Read-only account exploration',
        allowedIntents: ['order_inquiry', 'subscription'],
        blockedSafety: ['write', 'destructive'],
      },
    },
  },
});
assert.equal(protocolResult.success, true, 'protocol must accept compact access selectors');

const sdk = new Aelio();
sdk.pipeline({
  initialStage: 'onboarding',
  stages: {
    onboarding: {
      description: 'Read-only account exploration',
      allowedIntents: ['order_inquiry', 'subscription'],
      blockedSafety: ['write', 'destructive'],
    },
  },
});
let registered: unknown;
const sdkInternals = sdk as unknown as {
  ws: { readyState: number; send(raw: string): void };
  listenOptions: { secret: string; sdkVersion: string };
  sendRegister(): void;
};
sdkInternals.ws = {
  readyState: 1,
  send(raw) {
    registered = JSON.parse(raw);
  },
};
sdkInternals.listenOptions = { secret: 'test', sdkVersion: 'test' };
sdkInternals.sendRegister();

const parsed = RegisterMessageSchema.parse(registered);
assert.deepEqual(parsed.states?.map((state) => state.id), ['onboarding']);
assert.deepEqual(parsed.states?.[0]?.allowedIntents, ['order_inquiry', 'subscription']);
assert.deepEqual(parsed.states?.[0]?.blockedSafety, ['write', 'destructive']);

// --- tool groups expand into flat selectors before register ---
const grouped = new Aelio();
grouped.pipeline({
  initialStage: 'browsing',
  toolGroups: {
    catalog: { intents: ['catalog'] },
    cart: { tools: ['get_cart', 'add_to_cart'] },
    writes: { safety: ['write', 'destructive'] },
  },
  stages: {
    browsing: {
      description: 'Browse and cart',
      allowedGroups: ['catalog', 'cart'],
      blockedGroups: ['writes'],
      allowedTools: ['list_orders'],
    },
    retention: {
      description: 'No upsells',
      blockedGroups: ['cart'],
      blockedTools: ['upgradePlan'],
    },
  },
});
let groupedRegistered: unknown;
const groupedInternals = grouped as unknown as {
  ws: { readyState: number; send(raw: string): void };
  listenOptions: { secret: string; sdkVersion: string };
  sendRegister(): void;
};
groupedInternals.ws = {
  readyState: 1,
  send(raw) {
    groupedRegistered = JSON.parse(raw);
  },
};
groupedInternals.listenOptions = { secret: 'test', sdkVersion: 'test' };
groupedInternals.sendRegister();
const groupedParsed = RegisterMessageSchema.parse(groupedRegistered);
const browsing = groupedParsed.states?.find((state) => state.id === 'browsing');
const retention = groupedParsed.states?.find((state) => state.id === 'retention');
assert.deepEqual(browsing?.allowedIntents, ['catalog']);
assert.deepEqual(browsing?.allowedTools?.sort(), ['add_to_cart', 'get_cart', 'list_orders']);
assert.deepEqual(browsing?.blockedSafety?.sort(), ['destructive', 'write']);
assert.deepEqual(retention?.blockedTools?.sort(), ['add_to_cart', 'get_cart', 'upgradePlan']);

assert.throws(
  () =>
    expandToolAccess(
      { catalog: { intents: ['catalog'] } },
      { allowedGroups: ['missing'] },
    ),
  /Unknown tool group "missing"/,
);

console.log('compact tool-access selectors: PASS');
console.log('tool-group expansion: PASS');
