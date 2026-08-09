#!/usr/bin/env node
/**
 * Semantic Pathway Engine integration + latency smoke against real AelioDb.
 */
import { spawn } from 'node:child_process';
import { createServer } from 'node:net';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  bootstrapAelioDbTables,
  configureEmbedder,
  createConvoxMemoryStore,
  createSemanticPathwayEngine,
  embed,
} from '@aelio/core';
import { AelioDbClient } from '@aelio/db-client';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const AELIO_OS = join(ROOT, 'aelio-os');
const AELIO_SERVER = join(AELIO_OS, 'target/release/aelio-server');
const DIM = 32;
const TABLES = {
  messages: 'convox_messages', conversations: 'convox_conversations',
  memories: 'convox_memories', compactions: 'convox_compactions',
  runtimeState: 'runtime_state', harnessTools: 'harness_tools',
  harnessCapabilities: 'harness_capabilities', harnessBindings: 'harness_bindings',
  harnessSuspensions: 'harness_suspensions', harnessLedger: 'harness_ledger',
  harnessTraces: 'harness_traces', customers: 'convox_customers',
  channelAddresses: 'convox_channel_addresses', sessions: 'convox_sessions',
  jobQueue: 'convox_job_queue', responseCache: 'convox_response_cache',
  functionCalls: 'convox_function_calls', turnApiCalls: 'convox_turn_api_calls',
  reflections: 'convox_reflections', proactiveMessages: 'convox_proactive_messages',
  inboundDedup: 'convox_inbound_dedup', magicLinks: 'convox_magic_links',
  sdkConnections: 'convox_sdk_connections', sdkCatalog: 'convox_sdk_catalog',
  archetypes: 'convox_archetypes', aspects: 'convox_aspects', axisNodes: 'convox_axis_nodes',
};

let child;
let temp;
const embeddingCalls = new Map();

function vectorize(text) {
  embeddingCalls.set(text, (embeddingCalls.get(text) ?? 0) + 1);
  const vector = new Array(DIM).fill(0);
  const concepts = [
    /refund|return|money back/i,
    /order|purchase/i,
    /price|pricing|plan|subscription/i,
    /email|notification/i,
    /shipping|address|delivery/i,
    /bye|goodbye|stop/i,
  ];
  concepts.forEach((pattern, index) => {
    if (pattern.test(text)) vector[index] = 1;
  });
  // Keep unrelated text non-zero and deterministic.
  for (const token of text.toLowerCase().split(/\W+/).filter(Boolean)) {
    let hash = 0;
    for (const char of token) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
    vector[6 + (hash % (DIM - 6))] += 0.05;
  }
  const mag = Math.sqrt(vector.reduce((sum, value) => sum + value * value, 0)) || 1;
  return vector.map((value) => value / mag);
}

function freePort() {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      server.close((error) => (error ? reject(error) : resolve(port)));
    });
  });
}

async function healthy(url) {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`${url}/v1/health`);
      if (response.ok) return;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error('aelio-server did not become healthy');
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
  console.log(`OK: ${message}`);
}

function cleanup(code) {
  configureEmbedder(null);
  child?.kill('SIGTERM');
  if (temp) rmSync(temp, { recursive: true, force: true });
  process.exit(code);
}

async function main() {
  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;
  temp = mkdtempSync(join(tmpdir(), 'aelio-pathway-'));
  child = spawn(AELIO_SERVER, [], {
    cwd: AELIO_OS,
    env: { ...process.env, AELIO_ALLOW_INSECURE_OPEN: '1', AELIO_RUNTIME_BIND: `127.0.0.1:${port}`, AELIO_DATA_DIR: join(temp, 'data') },
    stdio: 'ignore',
  });
  // Some sandboxed runners deny signalling a spawned native process during
  // teardown. Treat that cleanup-only error as non-fatal.
  child.on('error', () => {});
  await healthy(url);

  const client = new AelioDbClient({ baseUrl: url });
  await bootstrapAelioDbTables(client, TABLES, DIM);
  configureEmbedder(async (text) => vectorize(text));

  const memoryStore = createConvoxMemoryStore({ client, tables: TABLES, embedDim: DIM });
  const customerId = 'pathway-customer';
  const memoryText = 'Customer wants a refund for order 4421';
  await memoryStore.store({
    customerId,
    content: memoryText,
    category: 'goal',
    sourceSessionId: 'session-1',
    embedding: await embed(memoryText),
  });

  const engine = createSemanticPathwayEngine({ memoryStore });
  const message = 'Please refund my order 4421';
  const decision = await engine.decide({
    customerId,
    message,
    functions: [
      {
        name: 'refund_order', intent: 'order_refund',
        description: 'Refund an eligible customer order', safety: 'write', params: {},
      },
      {
        name: 'list_plans', intent: 'plan_discovery',
        description: 'List subscription pricing plans', safety: 'read', params: {},
      },
    ],
    policies: [
      { id: 'confirm-writes', description: 'Confirm consequential write actions', severity: 'hard' },
      { id: 'refund-help', description: 'Explain refund eligibility clearly', severity: 'soft' },
      { id: 'pricing-tone', description: 'Explain pricing without pressure', severity: 'soft' },
    ],
    flows: [{
      id: 'refund-flow', state: 'support', description: 'Resolve an order refund request',
      steps: [{ id: 'identify-order', goal: 'Identify the order', tool: 'refund_order' }],
    }],
    activeStateId: 'support',
    flowProgress: {},
    intentStack: [],
    forceIncludeTools: ['refund_order'],
  });

  assert(decision.intent.label === 'order_refund', 'semantic intent selects refund pathway');
  assert(decision.tools[0]?.fn.name === 'refund_order', 'refund tool ranks first');
  assert(decision.memories.some((memory) => memory.content === memoryText), 'AelioDb VSS recalls relevant memory');
  assert(decision.policies.some((policy) => policy.id === 'confirm-writes'), 'hard policy always survives retrieval');
  assert(decision.flow?.definition.id === 'refund-flow', 'active-state flow is selected');
  assert(decision.strategy === 'guide', 'active flow produces guided response strategy');
  assert(decision.proactive.action === 'consider_followup', 'unfinished flow emits guarded follow-up hint');
  assert(embeddingCalls.get(message) === 1, 'incoming message is embedded exactly once');
  console.log('timings_ms', JSON.stringify(decision.timings));

  const goodbye = await engine.decide({
    customerId,
    message: 'Thanks, goodbye',
    functions: [],
    policies: [],
    flows: [],
    intentStack: [],
  });
  assert(goodbye.strategy === 'disengage', 'goodbye selects disengagement');
  assert(goodbye.proactive.action === 'suppress', 'goodbye suppresses proactive re-engagement');

  const degradedEngine = createSemanticPathwayEngine({
    memoryStore: {
      recallByVector: async () => {
        throw new Error('simulated memory outage');
      },
    },
  });
  const degraded = await degradedEngine.decide({
    customerId,
    message: 'What can you help with?',
    functions: [],
    policies: [],
    flows: [],
    intentStack: [],
  });
  assert(
    degraded.degradedSources.includes('memories'),
    'memory retrieval failure degrades without failing the turn',
  );

  console.log('✓ Semantic Pathway Engine integration smoke passed.');
  cleanup(0);
}

main().catch((error) => {
  console.error(error);
  cleanup(1);
});
