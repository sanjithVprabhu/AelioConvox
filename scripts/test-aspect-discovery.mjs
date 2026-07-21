#!/usr/bin/env node
/**
 * Self-learning aspect taxonomy integration smoke against real Sunjet.
 *
 * Proves the full loop: seed builtin aspects into the mother collection →
 * a message about a NEW "thing" isn't covered → the discovery analyst (stub
 * LLM) names it and writes its pos/neu/neg buckets → on the NEXT turn the
 * (vector-only) archetype engine detects and feeds that brand-new aspect →
 * a second sighting is reinforced with NO further LLM cost.
 */
import { spawn } from 'node:child_process';
import { createServer } from 'node:net';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  bootstrapSunjetTables,
  configureEmbedder,
  createArchetypeEngine,
  createConvoxArchetypeStore,
  createConvoxAspectStore,
  seedBuiltinArchetypes,
  DEFAULT_ARCHETYPES,
} from '@aelio/core';
import { SunjetClient } from '@aelio/sunjet-client';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const ASTROLOBE = join(ROOT, 'Sunjet/Astrolobe');
const LL_SERVER = join(ASTROLOBE, 'target/release/ll-server');
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

// Concept dims shared between archetype signatures and messages. 12-14 are the
// "price_sensitivity" dimension the stub LLM will discover.
const CONCEPTS = [
  [1, /broken|terrible|not working|frustrat|angry|upset|annoyed|unacceptable/i],
  [4, /\bbye\b|goodbye|that is all|talk later|leave/i],
  [7, /confus|unsure|lost|don.?t understand|what do you mean|which one/i],
  [9, /urgent|asap|immediately|right now|emergency/i],
  [12, /expensive|overpriced|too pricey|can.?t afford|cannot afford|unafford/i],
  [13, /affordable|great value|worth it|good price|cheap/i],
  [14, /how much|pricing option|what.*price|cost breakdown/i],
];

let child;
let temp;
let llmCalls = 0;

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

// Stub LLM: whatever it's asked, it proposes the price_sensitivity aspect.
const stubLLM = {
  async complete() {
    llmCalls += 1;
    return {
      text: JSON.stringify({
        aspects: [
          {
            name: 'price_sensitivity',
            description:
              'How sensitive the user is to price — mentions of things being too expensive, overpriced, or unaffordable.',
            positive: {
              keyword: 'affordable great value worth it',
              description: 'User finds the price acceptable or a good deal.',
              usage: 'great value, worth it, affordable, good price',
              inference: 'Positive remarks about cost.',
              guidance: 'Reinforce the value they are getting.',
            },
            neutral: {
              keyword: 'pricing question how much',
              description: 'User is asking about cost without judgement.',
              usage: 'how much does it cost, pricing options, cost breakdown',
              inference: 'Neutral price inquiry.',
              guidance: 'Give clear, itemised pricing.',
            },
            negative: {
              keyword: 'expensive overpriced cannot afford',
              description: 'User finds it too expensive or unaffordable.',
              usage: 'too expensive, overpriced, cannot afford, too pricey',
              inference: 'Price objection or budget concern.',
              guidance: 'Acknowledge the budget concern and show value or a cheaper option.',
            },
          },
        ],
      }),
    };
  },
};

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
  throw new Error('ll-server did not become healthy');
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
  temp = mkdtempSync(join(tmpdir(), 'aelio-aspect-'));
  child = spawn(LL_SERVER, [], {
    cwd: ASTROLOBE,
    env: { ...process.env, LL_BIND: `127.0.0.1:${port}`, LL_DATA_DIR: join(temp, 'data') },
    stdio: 'ignore',
  });
  child.on('error', () => {});
  await healthy(url);

  const client = new SunjetClient({ baseUrl: url });
  await bootstrapSunjetTables(client, TABLES, DIM);
  configureEmbedder(async (text) => vectorize(text));

  const config = { client, tables: TABLES, embedDim: DIM };
  const aspectStore = createConvoxAspectStore(config);
  const archetypeStore = createConvoxArchetypeStore(config);

  // --- seed the mother collection with builtin aspects ---
  const seeded = await seedBuiltinArchetypes(aspectStore, archetypeStore, DEFAULT_ARCHETYPES);
  assert(seeded === 4, `seeded ${seeded} builtin aspects into the mother collection`);
  const builtins = await aspectStore.list();
  assert(builtins.every((a) => a.source === 'builtin' && a.status === 'active'), 'builtins are active');
  assert(builtins.some((a) => a.name === 'sentiment'), 'sentiment builtin registered');

  const engine = createArchetypeEngine({ store: archetypeStore, aspectStore });

  // --- turn 1: a NEW "thing" not covered by builtins → discovery runs ---
  const priceMsg = 'Honestly this is way too expensive, I really cannot afford this.';
  const before = llmCalls;
  const learned = await engine.learn({
    message: priceMsg,
    llm: stubLLM,
    model: 'stub',
    maxTokens: 600,
  });
  assert(learned?.usedLLM === true, 'uncovered message triggers the discovery LLM pass');
  assert(learned.createdAspects.includes('price_sensitivity'), 'a new aspect is discovered and created');
  assert(llmCalls === before + 1, 'exactly one discovery LLM call was made');

  const afterCreate = await aspectStore.list();
  const price = afterCreate.find((a) => a.name === 'price_sensitivity');
  assert(!!price, 'price_sensitivity now lives in the mother collection');
  assert(price.status === 'candidate' && price.source === 'discovered', 'new aspect is a discovered candidate');

  // --- turn 2: STAGING — a candidate aspect must NOT feed live prompts yet ---
  const nextMsg = 'This plan is overpriced, way too expensive for me.';
  const stagedStance = await engine.assess({ message: nextMsg });
  const stagedCat = stagedStance.categories.find((c) => c.category === 'price_sensitivity');
  assert(stagedCat === undefined, 'a candidate aspect is withheld from the prompt (staged)');

  // --- approval: promotion to 'active' publishes its buckets to live retrieval ---
  await aspectStore.setStatus(price.id, 'active');
  const stance = await new (engine.constructor)({ store: archetypeStore, aspectStore }).assess({
    message: nextMsg,
  }); // fresh engine: the 30s active-aspect cache must not serve the pre-approval set
  const priceCat = stance.categories.find((c) => c.category === 'price_sensitivity');
  assert(priceCat?.dominant === 'negative', 'the approved aspect is detected as negative next turn');
  assert(priceCat?.fed === true, 'the approved aspect is fed into the reply');
  assert(/budget|value|cheaper/i.test(stance.prompt), 'discovered guidance reaches the prompt');
  console.log('  learned stance =>', JSON.stringify(stance.prompt));

  // --- turn 3: another sighting is reinforced with NO LLM cost ---
  const beforeReinforce = llmCalls;
  const reinforced = await engine.learn({
    message: 'Everything here is just so expensive, I cannot afford it at all.',
    llm: stubLLM,
    model: 'stub',
    maxTokens: 600,
  });
  assert(reinforced?.usedLLM === false, 'a covered message reinforces without any LLM call');
  assert(reinforced.touchedAspects.includes('price_sensitivity'), 'the known aspect is touched/reinforced');
  assert(llmCalls === beforeReinforce, 'no extra LLM call on the covered turn');

  const priceAfter = (await aspectStore.list()).find((a) => a.name === 'price_sensitivity');
  assert(priceAfter.hits >= 1, `aspect hit count grows with sightings (hits=${priceAfter.hits})`);
  assert((await aspectStore.list()).length === 5, 'no duplicate aspect was created (still 5 total)');

  console.log('✓ Self-learning aspect taxonomy integration smoke passed.');
  cleanup(0);
}

main().catch((error) => {
  console.error(error);
  cleanup(1);
});
