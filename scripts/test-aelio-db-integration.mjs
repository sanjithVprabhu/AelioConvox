#!/usr/bin/env node
/**
 * End-to-end AelioDb integration test (AelioDb-only — no SQLite):
 * 1) aelio-server health + table schemas
 * 2) bootstrapAelioDbTables + Convox stores
 * 3) processTurn writes L0 rows + session/customer state to AelioDb
 * 4) AelioDb loadHistory returns the conversation
 */

import {
  bootstrapAelioDbTables,
  createConvoxCustomerStore,
  createConvoxFunctionCallStore,
  createConvoxMessageStore,
  createConvoxSessionStore,
  processTurn,
} from '@aelio/core';
import { createLLMProviderChain } from '@aelio/llm';
import { AelioDbClient } from '@aelio/db-client';

const AELIO_DB_URL = process.env.AELIO_DB_URL ?? 'http://127.0.0.1:18080';

const TABLES = {
  messages: 'convox_messages',
  conversations: 'convox_conversations',
  memories: 'convox_memories',
  compactions: 'convox_compactions',
  runtimeState: 'runtime_state',
  harnessTools: 'harness_tools',
  harnessCapabilities: 'harness_capabilities',
  harnessBindings: 'harness_bindings',
  harnessSuspensions: 'harness_suspensions',
  harnessLedger: 'harness_ledger',
  harnessTraces: 'harness_traces',
  customers: 'convox_customers',
  channelAddresses: 'convox_channel_addresses',
  sessions: 'convox_sessions',
  jobQueue: 'convox_job_queue',
  responseCache: 'convox_response_cache',
  functionCalls: 'convox_function_calls',
  turnApiCalls: 'convox_turn_api_calls',
  reflections: 'convox_reflections',
  proactiveMessages: 'convox_proactive_messages',
  inboundDedup: 'convox_inbound_dedup',
  magicLinks: 'convox_magic_links',
  sdkConnections: 'convox_sdk_connections',
  archetypes: 'convox_archetypes', aspects: 'convox_aspects', axisNodes: 'convox_axis_nodes',
};

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exit(1);
}

function ok(message) {
  console.log(`OK: ${message}`);
}

async function fetchJson(url, options = {}) {
  const response = await fetch(url, options);
  const text = await response.text();
  let body;
  try {
    body = text ? JSON.parse(text) : null;
  } catch {
    body = text;
  }
  return { status: response.status, body };
}

async function main() {
  // --- 1. aelio-server health ---
  const llHealth = await fetchJson(`${AELIO_DB_URL}/v1/health`);
  if (llHealth.status !== 200 || llHealth.body?.status !== 'ok') {
    fail(`aelio-server not healthy at ${AELIO_DB_URL} (status ${llHealth.status})`);
  }
  ok(`aelio-server health at ${AELIO_DB_URL}`);

  // --- 2. Bootstrap tables + stores ---
  const client = new AelioDbClient({ baseUrl: AELIO_DB_URL });
  await bootstrapAelioDbTables(client, TABLES, 1536);

  for (const table of Object.values(TABLES)) {
    const schema = await client.getSchema(table);
    ok(`table exists: ${table} (${schema.columns.length} columns)`);
  }

  const storageConfig = { client, tables: TABLES, embedDim: 1536 };
  const messageStore = createConvoxMessageStore(storageConfig);
  const sessionStore = createConvoxSessionStore(storageConfig);
  const customerStore = createConvoxCustomerStore(storageConfig);
  const functionCallStore = createConvoxFunctionCallStore(storageConfig);
  ok('Convox stores ready');

  // --- 3. processTurn ---
  const llm = createLLMProviderChain([
    { provider: 'mock', model: 'mock-model', maxTokens: 4096 },
  ]);

  const sdk = {
    getFunctions: () => [],
    getStates: () => [],
    getPolicies: () => [],
    getFlows: () => [],
    invoke: async () => ({ ok: true, data: {}, durationMs: 0 }),
  };

  const sessionTag = `aelioDb-test-${Date.now()}`;
  const customerId = `cust-${sessionTag}`;
  const userMessage = `AelioDb integration ping ${sessionTag}`;

  const { reply } = await processTurn({
    llm,
    sdk,
    model: 'mock-model',
    maxTokens: 4096,
    historyWindow: 20,
    safety: { defaultMode: 'full', requireConfirmationFor: ['write', 'destructive'] },
    identity: { mappingFunction: 'phone', allowAnonymous: true },
    rateLimit: { perCustomerPerMinute: 100, perCustomerPerDay: 1000 },
    idleTimeoutMinutes: 60,
    summarizeAfter: 50,
    memoryRecallLimit: 5,
    memoryEnabled: false,
    intent: { enabled: true, ttlMinutes: 20, maxDepth: 5 },
    messageStore,
    sessionStore,
    customerStore,
    functionCallStore,
    customerExternalId: customerId,
    channel: 'web',
    channelAddress: `web:${customerId}`,
    message: userMessage,
  });

  if (!reply || typeof reply !== 'string') {
    fail('processTurn returned empty reply');
  }
  ok(`processTurn reply (${reply.length} chars)`);

  const customer = await customerStore.getByExternalId(customerId);
  if (!customer) {
    fail('AelioDb customer record missing after processTurn');
  }
  ok(`AelioDb customer ${customer.id}`);

  const session = await sessionStore.findOrCreate(customer.id, 'web', 60);
  const sessionId = session.id;
  if (!sessionId) {
    fail('AelioDb session record missing after processTurn');
  }
  ok(`AelioDb session ${sessionId}`);

  // --- 4. Verify AelioDb L0 rows ---
  const scan = await client.scanRows(TABLES.messages, {
    k: 50,
    filters: [
      { col: 'customer_id', op: 'eq', value: { type: 'utf8', value: customer.id } },
      { col: 'tier', op: 'eq', value: { type: 'i64', value: 0 } },
    ],
  });

  const rows = scan.rows ?? [];
  const contents = rows.map((row) => row.values.content?.value ?? '').filter(Boolean);
  const roles = rows.map((row) => row.values.role?.value ?? '');

  if (rows.length < 2) {
    fail(`expected >=2 L0 rows in AelioDb, got ${rows.length}`);
  }
  if (!contents.some((c) => c.includes(sessionTag))) {
    fail(`user message not found in AelioDb: ${JSON.stringify(contents)}`);
  }
  if (!roles.includes('user') || !roles.includes('assistant')) {
    fail(`expected user+assistant roles, got ${JSON.stringify(roles)}`);
  }
  ok(`AelioDb L0 rows: ${rows.length} (user + assistant)`);

  // --- 5. loadHistory from AelioDb ---
  const history = await messageStore.loadHistory(sessionId, 20);
  if (history.length < 2) {
    fail(`AelioDb loadHistory expected >=2, got ${history.length}`);
  }
  if (!history.some((row) => row.content.includes(sessionTag))) {
    fail('AelioDb loadHistory missing user message');
  }
  ok(`AelioDb loadHistory: ${history.length} messages`);

  // --- 6. Rich conversation archive ---
  const convoScan = await client.scanRows(TABLES.conversations, {
    k: 50,
    filters: [
      { col: 'session_id', op: 'eq', value: { type: 'utf8', value: sessionId } },
    ],
  });
  const convoRows = convoScan.rows ?? [];
  if (convoRows.length < 2) {
    fail(`convox_conversations expected >=2 rows, got ${convoRows.length}`);
  }
  const userRow = convoRows.find((row) => row.values.role?.value === 'user');
  const assistantRow = convoRows.find((row) => row.values.role?.value === 'assistant');
  if (!userRow?.values.customer_external_id?.value) {
    fail('convox_conversations missing customer_external_id');
  }
  if (!assistantRow?.values.intent_label?.value) {
    fail('convox_conversations assistant row missing intent_label');
  }
  ok(`convox_conversations: ${convoRows.length} rows with sender + intent context`);

  console.log('\nAll AelioDb integration checks passed.');
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
