#!/usr/bin/env node
/**
 * Archetype valence engine integration smoke against real AelioDb.
 *
 * Seeds the default archetype buckets (positive/negative/neutral per category)
 * into Aelio database, then asserts that incoming messages are classified into the
 * right per-category valence with confident guidance fed into the prompt — and
 * that ambiguous messages feed nothing. Uses a deterministic in-process
 * embedder that encodes category+valence concept dimensions so the recomputed
 * cosine (not AelioDb's RRF score) is the thing under test.
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
  DEFAULT_ARCHETYPES,
  embed,
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

// dim -> regex. Shared vocabulary between an archetype's signature and a real
// message lands them on the same concept dimension, so cosine is high for the
// matching bucket and near-zero for the others.
const CONCEPTS = [
  [0, /thank|great|perfect|love it|grateful|pleased|happy|satisfied|exactly what/i],
  [1, /broken|terrible|not working|frustrat|angry|upset|annoyed|unacceptable|fed up|disappointed|complain/i],
  [2, /can you tell|what is|how do i|i need to|informational|matter-of-fact/i],
  [3, /tell me more|what else|go on|continue|curious|interested|sounds good|and then/i],
  [4, /\bbye\b|goodbye|that is all|that's all|i am done|talk later|stop messaging|leave/i],
  [5, /let me think|maybe|not sure yet|i will see|hesitat|non-committal/i],
  [6, /go ahead|confirmed|i am sure|decisive|definitely|do it|yes do/i],
  [7, /confus|unsure|lost|don.?t understand|what do you mean|which one|misunderstand/i],
  [8, /what are the options|how does this compare|what happens if|exploratory|comparing/i],
  [9, /urgent|asap|immediately|right now|emergency|time sensitive/i],
  [10, /no rush|whenever|take your time|relaxed|no hurry/i],
];

let child;
let temp;
const embeddingCalls = new Map();

function vectorize(text) {
  embeddingCalls.set(text, (embeddingCalls.get(text) ?? 0) + 1);
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
  child?.kill('SIGTERM');
  if (temp) rmSync(temp, { recursive: true, force: true });
  process.exit(code);
}

function categoryOf(assessment, category) {
  return assessment.categories.find((entry) => entry.category === category);
}

async function main() {
  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;
  temp = mkdtempSync(join(tmpdir(), 'aelio-archetype-'));
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

  const store = createConvoxArchetypeStore({ client, tables: TABLES, embedDim: DIM });
  const seeded = await store.seedIfEmpty(DEFAULT_ARCHETYPES);
  assert(seeded === DEFAULT_ARCHETYPES.length, `seeded ${seeded} archetype buckets into AelioDb`);
  assert((await store.seedIfEmpty(DEFAULT_ARCHETYPES)) === 0, 'seedIfEmpty is idempotent');

  const engine = createArchetypeEngine({ store });

  // 1. Frustration → sentiment negative.
  const angry = await engine.assess({
    message: 'This is completely broken and still not working, I am so frustrated.',
  });
  const angrySentiment = categoryOf(angry, 'sentiment');
  assert(angrySentiment?.dominant === 'negative', 'frustrated message reads sentiment as negative');
  assert(angrySentiment?.fed === true, 'confident negative sentiment is fed to the prompt');
  assert(angry.overall.valence === 'negative', 'overall stance leans negative');
  assert(/acknowledge/i.test(angry.prompt), 'prompt carries the negative-sentiment response guidance');
  console.log('  angry.prompt =>', JSON.stringify(angry.prompt));

  // 2. Goodbye → engagement negative (disengaging).
  const bye = await engine.assess({ message: 'Thanks, that is all. Talk later, goodbye.' });
  const byeEngagement = categoryOf(bye, 'engagement');
  assert(byeEngagement?.dominant === 'negative', 'goodbye reads engagement as disengaging');
  assert(byeEngagement?.fed === true, 'disengagement stance is fed to the prompt');
  assert(/close warmly|do not open new topics/i.test(bye.prompt), 'prompt tells model to close warmly');

  // 3. Urgency → urgency positive.
  const urgent = await engine.assess({ message: 'I need this fixed right now, it is urgent, asap please.' });
  const urgentUrgency = categoryOf(urgent, 'urgency');
  assert(urgentUrgency?.dominant === 'positive', 'urgent message reads urgency as high');
  assert(urgentUrgency?.fed === true, 'high urgency is fed to the prompt');

  // 4. Confusion → certainty negative.
  const confused = await engine.assess({ message: 'I do not understand, what do you mean, which one?' });
  const confusedCertainty = categoryOf(confused, 'certainty');
  assert(confusedCertainty?.dominant === 'negative', 'confused message reads certainty as low');
  assert(/clarify/i.test(confused.prompt), 'prompt tells model to clarify');

  // 5. Per-category three numbers are populated and comparable (cosine, not RRF).
  assert(
    angrySentiment.negative > angrySentiment.positive &&
      angrySentiment.negative > angrySentiment.neutral,
    'negative number dominates positive/neutral for a frustrated message',
  );
  console.log('  sentiment numbers =>', JSON.stringify({
    positive: angrySentiment.positive.toFixed(3),
    negative: angrySentiment.negative.toFixed(3),
    neutral: angrySentiment.neutral.toFixed(3),
  }));

  // 6. Ambiguous/off-topic message feeds nothing (decide-whether-to-feed).
  const bland = await engine.assess({ message: 'The mitochondria is the powerhouse of the cell.' });
  assert(bland.prompt === '', 'weak/ambiguous signal feeds no stance into the prompt');
  assert(bland.categories.every((entry) => !entry.fed), 'nothing clears the feed threshold when unsure');

  // 7. Single-embedding reuse: passing a precomputed vector skips embedding.
  const message = 'This is broken and I am frustrated.';
  const vector = await embed(message);
  embeddingCalls.delete(message);
  await engine.assess({ message, queryVector: vector });
  assert(!embeddingCalls.has(message), 'precomputed vector is reused (no extra embedding call)');

  console.log('timings_ms', JSON.stringify(angry.timings));
  console.log('✓ Archetype valence engine integration smoke passed.');
  cleanup(0);
}

main().catch((error) => {
  console.error(error);
  cleanup(1);
});
