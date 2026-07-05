#!/usr/bin/env node
/**
 * End-to-end Sunjet integration test:
 * 1) ll-server health + table schemas
 * 2) bootstrapSunjetTables + ConvoxMessageStore
 * 3) processTurn writes L0 rows to Sunjet (+ SQLite dual-write)
 * 4) Sunjet loadHistory returns the conversation
 */

import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createDatabase, customers, messages, sessions } from '@aelio/db';
import {
  bootstrapSunjetTables,
  createConvoxMessageStore,
  processTurn,
} from '@aelio/core';
import { createLLMProviderChain } from '@aelio/llm';
import { SunjetClient } from '@aelio/sunjet-client';
import { desc, eq } from 'drizzle-orm';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const MIGRATIONS = join(ROOT, 'packages/db/drizzle');

const SUNJET_URL = process.env.SUNJET_URL ?? 'http://127.0.0.1:18080';

const TABLES = {
  messages: 'convox_messages',
  conversations: 'convox_conversations',
  memories: 'convox_memories',
  compactions: 'convox_compactions',
  runtimeState: 'runtime_state',
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
  // --- 1. ll-server health ---
  const llHealth = await fetchJson(`${SUNJET_URL}/v1/health`);
  if (llHealth.status !== 200 || llHealth.body?.status !== 'ok') {
    fail(`ll-server not healthy at ${SUNJET_URL} (status ${llHealth.status})`);
  }
  ok(`ll-server health at ${SUNJET_URL}`);

  // --- 2. Bootstrap tables + message store ---
  const client = new SunjetClient({ baseUrl: SUNJET_URL });
  await bootstrapSunjetTables(client, TABLES, 1536);

  for (const table of Object.values(TABLES)) {
    const schema = await client.getSchema(table);
    ok(`table exists: ${table} (${schema.columns.length} columns)`);
  }

  const messageStore = createConvoxMessageStore({
    client,
    tables: TABLES,
    embedDim: 1536,
    dualWriteSqlite: true,
    fallbackSqliteOnError: true,
  });
  ok('ConvoxMessageStore ready');

  // --- 3. processTurn ---
  const dbDir = mkdtempSync(join(tmpdir(), 'aelio-sunjet-'));
  const database = createDatabase(join(dbDir, 'test.db'));
  database.migrate(MIGRATIONS);

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

  const sessionTag = `sunjet-test-${Date.now()}`;
  const customerId = `cust-${sessionTag}`;
  const userMessage = `Sunjet integration ping ${sessionTag}`;

  const { reply } = await processTurn({
    database,
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
    memoryEnabled: true,
    intent: { enabled: true, ttlMinutes: 20, maxDepth: 5 },
    messageStore,
    customerExternalId: customerId,
    channel: 'web',
    channelAddress: `web:${customerId}`,
    message: userMessage,
  });

  if (!reply || typeof reply !== 'string') {
    fail('processTurn returned empty reply');
  }
  ok(`processTurn reply (${reply.length} chars)`);

  const customerRows = await database.db
    .select()
    .from(customers)
    .where(eq(customers.externalId, customerId))
    .limit(1);
  const customerInternalId = customerRows[0]?.id;
  if (!customerInternalId) {
    fail('sqlite customer row missing after processTurn');
  }

  const sessionRows = await database.db
    .select()
    .from(sessions)
    .where(eq(sessions.customerId, customerInternalId))
    .orderBy(desc(sessions.lastActivityAt))
    .limit(1);
  const sessionId = sessionRows[0]?.id;
  if (!sessionId) {
    fail('sqlite session row missing after processTurn');
  }
  ok(`sqlite session ${sessionId}`);

  // --- 4. Verify Sunjet L0 rows ---
  const scan = await client.scanRows(TABLES.messages, {
    k: 50,
    filters: [
      { col: 'customer_id', op: 'eq', value: { type: 'utf8', value: customerInternalId } },
      { col: 'tier', op: 'eq', value: { type: 'i64', value: 0 } },
    ],
  });

  const rows = scan.rows ?? [];
  const contents = rows.map((row) => row.values.content?.value ?? '').filter(Boolean);
  const roles = rows.map((row) => row.values.role?.value ?? '');

  if (rows.length < 2) {
    fail(`expected >=2 L0 rows in Sunjet, got ${rows.length}`);
  }
  if (!contents.some((c) => c.includes(sessionTag))) {
    fail(`user message not found in Sunjet: ${JSON.stringify(contents)}`);
  }
  if (!roles.includes('user') || !roles.includes('assistant')) {
    fail(`expected user+assistant roles, got ${JSON.stringify(roles)}`);
  }
  ok(`Sunjet L0 rows: ${rows.length} (user + assistant)`);

  // --- 5. loadHistory from Sunjet ---
  const history = await messageStore.loadHistory(sessionId, 20);
  if (history.length < 2) {
    fail(`Sunjet loadHistory expected >=2, got ${history.length}`);
  }
  if (!history.some((row) => row.content.includes(sessionTag))) {
    fail('Sunjet loadHistory missing user message');
  }
  ok(`Sunjet loadHistory: ${history.length} messages`);

  // --- 6. SQLite dual-write ---
  const sqliteMessages = await database.db
    .select()
    .from(messages)
    .where(eq(messages.sessionId, sessionId));
  if (sqliteMessages.length < 2) {
    fail(`SQLite dual-write expected >=2, got ${sqliteMessages.length}`);
  }
  ok(`SQLite dual-write: ${sqliteMessages.length} messages`);

  // --- 7. Rich conversation archive ---
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

  database.close();

  console.log('\nAll Sunjet integration checks passed.');
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});