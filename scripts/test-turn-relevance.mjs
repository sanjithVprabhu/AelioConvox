#!/usr/bin/env node
/**
 * Reply-relevance audit: drives processTurn end-to-end against a real AelioDb
 * (spawned aelio-server) with a capturing mock LLM, and asserts that everything
 * retrieval pulled actually REACHES the system prompt the model answers from:
 *
 *   1. a seeded long-term memory (the customer's correct VAT number),
 *   2. the archetype stance guidance for a frustrated message,
 *   3. the immediate-context block carrying the previous turn verbatim,
 *   4. the semantic pathway decision naming the relevant capability,
 *   5. and that the reply is produced and persisted to AelioDb.
 *
 * If any of these blocks go missing from the prompt, replies lose relevance
 * silently — this script makes that regression loud.
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
  createConvoxCustomerStore,
  createConvoxFunctionCallStore,
  createConvoxMemoryStore,
  createConvoxMessageStore,
  createConvoxSessionStore,
  createImmediateContextEngine,
  createSemanticPathwayEngine,
  DEFAULT_ARCHETYPES,
  embed,
  HarnessTracer,
  processTurn,
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
  sdkConnections: 'convox_sdk_connections',
  archetypes: 'convox_archetypes', aspects: 'convox_aspects', axisNodes: 'convox_axis_nodes',
};

let child;
let temp;
const embeddingCalls = new Map();

// Deterministic concept embedder: shared concept dimensions make the message,
// the archetype signatures, the memory, and the tool descriptor land near each
// other exactly when they talk about the same thing.
function vectorize(text) {
  embeddingCalls.set(text, (embeddingCalls.get(text) ?? 0) + 1);
  const vector = new Array(DIM).fill(0);
  const concepts = [
    /frustrat|angry|upset|annoyed|unacceptable|terrible|fed up|complain/i,
    /invoice|vat|billing|charge/i,
    /urgent|asap|immediately|right now|time sensitive|emergency/i,
    /cancel|refund/i,
    /bye|goodbye|stop|leave/i,
  ];
  concepts.forEach((pattern, index) => {
    if (pattern.test(text)) vector[index] = 1;
  });
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
  if (!condition) throw new Error(`FAIL: ${message}`);
  console.log(`OK: ${message}`);
}

function cleanup(code) {
  configureEmbedder(null);
  if (child && child.exitCode === null) {
    try {
      child.kill('SIGTERM');
    } catch (error) {
      // Some managed sandboxes deny signalling child processes during teardown.
      // The relevance assertions above are the test signal; cleanup failures
      // should not turn a passing audit into a false negative.
      console.warn(
        `WARN: unable to stop temporary aelio-server: ${
          error instanceof Error ? error.message : String(error)
        }`,
      );
    }
  }
  if (temp) rmSync(temp, { recursive: true, force: true });
  process.exit(code);
}

async function main() {
  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;
  temp = mkdtempSync(join(tmpdir(), 'aelio-relevance-'));
  child = spawn(AELIO_SERVER, [], {
    cwd: AELIO_OS,
    env: { ...process.env, AELIO_ALLOW_INSECURE_OPEN: '1', AELIO_RUNTIME_BIND: `127.0.0.1:${port}`, AELIO_DATA_DIR: join(temp, 'data') },
    stdio: 'ignore',
  });
  await healthy(url);
  configureEmbedder(async (text) => vectorize(text));

  const client = new AelioDbClient({ baseUrl: url });
  await bootstrapAelioDbTables(client, TABLES, DIM);
  const storageConfig = { client, tables: TABLES, embedDim: DIM };

  const messageStore = createConvoxMessageStore(storageConfig);
  const sessionStore = createConvoxSessionStore(storageConfig);
  const customerStore = createConvoxCustomerStore(storageConfig);
  const functionCallStore = createConvoxFunctionCallStore(storageConfig);
  const memoryStore = createConvoxMemoryStore(storageConfig);

  const archetypeStore = createConvoxArchetypeStore(storageConfig);
  await archetypeStore.seedIfEmpty(DEFAULT_ARCHETYPES);
  const archetypeEngine = createArchetypeEngine({ store: archetypeStore });
  const contextEngine = createImmediateContextEngine({ storage: storageConfig });
  const pathwayEngine = createSemanticPathwayEngine({ memoryStore });
  const tracer = new HarnessTracer({
    client,
    table: TABLES.harnessTraces,
    tenant: 'relevance-test',
    embedDim: DIM,
  });

  // Capturing mock LLM: records every system prompt it is asked to answer from.
  const llmCalls = [];
  const llm = {
    complete: async (opts) => {
      llmCalls.push(opts);
      return { text: 'Understood — I will get that corrected for you right away.', toolCalls: [] };
    },
  };

  const sdk = {
    getFunctions: () => [
      {
        name: 'update_invoice',
        intent: 'billing_correction',
        description: 'Correct invoice details such as the VAT number or billing address',
        safety: 'write',
        parameters: { type: 'object', properties: {} },
      },
      {
        name: 'get_weather',
        intent: 'weather_lookup',
        description: 'Look up the current weather forecast for a city',
        safety: 'read',
        parameters: { type: 'object', properties: {} },
      },
    ],
    getStates: () => [],
    getPolicies: () => [],
    getFlows: () => [],
    invoke: async () => ({ ok: true, data: {}, durationMs: 0 }),
  };

  const externalId = `relevance-${Date.now()}`;
  const base = {
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
    sessionStore,
    customerStore,
    functionCallStore,
    memoryStore,
    contextEngine,
    pathwayEngine,
    archetypeEngine,
    tracer,
    customerExternalId: externalId,
    channel: 'web',
    channelAddress: `web:${externalId}`,
  };

  // --- Turn 1: neutral opener (creates customer/session, fills hot window) ---
  const turn1 = await processTurn({ ...base, message: 'hi, I need some help with my account' });
  assert(turn1.reply.length > 0, 'turn 1 produced a reply');

  const customer = await customerStore.getByExternalId(externalId);
  assert(Boolean(customer), 'customer exists after turn 1');

  // Seed the long-term memory the relevant reply depends on.
  const memoryContent =
    'Customer is on the Pro plan; their correct VAT registration for invoices is DE811556677.';
  await memoryStore.store({
    customerId: customer.id,
    content: memoryContent,
    category: 'billing',
    sourceSessionId: 'seed',
    embedding: await embed(memoryContent),
  });

  // --- Turn 2: the frustrated, non-generic message under audit ---
  const turn2 = await processTurn({
    ...base,
    message:
      'I am really frustrated — my invoice has the wrong VAT number again. Fix it immediately or I will cancel.',
  });
  assert(turn2.reply.length > 0, 'turn 2 produced a reply');

  const audited = llmCalls[llmCalls.length - 1];
  assert(Boolean(audited?.system), 'turn 2 reached the LLM with a system prompt');
  const system = audited.system;

  // 1. Long-term memory relevance: the seeded fact must reach the prompt.
  assert(system.includes('DE811556677'), 'recalled memory (correct VAT number) is in the prompt');

  // 2. Stance relevance: frustrated message must feed de-escalation guidance.
  assert(system.includes('CONVERSATIONAL STANCE'), 'stance block is in the prompt');
  assert(
    system.includes('sincere acknowledgement'),
    'negative-sentiment guidance (de-escalation) is in the prompt',
  );

  // 3. Immediate context: turn 1 must be visible verbatim in the hot window.
  assert(system.includes('IMMEDIATE CONVERSATION CONTEXT'), 'immediate context block is in the prompt');
  assert(
    system.includes('help with my account'),
    'previous turn is carried verbatim in the hot window',
  );

  // 4. Pathway relevance: the billing capability must be surfaced, ranked first.
  assert(system.includes('SEMANTIC PATHWAY DECISION'), 'pathway decision block is in the prompt');
  const capabilityLine = system
    .split('\n')
    .find((line) => line.startsWith('Most relevant capabilities:'));
  assert(Boolean(capabilityLine), 'pathway lists most relevant capabilities');
  assert(
    capabilityLine.indexOf('update_invoice') !== -1 &&
      (capabilityLine.indexOf('get_weather') === -1 ||
        capabilityLine.indexOf('update_invoice') < capabilityLine.indexOf('get_weather')),
    'update_invoice outranks the irrelevant capability',
  );

  // 5. The turn-2 message was embedded exactly once (shared vector fan-out).
  const turn2Message =
    'I am really frustrated — my invoice has the wrong VAT number again. Fix it immediately or I will cancel.';
  assert(
    (embeddingCalls.get(turn2Message) ?? 0) === 1,
    'turn 2 message embedded exactly once across cache/pathway/stance/storage',
  );

  // 6. Persistence: both turns' user+assistant rows landed in AelioDb.
  const scan = await client.scanRows(TABLES.messages, {
    k: 50,
    filters: [{ col: 'customer_id', op: 'eq', value: { type: 'utf8', value: customer.id } }],
  });
  const roles = scan.rows.map((row) => row.values.role?.value);
  assert(
    roles.filter((role) => role === 'user').length >= 2 &&
      roles.filter((role) => role === 'assistant').length >= 2,
    'both turns persisted user + assistant messages to AelioDb',
  );

  // 7. Decision journal: the turn's decisions must be reconstructable from
  //    the trace firehose — which prompt was built, from what, and what it
  //    produced. Traces are fire-and-forget, so poll briefly.
  const deadline = Date.now() + 5_000;
  let traceRows = [];
  while (Date.now() < deadline) {
    const traceScan = await client.scanRows(TABLES.harnessTraces, {
      k: 100,
      filters: [{ col: 'turn_id', op: 'eq', value: { type: 'utf8', value: turn2.turnId } }],
    });
    traceRows = traceScan.rows ?? [];
    const kinds = new Set(traceRows.map((row) => row.values.kind?.value));
    if (kinds.has('prompt') && kinds.has('reply')) break;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  const kinds = new Set(traceRows.map((row) => row.values.kind?.value));
  for (const expected of ['pathway', 'stance', 'prompt', 'reply']) {
    assert(kinds.has(expected), `decision journal has a '${expected}' entry for the turn`);
  }
  const promptTrace = JSON.parse(
    traceRows.find((row) => row.values.kind?.value === 'prompt').values.payload.value,
  );
  assert(
    promptTrace.prompt.includes('DE811556677') && promptTrace.hasStance === true,
    'journaled prompt entry carries the full compiled system prompt + section flags',
  );
  const replyTrace = JSON.parse(
    traceRows.find((row) => row.values.kind?.value === 'reply').values.payload.value,
  );
  assert(
    replyTrace.reply === turn2.reply && replyTrace.userMessage === turn2Message,
    'journaled reply entry links the user message to the reply it produced',
  );
  // `overall` is the net across ALL fed categories (urgency/certainty read as
  // positive-valence here), so assert presence + validity, not a fixed sign.
  assert(
    ['positive', 'negative', 'neutral'].includes(replyTrace.stance?.valence),
    `journaled reply entry records the inferred stance (${replyTrace.stance?.valence})`,
  );

  // 8. Generic hot-path gate: a bare "thanks!" must be answered from a template
  //    with ZERO LLM calls, and the decision must be journaled.
  const llmCallsBefore = llmCalls.length;
  const turn3 = await processTurn({ ...base, message: 'thanks!' });
  assert(
    turn3.reply === "You're welcome! Is there anything else I can help with?",
    'generic gate serves the template reply for a bare thanks',
  );
  assert(llmCalls.length === llmCallsBefore, 'generic gate reply used zero LLM calls');
  const gateDeadline = Date.now() + 5_000;
  let genericTrace = null;
  while (Date.now() < gateDeadline && !genericTrace) {
    const gateScan = await client.scanRows(TABLES.harnessTraces, {
      k: 50,
      filters: [{ col: 'turn_id', op: 'eq', value: { type: 'utf8', value: turn3.turnId } }],
    });
    genericTrace = (gateScan.rows ?? []).find((row) => row.values.kind?.value === 'generic') ?? null;
    if (!genericTrace) await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert(Boolean(genericTrace), 'generic gate decision is journaled');
  assert(
    JSON.parse(genericTrace.values.payload.value).kind === 'thanks',
    'journaled gate decision names the matched kind',
  );

  // 9. Span decomposition: in a mixed message, the negative span must win the
  //    sentiment category on its own, with span attribution recorded.
  const mixedMessage =
    'The dashboard looks great and I like the new design. But my invoice VAT charge is still wrong and I am really frustrated.';
  const turn4 = await processTurn({ ...base, message: mixedMessage });
  assert(turn4.reply.length > 0, 'mixed-span turn produced a reply');
  let stanceTrace = null;
  const spanDeadline = Date.now() + 5_000;
  while (Date.now() < spanDeadline && !stanceTrace) {
    const spanScan = await client.scanRows(TABLES.harnessTraces, {
      k: 50,
      filters: [{ col: 'turn_id', op: 'eq', value: { type: 'utf8', value: turn4.turnId } }],
    });
    stanceTrace = (spanScan.rows ?? []).find((row) => row.values.kind?.value === 'stance') ?? null;
    if (!stanceTrace) await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert(Boolean(stanceTrace), 'mixed-span stance decision is journaled');
  const spanStance = JSON.parse(stanceTrace.values.payload.value);
  const sentimentCat = spanStance.categories.find((c) => c.category === 'sentiment');
  assert(
    sentimentCat?.dominant === 'negative' && sentimentCat.fed === true,
    'negative span wins sentiment despite the positive opener',
  );
  assert(
    typeof sentimentCat.span === 'string' && /invoice|frustrated/i.test(sentimentCat.span),
    `sentiment attribution names the offending span ("${sentimentCat.span ?? ''}")`,
  );

  console.log('\n✓ Reply-relevance audit passed: memory, stance, context, pathway, gate, spans and journal all verified.');
}

main()
  .then(() => cleanup(0))
  .catch((error) => {
    console.error(error);
    cleanup(1);
  });
