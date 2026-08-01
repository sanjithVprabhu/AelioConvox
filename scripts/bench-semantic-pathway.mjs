#!/usr/bin/env node
/**
 * Latency benchmark for semantic retrieval against a live aelio-server.
 *
 * Separates the three cost centers so we know where time actually goes:
 *   1. embedding generation (excluded from AelioDb — provider dependent)
 *   2. a single vector query (VSS)
 *   3. the full parallel pathway decision (memories + tools + policies + flows)
 *
 * Uses a deterministic in-process embedder so the numbers isolate AelioDb +
 * engine cost, not OpenAI network time (which is reported separately as a
 * fixed additive constant callers must budget for).
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
const DIM = 256;
const MEMORIES = Number(process.env.BENCH_MEMORIES ?? 2000);
const ITERATIONS = Number(process.env.BENCH_ITERS ?? 200);
const TOOLS = Number(process.env.BENCH_TOOLS ?? 40);

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
  sdkConnections: 'convox_sdk_connections',
  archetypes: 'convox_archetypes', aspects: 'convox_aspects', axisNodes: 'convox_axis_nodes',
};

let child;
let temp;

function seededVector(seed) {
  let state = seed >>> 0;
  const next = () => {
    state = (state * 1664525 + 1013904223) >>> 0;
    return state / 0xffffffff;
  };
  const vector = Array.from({ length: DIM }, () => next() - 0.5);
  const mag = Math.sqrt(vector.reduce((s, v) => s + v * v, 0)) || 1;
  return vector.map((v) => v / mag);
}

function hashText(text) {
  let hash = 2166136261;
  for (const char of text) {
    hash ^= char.charCodeAt(0);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
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
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error('aelio-server did not become healthy');
}

function pct(sorted, p) {
  const index = Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length));
  return sorted[index];
}

function report(label, samples) {
  const sorted = [...samples].sort((a, b) => a - b);
  const mean = samples.reduce((s, v) => s + v, 0) / samples.length;
  console.log(
    `${label.padEnd(28)} p50=${pct(sorted, 50).toFixed(2)}ms  ` +
      `p95=${pct(sorted, 95).toFixed(2)}ms  p99=${pct(sorted, 99).toFixed(2)}ms  ` +
      `mean=${mean.toFixed(2)}ms`,
  );
}

function cleanup(code) {
  configureEmbedder(null);
  try { child?.kill('SIGTERM'); } catch {}
  if (temp) rmSync(temp, { recursive: true, force: true });
  process.exit(code);
}

async function main() {
  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;
  temp = mkdtempSync(join(tmpdir(), 'aelio-bench-'));
  child = spawn(AELIO_SERVER, [], {
    cwd: AELIO_OS,
    env: { ...process.env, AELIO_ALLOW_INSECURE_OPEN: '1', AELIO_RUNTIME_BIND: `127.0.0.1:${port}`, AELIO_DATA_DIR: join(temp, 'data') },
    stdio: 'ignore',
  });
  child.on('error', () => {});
  await healthy(url);

  const client = new AelioDbClient({ baseUrl: url });
  await bootstrapAelioDbTables(client, TABLES, DIM);
  configureEmbedder(async (text) => seededVector(hashText(text)));

  const memoryStore = createConvoxMemoryStore({ client, tables: TABLES, embedDim: DIM });
  const customerId = 'bench-customer';

  console.log(`Seeding ${MEMORIES} memories (dim ${DIM})...`);
  const seedStart = performance.now();
  for (let i = 0; i < MEMORIES; i += 1) {
    const content = `memory fact number ${i} about topic ${i % 97}`;
    await memoryStore.store({
      customerId,
      content,
      category: 'fact',
      sourceSessionId: 'bench',
      embedding: await embed(content),
      dedupe: false,
    });
  }
  console.log(`Seeded in ${((performance.now() - seedStart) / 1000).toFixed(1)}s`);
  await client.flush();

  const functions = Array.from({ length: TOOLS }, (_, i) => ({
    name: `tool_${i}`,
    intent: `capability_${i}`,
    description: `Perform operation ${i} on domain object ${i % 13}`,
    safety: i % 5 === 0 ? 'write' : 'read',
    params: {},
  }));

  const engine = createSemanticPathwayEngine({ memoryStore });
  const queries = Array.from({ length: ITERATIONS }, (_, i) => `please help me with topic ${i % 97}`);

  // 1. Raw single vector query latency.
  const vectorSamples = [];
  for (const query of queries) {
    const vector = await embed(query);
    const start = performance.now();
    await client.query(TABLES.memories, {
      k: 20,
      vector: { col: 'embedding', query: vector },
      filters: [{ col: 'customer_id', op: 'eq', value: { type: 'utf8', value: customerId } }],
    });
    vectorSamples.push(performance.now() - start);
  }

  // 2. Full pathway decision latency (parallel memory + tools + policies + flows).
  const pathwaySamples = [];
  const retrievalSamples = [];
  for (const message of queries) {
    const decision = await engine.decide({
      customerId,
      message,
      functions,
      policies: [
        { id: 'confirm-writes', description: 'Confirm writes', severity: 'hard' },
        { id: 'tone', description: 'Stay concise and helpful', severity: 'soft' },
      ],
      flows: [],
      intentStack: [],
      memoryLimit: 5,
    });
    pathwaySamples.push(decision.timings.totalMs);
    retrievalSamples.push(decision.timings.parallelRetrievalMs);
  }

  console.log(`\n=== Semantic retrieval latency (${MEMORIES} memories, ${TOOLS} tools, ${ITERATIONS} iters) ===`);
  report('single vector query', vectorSamples);
  report('pathway parallel retrieval', retrievalSamples);
  report('pathway total (excl. embed)', pathwaySamples);
  console.log(
    '\nNote: excludes provider embedding round trip. Budget ~100–500ms additionally\n' +
      'for a remote OpenAI embedding on the first occurrence of a given message\n' +
      '(identical text is served from the in-process LRU at ~0ms).',
  );
  cleanup(0);
}

main().catch((error) => {
  console.error(error);
  cleanup(1);
});
