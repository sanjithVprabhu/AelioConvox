#!/usr/bin/env node
/**
 * Thorough end-to-end Astrolobe database admin test:
 * - spawns ll-server + Aelio (sunjet enabled)
 * - bootstraps all 11 collections
 * - writes real conversation data via processTurn
 * - exercises admin API across multiple tables
 * - validates auth, edge cases, vector preview, catalog metadata
 */

import { spawn } from 'node:child_process';
import { createServer } from 'node:net';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { setTimeout as sleep } from 'node:timers/promises';
import { dirname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { createDatabase } from '@aelio/db';
import {
  createConvoxMessageStore,
  processTurn,
} from '@aelio/core';
import { createLLMProviderChain } from '@aelio/llm';
import { desc, eq } from 'drizzle-orm';
import { customers, sessions } from '@aelio/db';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const ASTROLOBE = join(ROOT, 'Sunjet/Astrolobe');
const LL_SERVER_BIN = join(ASTROLOBE, 'target/release/ll-server');
const MIGRATIONS = join(ROOT, 'packages/db/drizzle');
const SECRET = 'test-secret';

const EXPECTED_TABLE_KEYS = [
  'messages',
  'conversations',
  'memories',
  'compactions',
  'runtime_state',
  'harness_tools',
  'harness_capabilities',
  'harness_bindings',
  'harness_suspensions',
  'harness_ledger',
  'harness_traces',
];

let llServer = null;
let aelioServer = null;
let tmpRoot = null;
let passed = 0;

function fail(message) {
  console.error(`FAIL: ${message}`);
  cleanup(1);
}

function ok(message) {
  passed += 1;
  console.log(`OK: ${message}`);
}

function section(title) {
  console.log(`\n── ${title} ──`);
}

async function getFreePort() {
  const server = createServer();
  await new Promise((resolvePort, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolvePort);
  });
  const address = server.address();
  const port = typeof address === 'object' && address ? address.port : 0;
  await new Promise((resolveClose) => server.close(resolveClose));
  if (!port) throw new Error('Unable to allocate port');
  return port;
}

function spawnProcess(label, command, args, options = {}) {
  const child = spawn(command, args, {
    stdio: ['ignore', 'pipe', 'pipe'],
    ...options,
  });
  let output = '';
  child.stdout?.on('data', (chunk) => {
    output += chunk.toString();
  });
  child.stderr?.on('data', (chunk) => {
    output += chunk.toString();
  });
  child.on('exit', (code) => {
    if (code !== null && code !== 0 && !options.allowExit) {
      console.error(`[${label}] exited ${code}\n${output.slice(-4000)}`);
    }
  });
  return { child, getOutput: () => output };
}

async function waitForHealth(url, child, getOutput, path = '/health') {
  for (let i = 0; i < 60; i += 1) {
    if (child.exitCode !== null) {
      throw new Error(`Process exited before healthy:\n${getOutput()}`);
    }
    try {
      const response = await fetch(`${url}${path}`);
      if (response.ok) return await response.json();
    } catch {
      // retry
    }
    await sleep(500);
  }
  throw new Error(`Not healthy at ${url}${path}\n${getOutput()}`);
}

async function api(baseUrl, method, path, body, secret = SECRET) {
  const headers = {};
  if (secret !== null) {
    if (secret) headers.authorization = `Bearer ${secret}`;
  }
  if (body !== undefined) headers['content-type'] = 'application/json';
  const response = await fetch(`${baseUrl}${path}`, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  let data = {};
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = { raw: text };
    }
  }
  return { status: response.status, data, text };
}

function cleanup(code = 0) {
  for (const proc of [aelioServer, llServer]) {
    if (!proc) continue;
    try {
      proc.child.kill('SIGTERM');
    } catch {
      // Sandbox or already-exited process — ignore.
    }
  }
  aelioServer = null;
  llServer = null;
  if (tmpRoot) {
    try {
      rmSync(tmpRoot, { recursive: true, force: true });
    } catch {
      // best effort
    }
    tmpRoot = null;
  }
  process.exit(code);
}

process.on('SIGINT', () => cleanup(130));
process.on('SIGTERM', () => cleanup(143));

function fakeVector(dim) {
  return Array.from({ length: dim }, (_, i) => (i + 1) * 0.001);
}

async function main() {
  const sunjetPort = await getFreePort();
  const aelioPort = await getFreePort();
  const sunjetUrl = `http://127.0.0.1:${sunjetPort}`;
  const aelioUrl = `http://127.0.0.1:${aelioPort}`;

  tmpRoot = mkdtempSync(join(tmpdir(), 'aelio-admin-db-'));
  const llDataDir = join(tmpRoot, 'sunjet');
  const dbPath = join(tmpRoot, 'aelio.db');

  section('Infrastructure');
  llServer = spawnProcess('ll-server', LL_SERVER_BIN, [], {
    cwd: ASTROLOBE,
    env: {
      ...process.env,
      LL_DATA_DIR: llDataDir,
      LL_BIND: `127.0.0.1:${sunjetPort}`,
    },
  });
  await waitForHealth(sunjetUrl, llServer.child, llServer.getOutput, '/v1/health');
  ok(`ll-server healthy at ${sunjetUrl}`);

  const configSrc = readFileSync(join(ROOT, 'config.sunjet-test.yaml'), 'utf8');
  const configPath = join(tmpRoot, 'config.yaml');
  const testConfig = configSrc
    .replace(/url:\s*http:\/\/127\.0\.0\.1:\d+/m, `url: ${sunjetUrl}`)
    .replace(/^(\s*port:\s*)\d+\s*$/m, `$1${aelioPort}`)
    .replace(/database_path:\s.*$/m, `database_path: ${dbPath}`);
  writeFileSync(configPath, testConfig);

  aelioServer = spawnProcess('aelio', 'npx', ['tsx', 'src/main.ts'], {
    cwd: join(ROOT, 'server'),
    env: {
      ...process.env,
      AELIO_CONFIG: configPath,
      AELIO_PORT: String(aelioPort),
      AELIO_SDK_SECRET: SECRET,
      AELIO_MIGRATIONS_PATH: MIGRATIONS,
      AELIO_PUBLIC_PATH: join(ROOT, 'server/public'),
    },
  });
  await waitForHealth(aelioUrl, aelioServer.child, aelioServer.getOutput);
  ok(`Aelio server healthy at ${aelioUrl}`);

  const ready = await api(aelioUrl, 'GET', '/ready', undefined, SECRET);
  if (ready.status !== 200) {
    fail(`/ready failed: ${ready.status}`);
  }
  if (ready.data.sunjet?.messageBackend !== 'sunjet') {
    fail(`expected messageBackend=sunjet, got ${ready.data.sunjet?.messageBackend}`);
  }
  if (!ready.data.checks?.sunjet) {
    fail('/ready sunjet check failed');
  }
  ok('/ready reports sunjet message backend');

  section('UI shells');
  const dbHtml = await fetch(`${aelioUrl}/admin/db`);
  const dbPage = await dbHtml.text();
  if (dbHtml.status !== 200) fail(`/admin/db status ${dbHtml.status}`);
  for (const needle of [
    'Astrolobe Database Admin',
    '/api/v1/admin/db/tables',
    'Collections',
    'Flush',
    'Compact',
    'localStorage',
  ]) {
    if (!dbPage.includes(needle)) fail(`/admin/db missing "${needle}"`);
  }
  ok('/admin/db HTML contains dashboard UI elements');

  const telemetryHtml = await fetch(`${aelioUrl}/telemetry`);
  const telemetryPage = await telemetryHtml.text();
  if (!telemetryPage.includes('/admin/db')) {
    fail('telemetry page missing link to /admin/db');
  }
  ok('/telemetry links to database admin');

  section('Auth & access control');
  const unauth = await api(aelioUrl, 'GET', '/api/v1/admin/db/tables', undefined, '');
  if (unauth.status !== 401) fail(`missing secret: expected 401, got ${unauth.status}`);
  ok('rejects missing secret');

  const badSecret = await api(aelioUrl, 'GET', '/api/v1/admin/db/tables', undefined, 'wrong-secret');
  if (badSecret.status !== 401) fail(`wrong secret: expected 401, got ${badSecret.status}`);
  ok('rejects wrong secret');

  section('Catalog & schema');
  const catalog = await api(aelioUrl, 'GET', '/api/v1/admin/db/tables');
  if (!catalog.data.enabled) fail('catalog not enabled');
  if (catalog.data.tables.length !== EXPECTED_TABLE_KEYS.length) {
    fail(`expected ${EXPECTED_TABLE_KEYS.length} tables, got ${catalog.data.tables.length}`);
  }
  for (const key of EXPECTED_TABLE_KEYS) {
    const entry = catalog.data.tables.find((t) => t.key === key);
    if (!entry) fail(`missing catalog entry for ${key}`);
    if (!entry.exists) fail(`table ${key} (${entry.name}) not bootstrapped`);
    if (!entry.label || !entry.description || !entry.status || !entry.writtenBy) {
      fail(`catalog entry ${key} missing metadata`);
    }
    if (!Array.isArray(entry.columns) || entry.columns.length === 0) {
      fail(`catalog entry ${key} has no columns`);
    }
  }
  ok(`all ${EXPECTED_TABLE_KEYS.length} collections exist with schema + metadata`);

  const activeTables = catalog.data.tables.filter((t) => t.status === 'active');
  const schemaOnly = catalog.data.tables.filter((t) => t.status === 'schema-only');
  if (activeTables.length < 5 || schemaOnly.length < 4) {
    fail(`unexpected status mix: active=${activeTables.length}, schema-only=${schemaOnly.length}`);
  }
  ok(`catalog status badges: ${activeTables.length} active, ${schemaOnly.length} schema-only`);

  const messagesTable = catalog.data.tables.find((t) => t.key === 'messages');
  const byKey = await api(aelioUrl, 'GET', `/api/v1/admin/db/tables/messages/schema`);
  const byName = await api(
    aelioUrl,
    'GET',
    `/api/v1/admin/db/tables/${encodeURIComponent(messagesTable.name)}/schema`,
  );
  if (byKey.status !== 200 || byName.status !== 200) {
    fail('schema lookup by key/name failed');
  }
  if (byKey.data.columns.length !== byName.data.columns.length) {
    fail('schema mismatch between key and name lookup');
  }
  ok('schema resolves by table key and table name');

  const unknown = await api(aelioUrl, 'GET', '/api/v1/admin/db/tables/totally_unknown/schema');
  if (unknown.status !== 404) fail(`unknown table: expected 404, got ${unknown.status}`);
  ok('unknown table rejected');

  section('Real conversation data via processTurn');
  const database = createDatabase(dbPath);
  database.migrate(MIGRATIONS);

  const tables = {
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
  };

  // Write conversation data directly to the same ll-server instance via SunjetClient
  const { SunjetClient } = await import('@aelio/sunjet-client');
  const sunjetClient = new SunjetClient({ baseUrl: sunjetUrl });
  const store = createConvoxMessageStore({
    client: sunjetClient,
    tables,
    embedDim: 1536,
    dualWriteSqlite: true,
    fallbackSqliteOnError: true,
  });

  const llm = createLLMProviderChain([{ provider: 'mock', model: 'mock-model', maxTokens: 4096 }]);
  const sessionTag = `admin-db-thorough-${Date.now()}`;
  const customerExternalId = `cust-${sessionTag}`;

  const { reply } = await processTurn({
    database,
    llm,
    sdk: {
      getFunctions: () => [],
      getStates: () => [],
      getPolicies: () => [],
      getFlows: () => [],
      invoke: async () => ({ ok: true, data: {}, durationMs: 0 }),
    },
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
    messageStore: store,
    customerExternalId,
    channel: 'web',
    channelAddress: `web:${customerExternalId}`,
    message: `Admin DB thorough test ${sessionTag}`,
  });

  if (!reply) fail('processTurn returned empty reply');
  ok(`processTurn wrote conversation (${reply.length} char reply)`);

  const customerRows = await database.db
    .select()
    .from(customers)
    .where(eq(customers.externalId, customerExternalId))
    .limit(1);
  const sessionRows = await database.db
    .select()
    .from(sessions)
    .where(eq(sessions.customerId, customerRows[0].id))
    .orderBy(desc(sessions.lastActivityAt))
    .limit(1);
  const sessionId = sessionRows[0]?.id;
  if (!sessionId) fail('no sqlite session after processTurn');
  ok(`sqlite session ${sessionId}`);

  section('Admin scan of real data');
  const msgScan = await api(aelioUrl, 'POST', '/api/v1/admin/db/tables/messages/scan', {
    limit: 50,
    filters: [
      { col: 'session_id', op: 'eq', value: { type: 'utf8', value: sessionId } },
    ],
  });
  if (msgScan.status !== 200) fail(`messages scan failed: ${msgScan.status}`);
  if ((msgScan.data.rows ?? []).length < 2) {
    fail(`expected >=2 message rows, got ${msgScan.data.rows?.length}`);
  }
  const msgRow = msgScan.data.rows.find((r) =>
    String(r.values.content?.value ?? '').includes(sessionTag),
  );
  if (!msgRow) fail('user message not found in admin messages scan');
  ok(`messages scan: ${msgScan.data.rows.length} rows for session`);

  const convoScan = await api(aelioUrl, 'POST', '/api/v1/admin/db/tables/conversations/scan', {
    limit: 50,
    filters: [
      { col: 'session_id', op: 'eq', value: { type: 'utf8', value: sessionId } },
    ],
  });
  if (convoScan.status !== 200) fail(`conversations scan failed`);
  if ((convoScan.data.rows ?? []).length < 2) {
    fail(`expected >=2 conversation rows, got ${convoScan.data.rows?.length}`);
  }
  const assistantRow = convoScan.data.rows.find((r) => r.values.role?.value === 'assistant');
  if (!assistantRow?.values.intent_label?.value) {
    fail('conversation assistant row missing intent_label');
  }
  ok(`conversations scan: ${convoScan.data.rows.length} telemetry rows`);

  const telemetryApi = await api(
    aelioUrl,
    'GET',
    `/api/v1/telemetry/events?session_id=${encodeURIComponent(sessionId)}&limit=50`,
  );
  if (telemetryApi.status !== 200 || !telemetryApi.data.enabled) {
    fail('telemetry events API failed');
  }
  if ((telemetryApi.data.events ?? []).length < 2) {
    fail('telemetry events missing conversation data');
  }
  ok(`telemetry API returns ${telemetryApi.data.events.length} events for same session`);

  section('Vector preview in scan');
  const fullMsg = await api(
    aelioUrl,
    'GET',
    `/api/v1/admin/db/tables/messages/rows/${msgRow.row_id}`,
  );
  const embedFull = fullMsg.data.values?.embedding;
  if (embedFull?.type !== 'vector' || !Array.isArray(embedFull.value) || embedFull.value.length < 100) {
    fail('full row missing embedding vector');
  }
  const scanEmbed = msgRow.values.embedding;
  if (scanEmbed?.type !== 'vector' || scanEmbed.value.length > 10) {
    fail('scan should preview-truncate embedding');
  }
  if (!scanEmbed.__omitted || scanEmbed.__omitted < 100) {
    fail('scan embedding missing __omitted dimension count');
  }
  ok(`scan truncates embedding (${scanEmbed.value.length} shown, ${scanEmbed.__omitted} total)`);

  section('CRUD on runtime_state');
  const now = Date.now();
  const insert = await api(aelioUrl, 'POST', '/api/v1/admin/db/tables/runtime_state/rows', {
    values: {
      scope: { type: 'utf8', value: 'admin-db-test' },
      kind: { type: 'utf8', value: 'ping' },
      payload: { type: 'utf8', value: `e2e-${now}` },
      expires_at: { type: 'i64', value: now + 3_600_000 },
      updated_at: { type: 'i64', value: now },
    },
  });
  if (insert.status !== 201) fail(`insert failed: ${insert.status}`);
  const rowId = insert.data.row_id;
  ok(`inserted row #${rowId}`);

  const badBody = await api(aelioUrl, 'POST', '/api/v1/admin/db/tables/runtime_state/rows', {});
  if (badBody.status !== 400) fail(`bad insert body: expected 400, got ${badBody.status}`);
  ok('rejects insert without values');

  const badId = await api(aelioUrl, 'GET', '/api/v1/admin/db/tables/runtime_state/rows/not-a-number');
  if (badId.status !== 400) fail(`bad row id: expected 400, got ${badId.status}`);
  ok('rejects invalid row id');

  const patch = await api(
    aelioUrl,
    'PATCH',
    `/api/v1/admin/db/tables/runtime_state/rows/${rowId}`,
    { values: { payload: { type: 'utf8', value: `updated-${now}` } } },
  );
  if (!patch.data.applied) fail('update not applied');
  ok(`patched row #${rowId}`);

  const filtered = await api(aelioUrl, 'POST', '/api/v1/admin/db/tables/runtime_state/scan', {
    limit: 10,
    filters: [
      { col: 'scope', op: 'eq', value: { type: 'utf8', value: 'admin-db-test' } },
      { col: 'kind', op: 'eq', value: { type: 'utf8', value: 'ping' } },
    ],
  });
  if (!filtered.data.rows.some((r) => r.row_id === rowId)) fail('multi-filter scan missed row');
  ok('multi-filter scan finds row');

  section('CRUD on harness_traces (text + vector columns)');
  const traceInsert = await api(aelioUrl, 'POST', '/api/v1/admin/db/tables/harness_traces/rows', {
    values: {
      turn_id: { type: 'utf8', value: `turn-${now}` },
      session_id: { type: 'utf8', value: sessionId },
      tenant: { type: 'utf8', value: 'aelio-sunjet-test' },
      kind: { type: 'utf8', value: 'plan' },
      payload: { type: 'utf8', value: JSON.stringify({ test: true, tag: sessionTag }) },
      embedding: { type: 'vector', value: fakeVector(1536) },
      tier: { type: 'i64', value: 0 },
      created_at: { type: 'i64', value: now },
    },
  });
  if (traceInsert.status !== 201) fail(`harness_traces insert failed: ${traceInsert.status}`);
  const traceId = traceInsert.data.row_id;
  ok(`inserted harness_traces row #${traceId}`);

  const traceGet = await api(
    aelioUrl,
    'GET',
    `/api/v1/admin/db/tables/harness_traces/rows/${traceId}`,
  );
  if (traceGet.data.values.kind?.value !== 'plan') fail('harness trace get mismatch');
  ok(`get harness_traces row #${traceId}`);

  section('Admin maintenance (flush + compact)');
  const flush = await api(aelioUrl, 'POST', '/api/v1/admin/db/flush');
  if (!flush.data.ok) fail('flush failed');
  ok('flush');

  const compact = await api(aelioUrl, 'POST', '/api/v1/admin/db/compact');
  if (!compact.data.ok) fail('compact failed');
  ok('compact');

  const afterCompact = await api(
    aelioUrl,
    'GET',
    `/api/v1/admin/db/tables/messages/rows/${msgRow.row_id}`,
  );
  if (afterCompact.status !== 200) fail('message row lost after compact');
  ok('conversation rows survive flush/compact');

  section('Delete');
  const delTrace = await api(
    aelioUrl,
    'DELETE',
    `/api/v1/admin/db/tables/harness_traces/rows/${traceId}`,
  );
  if (!delTrace.data.applied) fail('trace delete not applied');
  const delState = await api(
    aelioUrl,
    'DELETE',
    `/api/v1/admin/db/tables/runtime_state/rows/${rowId}`,
  );
  if (!delState.data.applied) fail('runtime_state delete not applied');
  ok('deleted test rows');

  const gone = await api(
    aelioUrl,
    'GET',
    `/api/v1/admin/db/tables/runtime_state/rows/${rowId}`,
  );
  if (gone.status !== 404) fail(`expected 404 after delete, got ${gone.status}`);
  ok('deleted row returns 404 on get');

  database.close();

  console.log(`\n✓ All ${passed} Astrolobe database admin checks passed.`);
  cleanup(0);
}

main().catch((error) => {
  console.error(error);
  cleanup(1);
});
