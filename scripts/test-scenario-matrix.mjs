#!/usr/bin/env node
/**
 * Harness scenario matrix: runs realistic sample chats through the real
 * processTurn path against a temporary AelioDb aelio-server and validates both the
 * reply and the decision journal. This is not a unit test of one component; it
 * is an auditor's sweep across generic gate, semantic pathway, stance,
 * immediate context, memory recall, cache, flow/policy guidance, and traces.
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
  createConvoxResponseCacheStore,
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
const DIM = 96;

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
  archetypes: 'convox_archetypes',
  aspects: 'convox_aspects', axisNodes: 'convox_axis_nodes',
};

let child;
let temp;
const embedCalls = new Map();

const CONCEPTS = [
  ['frustrated', /frustrat|angry|upset|annoyed|unacceptable|broken|terrible|again/i],
  ['billing', /invoice|vat|billing|charge|payment/i],
  ['urgent', /urgent|asap|immediately|right now|today|quickly/i],
  ['refund', /refund|return|order|cancel order/i],
  ['farewell', /bye|goodbye|stop|leave|no more|that'?s all/i],
  ['pricing', /price|pricing|expensive|cost|plan|cheaper|upgrade|downgrade/i],
  ['confused', /confused|lost|don'?t understand|unclear|what do you mean/i],
  ['weather', /weather|forecast|temperature|rain/i],
  ['shipment', /ship|shipping|delivery|tracking|package/i],
  ['security', /login|password|account locked|2fa|security/i],
  ['positive', /thanks|great|love|perfect|awesome|happy|satisfied/i],
  ['continue', /more|continue|tell me|what next|go on|interested/i],
];

function vectorize(text) {
  embedCalls.set(text, (embedCalls.get(text) ?? 0) + 1);
  const vector = new Array(DIM).fill(0);
  CONCEPTS.forEach(([_, pattern], index) => {
    if (pattern.test(text)) vector[index] = index === 0 ? 3 : 1;
  });
  for (const token of text.toLowerCase().split(/\W+/).filter(Boolean)) {
    let hash = 0;
    for (const char of token) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
    vector[16 + (hash % (DIM - 16))] += 0.03;
  }
  const mag = Math.sqrt(vector.reduce((sum, value) => sum + value * value, 0)) || 1;
  return vector.map((value) => value / mag);
}

function assert(condition, message) {
  if (!condition) throw new Error(`FAIL: ${message}`);
}

function ok(message) {
  console.log(`OK: ${message}`);
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

async function waitHealthy(url) {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`${url}/v1/health`);
      if (response.ok) return;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`aelio-server did not become healthy at ${url}`);
}

function cleanup(code) {
  configureEmbedder(null);
  if (child && child.exitCode === null) {
    try {
      child.kill('SIGTERM');
    } catch (error) {
      console.warn(`WARN: unable to stop temporary aelio-server: ${error.message}`);
    }
  }
  if (temp) rmSync(temp, { recursive: true, force: true });
  process.exit(code);
}

function buildSdk() {
  const functions = [
    {
      name: 'update_invoice',
      intent: 'billing_correction',
      description: 'Correct invoice details such as VAT number, billing address, payment charge, or tax ID.',
      safety: 'write',
      parameters: { type: 'object', properties: {} },
    },
    {
      name: 'refund_order',
      intent: 'order_refund',
      description: 'Start a refund or return for an order, including cancellation refund requests.',
      safety: 'write',
      parameters: { type: 'object', properties: {} },
    },
    {
      name: 'get_order_status',
      intent: 'shipping_status',
      description: 'Check shipping, delivery, tracking, and package status for an order.',
      safety: 'read',
      parameters: { type: 'object', properties: {} },
    },
    {
      name: 'compare_plans',
      intent: 'plan_comparison',
      description: 'Compare product plans, pricing, value, upgrades, downgrades, and cheaper options.',
      safety: 'read',
      parameters: { type: 'object', properties: {} },
    },
    {
      name: 'cancel_subscription',
      intent: 'subscription_cancel',
      description: 'Cancel a subscription or stop future renewal.',
      safety: 'destructive',
      parameters: { type: 'object', properties: {} },
    },
    {
      name: 'reset_login',
      intent: 'account_security',
      description: 'Help with login, password reset, locked account, 2FA, and security access.',
      safety: 'write',
      parameters: { type: 'object', properties: {} },
    },
    {
      name: 'get_weather',
      intent: 'weather_lookup',
      description: 'Look up weather forecast, rain, temperature, and climate for a city.',
      safety: 'read',
      parameters: { type: 'object', properties: {} },
    },
  ];
  return {
    getPersona: () => 'You are Aelio, a precise customer-support assistant for Acme SaaS.',
    getFunctions: () => functions,
    getStates: () => [
      {
        id: 'billing_review',
        description: 'Customer is reviewing billing, invoice, pricing, or plan fit.',
        allowedTools: ['update_invoice', 'compare_plans', 'cancel_subscription'],
      },
      {
        id: 'support_recovery',
        description: 'Customer is recovering from a support problem or broken workflow.',
        allowedTools: ['reset_login', 'get_order_status', 'refund_order'],
      },
    ],
    getPolicies: () => [
      {
        id: 'confirm-writes',
        severity: 'hard',
        description: 'Any write, refund, cancellation, account change, or invoice correction requires platform confirmation.',
      },
      {
        id: 'deescalate-frustration',
        severity: 'soft',
        description: 'When the user is frustrated, acknowledge briefly and move to a concrete next step.',
      },
      {
        id: 'no-hard-sell',
        severity: 'soft',
        description: 'When price sensitivity is detected, avoid aggressive selling and offer lower-friction options.',
      },
    ],
    getFlows: () => [
      {
        id: 'plan-change-flow',
        state: 'billing_review',
        description: 'Help the user compare plans and choose a plan change safely.',
        steps: [
          { id: 'understand-value-gap', goal: 'Understand what feels expensive or missing', tool: 'compare_plans' },
          { id: 'confirm-change', goal: 'Confirm any billing-impacting plan change', tool: 'cancel_subscription' },
        ],
      },
      {
        id: 'account-recovery-flow',
        state: 'support_recovery',
        description: 'Recover access or unblock a failed support action.',
        steps: [
          { id: 'diagnose-access', goal: 'Identify the access issue', tool: 'reset_login' },
          { id: 'confirm-resolution', goal: 'Confirm the user can continue' },
        ],
      },
    ],
    invoke: async (name) => ({ ok: true, data: { tool: name, status: 'mocked' }, durationMs: 1 }),
  };
}

function makeLlm(calls) {
  return {
    complete: async (opts) => {
      calls.push(opts);
      const system = opts.system ?? '';
      const user = opts.messages.at(-1)?.content ?? '';
      let text = 'I understand. I will keep this focused and help with the next step.';
      if (/refund|damaged|return/i.test(user)) {
        text = 'I can help with the refund path. I will check the order/refund context and keep confirmation in place for any change.';
      } else if (/invoice|vat|billing|charge/i.test(user)) {
        text = 'I understand the invoice/VAT issue. I will use the billing correction path and make sure any account change asks for confirmation.';
      } else if (/plan|pricing|expensive|cheaper|upgrade|downgrade/i.test(user)) {
        text = 'I can compare the plan options and focus on value without pushing an upgrade.';
      } else if (/login|password|locked|2fa/i.test(user)) {
        text = 'I will help recover account access and keep the steps clear.';
      } else if (/shipping|delivery|tracking|package/i.test(user)) {
        text = 'I can check the shipping status and summarize the delivery state.';
      } else if (/weather|forecast|temperature|rain/i.test(user)) {
        text = 'I can help with the weather lookup.';
      } else if (/disengage|closure|Goodbye|bye/i.test(system + user)) {
        text = 'Goodbye. I will not start a new topic.';
      }
      return { text, toolCalls: [], stopReason: 'stop', usage: { inputTokens: 0, outputTokens: 0 } };
    },
  };
}

async function tracesFor(client, turnId) {
  const deadline = Date.now() + 5_000;
  while (Date.now() < deadline) {
    const scan = await client.scanRows(TABLES.harnessTraces, {
      k: 200,
      filters: [{ col: 'turn_id', op: 'eq', value: { type: 'utf8', value: turnId } }],
    });
    const rows = scan.rows ?? [];
    if (rows.length > 0) {
      return rows.map((row) => ({
        kind: row.values.kind?.value ?? '',
        payload: JSON.parse(row.values.payload?.value || '{}'),
        createdAt: row.values.created_at?.value ?? 0,
      }));
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return [];
}

function findTrace(traces, kind) {
  return traces.find((trace) => trace.kind === kind)?.payload ?? null;
}

function topTool(pathway) {
  return pathway?.tools?.[0]?.name ?? '';
}

function stanceDominant(stance, category) {
  return stance?.categories?.find((entry) => entry.category === category)?.dominant ?? '';
}

async function main() {
  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;
  temp = mkdtempSync(join(tmpdir(), 'aelio-scenario-matrix-'));
  child = spawn(AELIO_SERVER, [], {
    cwd: AELIO_OS,
    env: { ...process.env, AELIO_ALLOW_INSECURE_OPEN: '1', AELIO_RUNTIME_BIND: `127.0.0.1:${port}`, AELIO_DATA_DIR: join(temp, 'data') },
    stdio: 'ignore',
  });
  await waitHealthy(url);
  configureEmbedder(async (text) => vectorize(text));

  const client = new AelioDbClient({ baseUrl: url });
  await bootstrapAelioDbTables(client, TABLES, DIM);
  const storage = { client, tables: TABLES, embedDim: DIM };

  const messageStore = createConvoxMessageStore(storage);
  const sessionStore = createConvoxSessionStore(storage);
  const customerStore = createConvoxCustomerStore(storage);
  const functionCallStore = createConvoxFunctionCallStore(storage);
  const memoryStore = createConvoxMemoryStore(storage);
  const responseCacheStore = createConvoxResponseCacheStore(storage);
  const archetypeStore = createConvoxArchetypeStore(storage);
  await archetypeStore.seedIfEmpty(DEFAULT_ARCHETYPES);

  const llmCalls = [];
  const sdk = buildSdk();
  const base = {
    llm: makeLlm(llmCalls),
    sdk,
    model: 'scenario-mock',
    maxTokens: 4096,
    historyWindow: 20,
    safety: { defaultMode: 'full', requireConfirmationFor: ['write', 'destructive'] },
    identity: { mappingFunction: 'phone', allowAnonymous: true },
    rateLimit: { perCustomerPerMinute: 200, perCustomerPerDay: 2000 },
    idleTimeoutMinutes: 60,
    summarizeAfter: 100,
    memoryRecallLimit: 5,
    memoryEnabled: true,
    intent: { enabled: true, ttlMinutes: 20, maxDepth: 5 },
    cache: { enabled: false, similarityThreshold: 0.995, ttlMinutes: 60 },
    messageStore,
    sessionStore,
    customerStore,
    functionCallStore,
    memoryStore,
    responseCacheStore,
    contextEngine: createImmediateContextEngine({ storage }),
    pathwayEngine: createSemanticPathwayEngine({ memoryStore }),
    archetypeEngine: createArchetypeEngine({ store: archetypeStore }),
    tracer: new HarnessTracer({
      client,
      table: TABLES.harnessTraces,
      tenant: 'scenario-matrix',
      embedDim: DIM,
    }),
    tracePrompt: 'full',
    channel: 'web',
  };

  const customerExternalId = `matrix-${Date.now()}`;
  const channelAddress = `web:${customerExternalId}`;

  async function runTurn(label, message, extra = {}) {
    const beforeLlm = llmCalls.length;
    const result = await processTurn({
      ...base,
      ...extra,
      customerExternalId,
      channelAddress,
      message,
    });
    const traces = await tracesFor(client, result.turnId);
    return {
      label,
      message,
      reply: result.reply,
      turnId: result.turnId,
      llmCalls: llmCalls.length - beforeLlm,
      traces,
      pathway: findTrace(traces, 'pathway'),
      stance: findTrace(traces, 'stance'),
      prompt: findTrace(traces, 'prompt'),
      replyTrace: findTrace(traces, 'reply'),
      generic: findTrace(traces, 'generic'),
      cache: findTrace(traces, 'cache'),
    };
  }

  const rows = [];
  function record(row) {
    rows.push(row);
    ok(`${row.label}: ${row.observed}`);
  }

  const s1 = await runTurn('Generic greeting', 'hi');
  assert(s1.generic?.kind === 'greeting', 'greeting should hit generic gate');
  assert(s1.llmCalls === 0, 'greeting should not call LLM');
  record({ label: s1.label, message: s1.message, observed: 'generic=greeting, llm=0', reply: s1.reply });

  const s2 = await runTurn('Generic thanks', 'thanks!');
  assert(s2.generic?.kind === 'thanks', 'thanks should hit generic gate');
  assert(s2.llmCalls === 0, 'thanks should not call LLM');
  record({ label: s2.label, message: s2.message, observed: 'generic=thanks, llm=0', reply: s2.reply });

  const customer = await customerStore.getByExternalId(customerExternalId);
  assert(customer, 'customer should exist before memory seeding');
  const vatMemory = 'Customer correct invoice VAT registration is DE811556677 and billing changes need email confirmation.';
  await memoryStore.store({
    customerId: customer.id,
    content: vatMemory,
    category: 'billing',
    sourceSessionId: 'seed',
    embedding: await embed(vatMemory),
  });

  const s3Message = 'I am really frustrated — my invoice has the wrong VAT number again. Fix it immediately.';
  const s3 = await runTurn('Frustrated VAT correction', s3Message);
  assert(topTool(s3.pathway) === 'update_invoice', 'billing tool should rank first');
  assert(stanceDominant(s3.stance, 'sentiment') === 'negative', 'frustration should be negative sentiment');
  assert(s3.prompt?.prompt?.includes('DE811556677'), 'VAT memory should reach prompt');
  assert(/invoice|VAT|billing/i.test(s3.reply), 'reply should mention billing/VAT path');
  record({ label: s3.label, message: s3.message, observed: `tool=${topTool(s3.pathway)}, sentiment=negative, memory=VAT`, reply: s3.reply });

  const s4 = await runTurn('Refund request', 'Order 8842 arrived damaged and I need a refund right now.');
  assert(topTool(s4.pathway) === 'refund_order', 'refund tool should rank first');
  assert(s4.pathway?.policies?.some((p) => p.id === 'confirm-writes'), 'hard confirmation policy should survive');
  assert(/refund/i.test(s4.reply), 'reply should mention refund');
  record({ label: s4.label, message: s4.message, observed: `tool=${topTool(s4.pathway)}, policy=confirm-writes`, reply: s4.reply });

  await customerStore.updateMetadata(customer.id, {
    lifecycleState: 'billing_review',
    lifecycleStateReason: 'scenario matrix',
    flowProgress: { 'plan-change-flow': { currentStepIndex: 0, completedSteps: [] } },
  });
  const s5 = await runTurn('Active plan flow', 'This plan feels too expensive. Can you compare cheaper options?');
  assert(s5.pathway?.flow?.id === 'plan-change-flow', 'billing flow should be selected');
  assert(topTool(s5.pathway) === 'compare_plans', 'compare_plans should rank first');
  assert(/plan|value|upgrade|options/i.test(s5.reply), 'reply should discuss plans/options');
  record({ label: s5.label, message: s5.message, observed: `flow=${s5.pathway.flow.id}, tool=${topTool(s5.pathway)}`, reply: s5.reply });

  await customerStore.updateMetadata(customer.id, {
    lifecycleState: 'support_recovery',
    lifecycleStateReason: 'scenario matrix',
    flowProgress: { 'account-recovery-flow': { currentStepIndex: 0, completedSteps: [] } },
  });
  const s6 = await runTurn('Confused login recovery', "I'm confused and locked out of login. I don't understand the reset step.");
  assert(s6.pathway?.flow?.id === 'account-recovery-flow', 'account recovery flow should be selected');
  assert(topTool(s6.pathway) === 'reset_login', 'reset_login should rank first');
  assert(stanceDominant(s6.stance, 'certainty') === 'negative', 'confusion should produce negative certainty');
  record({ label: s6.label, message: s6.message, observed: `flow=${s6.pathway.flow.id}, tool=${topTool(s6.pathway)}, certainty=negative`, reply: s6.reply });

  await customerStore.updateMetadata(customer.id, {});
  const s7 = await runTurn('Shipping lookup', 'Where is my package? Please check the shipping status.');
  assert(topTool(s7.pathway) === 'get_order_status', 'shipping tool should rank first');
  assert(/shipping|delivery/i.test(s7.reply), 'reply should mention shipping');
  record({ label: s7.label, message: s7.message, observed: `tool=${topTool(s7.pathway)}`, reply: s7.reply });

  const s8 = await runTurn('Weather off-domain but valid tool', 'What is the weather forecast for Paris tomorrow?');
  assert(topTool(s8.pathway) === 'get_weather', 'weather tool should rank first');
  assert(/weather/i.test(s8.reply), 'reply should mention weather');
  record({ label: s8.label, message: s8.message, observed: `tool=${topTool(s8.pathway)}`, reply: s8.reply });

  const repeated = 'Can you explain what the invoice charge means?';
  const s9a = await runTurn('Cache seed', repeated, {
    cache: { enabled: true, similarityThreshold: 0.995, ttlMinutes: 60 },
  });
  assert(!s9a.cache, 'first cache turn should miss');
  const beforeRepeatLlm = llmCalls.length;
  const s9b = await runTurn('Cache hit', repeated, {
    cache: { enabled: true, similarityThreshold: 0.995, ttlMinutes: 60 },
  });
  assert(s9b.cache?.action === 'served_cached_response', 'repeat should serve cached response');
  assert(llmCalls.length === beforeRepeatLlm, 'cache hit should not call LLM');
  record({ label: s9b.label, message: s9b.message, observed: 'cache=hit, llm=0', reply: s9b.reply });

  const s10 = await runTurn('Disengagement', "That's all, goodbye.");
  assert(s10.pathway?.strategy === 'disengage' || s10.generic?.kind === 'farewell', 'goodbye should disengage or generic farewell');
  assert(s10.generic || s10.pathway?.proactive?.action === 'suppress', 'goodbye should suppress re-engagement when semantic path runs');
  record({
    label: s10.label,
    message: s10.message,
    observed: s10.generic ? 'generic=farewell' : `strategy=${s10.pathway.strategy}, proactive=suppress`,
    reply: s10.reply,
  });

  const s11 = await runTurn('Mixed sentiment span', 'The dashboard is usable. But my invoice charge is still wrong and I am really frustrated.');
  const sentiment = s11.stance?.categories?.find((entry) => entry.category === 'sentiment');
  assert(
    sentiment?.dominant === 'negative',
    `mixed message should pick negative sentiment (observed ${JSON.stringify(sentiment)})`,
  );
  assert(/invoice|frustrated/i.test(sentiment?.span ?? ''), 'stance should name the negative span');
  record({
    label: s11.label,
    message: s11.message,
    observed: `sentiment=negative, span="${sentiment.span}"`,
    reply: s11.reply,
  });

  console.log('\n| # | Scenario | User Message | Decision Observed | Reply Preview |');
  console.log('|---:|---|---|---|---|');
  rows.forEach((row, index) => {
    const esc = (value) => String(value).replace(/\|/g, '\\|').replace(/\s+/g, ' ').trim();
    console.log(`| ${index + 1} | ${esc(row.label)} | ${esc(row.message)} | ${esc(row.observed)} | ${esc(row.reply).slice(0, 140)} |`);
  });
  console.log('\n✓ Harness scenario matrix passed.');
}

main()
  .then(() => cleanup(0))
  .catch((error) => {
    console.error(error);
    cleanup(1);
  });
