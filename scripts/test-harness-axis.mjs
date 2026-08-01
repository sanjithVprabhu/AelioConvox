#!/usr/bin/env node
/**
 * Harness Axis + temporal + evidence integration smoke against real AelioDb.
 *
 * Proves:
 *   1. Temporal resolver locks onto "last week"
 *   2. Fed stance writes an occurrence onto the customer axis
 *   3. A later turn traverses the axis and feeds evidence into the prompt
 *   4. Evidence scorer ranks the prior occurrence above threshold
 *   5. Resolution state is derived deterministically
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
  createArchetypeEngine,
  createConvoxArchetypeStore,
  createConvoxAspectStore,
  createConvoxAxisStore,
  DEFAULT_ARCHETYPES,
  DEFAULT_EVIDENCE_CONFIG,
  deriveResolutionState,
  evidenceFromAxisOccurrence,
  resolveTemporalScope,
  scoreProactiveActivation,
  seedBuiltinArchetypes,
  selectEvidence,
  renderEvidenceBlock,
} from '@aelio/core';
import { AelioDbClient } from '@aelio/db-client';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const AELIO_OS = join(ROOT, 'aelio-os');
const AELIO_SERVER = join(AELIO_OS, 'target/release/aelio-server');
const DIM = 64;
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
  sdkConnections: 'convox_sdk_connections', archetypes: 'convox_archetypes',
  aspects: 'convox_aspects', axisNodes: 'convox_axis_nodes',
};

const CONCEPTS = [
  [1, /broken|terrible|not working|frustrat|angry|upset|annoyed|unacceptable/i],
  [12, /expensive|overpriced|too pricey|can.?t afford|cannot afford|unafford/i],
];

let child;
let temp;

function vectorize(text) {
  const vector = new Array(DIM).fill(0);
  for (const [dim, pattern] of CONCEPTS) {
    if (pattern.test(text)) vector[dim] = 2;
  }
  for (const token of text.toLowerCase().split(/\W+/).filter(Boolean)) {
    let hash = 0;
    for (const char of token) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
    vector[16 + (hash % (DIM - 16))] += 0.05;
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
  try {
    child?.kill('SIGTERM');
  } catch {}
  if (temp) rmSync(temp, { recursive: true, force: true });
  process.exit(code);
}

async function main() {
  // --- pure temporal / resolution unit checks (no AelioDb needed) ---
  const lastWeek = resolveTemporalScope({
    message: 'Last week you said there was a monthly option',
    now: Date.parse('2026-07-19T12:00:00Z'),
  });
  assert(lastWeek.mode === 'explicit', 'temporal resolver detects "last week"');
  assert(lastWeek.tiers.includes('historical'), 'last-week scope includes historical tier');

  const resolved = deriveResolutionState({
    reflectionOutcome: 'resolved',
    pathwayProactive: 'consider_followup',
  });
  assert(resolved.state === 'resolved', 'resolved reflection wins over follow-up hint');
  const activation = scoreProactiveActivation({ snapshot: resolved });
  assert(activation.suppressed === true, 'resolved state hard-suppresses proactive nudges');

  const awaiting = deriveResolutionState({
    pathwayProactive: 'consider_followup',
    reflectionOutcome: 'unresolved',
  });
  const nudge = scoreProactiveActivation({
    snapshot: { ...awaiting, updatedAt: Date.now() - 60_000 },
  });
  assert(nudge.action === 'nudge', `unresolved awaiting_user activates nudge (score=${nudge.score.toFixed(2)})`);

  // --- AelioDb axis graph ---
  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;
  temp = mkdtempSync(join(tmpdir(), 'aelio-axis-'));
  child = spawn(AELIO_SERVER, [], {
    cwd: AELIO_OS,
    env: { ...process.env, AELIO_ALLOW_INSECURE_OPEN: '1', AELIO_RUNTIME_BIND: `127.0.0.1:${port}`, AELIO_DATA_DIR: join(temp, 'data') },
    stdio: 'ignore',
  });
  child.on('error', () => {});
  await healthy(url);

  const client = new AelioDbClient({ baseUrl: url });
  await bootstrapAelioDbTables(client, TABLES, DIM);
  configureEmbedder(async (text) => vectorize(text));

  const config = { client, tables: TABLES, embedDim: DIM };
  const aspectStore = createConvoxAspectStore(config);
  const archetypeStore = createConvoxArchetypeStore(config);
  const axisStore = createConvoxAxisStore(config);
  await seedBuiltinArchetypes(aspectStore, archetypeStore, DEFAULT_ARCHETYPES);

  const engine = createArchetypeEngine({ store: archetypeStore, aspectStore });
  const customerId = 'axis-customer';
  const msg1 = 'I am really frustrated — this is broken and unacceptable.';
  const stance1 = await engine.assess({ message: msg1 });
  const fed = stance1.categories.filter((c) => c.fed && c.aspectId);
  assert(fed.length > 0, `stance feeds at least one aspect (got ${JSON.stringify(stance1.categories)})`);

  const weekAgo = Date.now() - 7 * 24 * 60 * 60 * 1000;
  for (const [index, category] of fed.entries()) {
    await axisStore.recordOccurrence({
      customerId,
      aspectId: category.aspectId,
      aspectName: category.category,
      turnId: 'turn-1',
      spanIndex: index,
      span: category.span ?? msg1,
      valence: category.dominant,
      positive: category.positive,
      negative: category.negative,
      neutral: category.neutral,
      strength: category.strength,
      intentLabel: 'support',
      embedding: vectorize(msg1),
      createdAt: weekAgo,
    });
  }
  assert((await axisStore.listAxes(customerId)).length >= 1, 'axis root created for customer');

  // Second write is idempotent.
  const again = await axisStore.recordOccurrence({
    customerId,
    aspectId: fed[0].aspectId,
    aspectName: fed[0].category,
    turnId: 'turn-1',
    spanIndex: 0,
    valence: fed[0].dominant,
    positive: fed[0].positive,
    negative: fed[0].negative,
    neutral: fed[0].neutral,
    strength: fed[0].strength,
    embedding: vectorize(msg1),
    createdAt: weekAgo,
  });
  assert(again === null, 'duplicate occurrence write is idempotent');

  const scope = resolveTemporalScope({
    message: 'I am still frustrated like last week',
    now: Date.now(),
  });
  const chain = await axisStore.traverseAxis({
    customerId,
    aspectId: fed[0].aspectId,
    scope,
    queryVector: vectorize('still frustrated'),
  });
  assert(chain.length >= 1, `axis traversal returns prior occurrence (got ${chain.length})`);
  assert(chain[0].aspectName === fed[0].category, 'occurrence carries aspect name');

  const evidence = selectEvidence(
    chain.map((occ) => evidenceFromAxisOccurrence(occ, { now: Date.now() })),
    DEFAULT_EVIDENCE_CONFIG,
  );
  assert(evidence.length >= 1, 'evidence scorer keeps the prior occurrence');
  const block = renderEvidenceBlock(evidence);
  assert(/RELEVANT EVIDENCE/.test(block), 'evidence prompt block renders');
  console.log('  evidence =>', block.split('\n')[1]);

  console.log('✓ Harness Axis / temporal / evidence / resolution smoke passed.');
  cleanup(0);
}

main().catch((error) => {
  console.error(error);
  cleanup(1);
});
