#!/usr/bin/env node
/**
 * Ten real-LLM Harness interactions against an isolated live AelioDb.
 *
 * Uses the configured OPENAI_API_KEY for response generation. Retrieval is
 * deterministic so observations isolate the Harness decisions while replies
 * come from the real model. Writes a redacted Markdown report to docs/.
 */
import { spawn } from 'node:child_process';
import { createServer } from 'node:net';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
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
  createConvoxCustomerStore,
  createConvoxFunctionCallStore,
  createConvoxMemoryStore,
  createConvoxMessageStore,
  createConvoxResponseCacheStore,
  createConvoxSessionStore,
  createImmediateContextEngine,
  createSemanticPathwayEngine,
  HarnessTracer,
  processTurn,
  seedBuiltinArchetypes,
  SuspensionStore,
  DEFAULT_ARCHETYPES,
  embed,
} from '@aelio/core';
import { createLLMProvider } from '@aelio/llm';
import { AelioDbClient } from '@aelio/db-client';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const AELIO_OS = join(ROOT, 'aelio-os');
const AELIO_SERVER = join(AELIO_OS, 'target/release/aelio-server');
const REPORT_PATH = join(ROOT, 'docs/HARNESS_10_REAL_LLM_OBSERVATION.md');
const DIM = 128;

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
  aspects: 'convox_aspects',
  axisNodes: 'convox_axis_nodes',
};

const CONCEPTS = [
  /frustrat|angry|upset|annoyed|unacceptable|broken|terrible|again/i,
  /invoice|vat|billing|charge|payment/i,
  /urgent|asap|immediately|right now|today|quickly/i,
  /refund|return|order|cancel order/i,
  /bye|goodbye|stop|leave|no more|that'?s all/i,
  /price|pricing|expensive|cost|plan|cheaper|upgrade|downgrade/i,
  /confused|lost|don'?t understand|unclear|what do you mean/i,
  /weather|forecast|temperature|rain/i,
  /ship|shipping|delivery|tracking|package/i,
  /login|password|account locked|2fa|security/i,
  /thanks|great|love|perfect|awesome|happy|satisfied/i,
  /continue|still|more|tell me|what next|go on|interested/i,
];

let child;
let temp;

function vectorize(text) {
  const vector = new Array(DIM).fill(0);
  CONCEPTS.forEach((pattern, index) => {
    if (pattern.test(text)) vector[index] = index === 0 ? 3 : 1;
  });
  for (const token of text.toLowerCase().split(/\W+/).filter(Boolean)) {
    let hash = 0;
    for (const char of token) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
    vector[16 + (hash % (DIM - 16))] += 0.03;
  }
  const magnitude = Math.sqrt(vector.reduce((sum, value) => sum + value * value, 0)) || 1;
  return vector.map((value) => value / magnitude);
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
      if ((await fetch(`${url}/v1/health`)).ok) return;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error('temporary aelio-server did not become healthy');
}

function buildSdk() {
  const functions = [
    {
      name: 'update_invoice',
      intent: 'billing_correction',
      description: 'Correct invoice VAT number, billing address, tax ID, payment charge.',
      safety: 'write',
      parameters: { type: 'object', properties: {} },
    },
    {
      name: 'refund_order',
      intent: 'order_refund',
      description: 'Start a refund or return for a damaged order.',
      safety: 'write',
      parameters: { type: 'object', properties: {} },
    },
    {
      name: 'compare_plans',
      intent: 'plan_comparison',
      description: 'Compare pricing, value, upgrades, downgrades, and cheaper plans.',
      safety: 'read',
      parameters: { type: 'object', properties: {} },
    },
    {
      name: 'reset_login',
      intent: 'account_security',
      description: 'Recover login, reset password, unlock account, or fix 2FA.',
      safety: 'write',
      parameters: { type: 'object', properties: {} },
    },
    {
      name: 'get_order_status',
      intent: 'shipping_status',
      description: 'Check shipping, delivery, tracking, and package status.',
      safety: 'read',
      parameters: { type: 'object', properties: {} },
    },
  ];
  return {
    getPersona: () =>
      'You are Aelio, a concise and empathetic customer-support assistant for Acme SaaS. Use only supplied customer facts. Never claim an action was completed unless a tool result says so.',
    getFunctions: () => functions,
    getStates: () => [
      {
        id: 'billing_review',
        description: 'Customer is reviewing invoices, pricing, or plan fit.',
        allowedTools: ['update_invoice', 'compare_plans'],
      },
      {
        id: 'support_recovery',
        description: 'Customer is recovering from access or order problems.',
        allowedTools: ['reset_login', 'get_order_status', 'refund_order'],
      },
    ],
    getPolicies: () => [
      {
        id: 'confirm-writes',
        severity: 'hard',
        description: 'Any refund, invoice correction, or account change requires confirmation.',
      },
      {
        id: 'deescalate',
        severity: 'soft',
        description: 'Acknowledge frustration briefly and move to a concrete next step.',
      },
      {
        id: 'no-hard-sell',
        severity: 'soft',
        description: 'When price sensitivity is present, avoid aggressive selling.',
      },
    ],
    getFlows: () => [
      {
        id: 'plan-change-flow',
        state: 'billing_review',
        description: 'Compare plans and change safely.',
        steps: [
          {
            id: 'understand-value-gap',
            goal: 'Understand what feels expensive or missing',
            tool: 'compare_plans',
          },
          {
            id: 'confirm-change',
            goal: 'Confirm a billing-impacting change',
            tool: 'compare_plans',
          },
        ],
      },
      {
        id: 'account-recovery-flow',
        state: 'support_recovery',
        description: 'Recover access or resolve a failed support action.',
        steps: [
          { id: 'diagnose', goal: 'Identify the access problem', tool: 'reset_login' },
          { id: 'confirm-resolution', goal: 'Confirm access works' },
        ],
      },
    ],
    invoke: async (name) => ({
      ok: true,
      data: { tool: name, status: 'observation-simulated-success' },
      durationMs: 1,
    }),
  };
}

async function tracesFor(client, turnId) {
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    const scan = await client.scanRows(TABLES.harnessTraces, {
      k: 300,
      filters: [{ col: 'turn_id', op: 'eq', value: { type: 'utf8', value: turnId } }],
    });
    const rows = (scan.rows ?? []).map((row) => ({
      kind: row.values.kind?.value ?? '',
      payload: JSON.parse(row.values.payload?.value || '{}'),
    }));
    if (rows.some((row) => row.kind === 'reply') || rows.some((row) => row.kind === 'generic')) {
      return rows;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return [];
}

function trace(rows, kind) {
  return rows.find((row) => row.kind === kind)?.payload ?? null;
}

function esc(value) {
  return String(value ?? '')
    .replace(/\|/g, '\\|')
    .replace(/\r?\n/g, '<br>')
    .trim();
}

async function main() {
  try {
    process.loadEnvFile(join(ROOT, '.env'));
  } catch {}
  const apiKey = process.env.OPENAI_API_KEY;
  if (!apiKey) {
    throw new Error('OPENAI_API_KEY is missing; refusing to call a mock provider');
  }
  const model = process.env.AELIO_OBSERVATION_MODEL || 'gpt-4o-mini';
  const provider = createLLMProvider({
    provider: 'openai',
    model,
    apiKey,
    maxTokens: 800,
  });
  const llmObservations = [];
  const llm = {
    complete: async (options) => {
      const observation = {
        purpose: options.telemetry?.purpose ?? 'unspecified',
        durationMs: null,
        usage: null,
        stopReason: null,
      };
      llmObservations.push(observation);
      const started = performance.now();
      const result = await provider.complete(options);
      observation.durationMs = performance.now() - started;
      observation.usage = result.usage ?? null;
      observation.stopReason = result.stopReason ?? null;
      return result;
    },
  };

  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;
  temp = mkdtempSync(join(tmpdir(), 'aelio-real-observation-'));
  child = spawn(AELIO_SERVER, [], {
    cwd: AELIO_OS,
    env: {
      ...process.env,
      AELIO_ALLOW_INSECURE_OPEN: '1',
      AELIO_RUNTIME_BIND: `127.0.0.1:${port}`,
      AELIO_DATA_DIR: join(temp, 'data'),
    },
    stdio: 'ignore',
  });
  child.on('error', () => {});
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
  const aspectStore = createConvoxAspectStore(storage);
  const archetypeStore = createConvoxArchetypeStore(storage);
  const axisStore = createConvoxAxisStore(storage);
  const suspensionStore = new SuspensionStore({
    aelioDb: {
      client,
      table: TABLES.harnessSuspensions,
      tenant: 'real-observation',
    },
  });
  await seedBuiltinArchetypes(aspectStore, archetypeStore, DEFAULT_ARCHETYPES);

  const base = {
    llm,
    sdk: buildSdk(),
    model,
    maxTokens: 800,
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
    harness: {
      enabled: true,
      budgets: {
        maxInstructions: 12,
        maxReplans: 2,
        maxRecoilsPerIntent: 3,
        maxToolCalls: 15,
        wallClockMs: 60_000,
        maxTurnTokens: 30_000,
      },
      binding: {
        scoreMin: 0.55,
        ambiguityGap: 0.08,
        cacheTtlMinutes: 1440,
      },
    },
    messageStore,
    sessionStore,
    customerStore,
    functionCallStore,
    memoryStore,
    responseCacheStore,
    contextEngine: createImmediateContextEngine({ storage }),
    pathwayEngine: createSemanticPathwayEngine({ memoryStore }),
    archetypeEngine: createArchetypeEngine({ store: archetypeStore, aspectStore }),
    axisStore,
    suspensionStore,
    ledgerAelioDb: {
      client,
      table: TABLES.harnessLedger,
      tenant: 'real-observation',
    },
    tracer: new HarnessTracer({
      client,
      table: TABLES.harnessTraces,
      tenant: 'real-observation',
      embedDim: DIM,
    }),
    tracePrompt: 'redacted',
    channel: 'web',
  };

  const runId = Date.now();
  const externalIds = new Map();
  const identityFor = (group) => {
    if (!externalIds.has(group)) {
      externalIds.set(group, `real-observation-${runId}-${group}`);
    }
    const customerExternalId = externalIds.get(group);
    return {
      customerExternalId,
      channelAddress: `web:${customerExternalId}`,
    };
  };
  const scenarios = [
    { group: 'billing', label: 'Generic greeting', message: 'Hi' },
    {
      group: 'billing',
      label: 'Frustrated VAT correction',
      message:
        'I am really frustrated — my invoice has the wrong VAT number again. Please help me fix it today.',
    },
    {
      group: 'billing',
      label: 'Harness confirmation resume',
      message: 'yes',
    },
    {
      group: 'billing',
      label: 'Explicit temporal continuation',
      message: 'Earlier today you fixed my invoice. Which VAT number did you use?',
    },
    {
      group: 'mixed',
      label: 'Mixed sentiment spans',
      message:
        'The dashboard is useful. But the billing charge is still wrong and I am very annoyed. Just explain it; do not change anything.',
    },
    {
      group: 'refund',
      label: 'Refund request',
      message: 'Order 8842 arrived damaged and I need a refund right now.',
    },
    {
      group: 'refund',
      label: 'Refund confirmation resume',
      message: 'yes',
    },
    {
      group: 'plan',
      label: 'Plan price concern',
      message:
        'The Pro plan feels too expensive. Can you compare cheaper options without upselling me?',
    },
    {
      group: 'login',
      label: 'Confused login recovery',
      message: "I'm locked out and confused. I don't understand the password reset step.",
    },
    {
      group: 'closure',
      label: 'Conversation closure',
      message: "That's all, goodbye.",
    },
  ];

  const results = [];
  for (let index = 0; index < scenarios.length; index += 1) {
    const { group, label, message } = scenarios[index];
    const identity = identityFor(group);

    if (index === 1) {
      const customer = await customerStore.getByExternalId(identity.customerExternalId);
      if (customer) {
        const fact =
          'The verified VAT registration for this observation customer is DE811556677.';
        await memoryStore.store({
          customerId: customer.id,
          content: fact,
          category: 'billing',
          sourceSessionId: 'observation-seed',
          embedding: await embed(fact),
        });
      }
    }
    if (index === 7) {
      await customerStore.ensureCustomer(
        identity.customerExternalId,
        'web',
        identity.channelAddress,
      );
      const customer = await customerStore.getByExternalId(identity.customerExternalId);
      if (customer) {
        await customerStore.updateMetadata(customer.id, {
          lifecycleState: 'billing_review',
          lifecycleStateReason: 'real observation',
          flowProgress: {
            'plan-change-flow': { currentStepIndex: 0, completedSteps: [] },
          },
        });
      }
    }
    if (index === 8) {
      await customerStore.ensureCustomer(
        identity.customerExternalId,
        'web',
        identity.channelAddress,
      );
      const customer = await customerStore.getByExternalId(identity.customerExternalId);
      if (customer) {
        await customerStore.updateMetadata(customer.id, {
          lifecycleState: 'support_recovery',
          lifecycleStateReason: 'real observation',
          flowProgress: {
            'account-recovery-flow': { currentStepIndex: 0, completedSteps: [] },
          },
        });
      }
    }

    const beforeLlm = llmObservations.length;
    const started = performance.now();
    const turn = await processTurn({
      ...base,
      ...identity,
      message,
    });
    const totalMs = performance.now() - started;
    await new Promise((resolve) => setTimeout(resolve, 1500));
    const traces = await tracesFor(client, turn.turnId);
    const pathway = trace(traces, 'pathway');
    const stance = trace(traces, 'stance');
    const temporal = trace(traces, 'temporal');
    const evidence = trace(traces, 'evidence');
    const generic = trace(traces, 'generic');
    const replyTrace = trace(traces, 'reply');
    const callsStartedDuringTurn = llmObservations.slice(beforeLlm);
    const replyCalls = callsStartedDuringTurn.filter(
      (call) => call.purpose !== 'aspect_discovery',
    );
    const discoveryCalls = callsStartedDuringTurn.filter(
      (call) => call.purpose === 'aspect_discovery',
    );
    const llmCallCount = replyCalls.length;

    results.push({
      index: index + 1,
      group,
      label,
      message,
      reply: turn.reply,
      totalMs,
      llmCallCount,
      discoveryCallCount: discoveryCalls.length,
      llmDurationMs:
        llmCallCount > 0
          ? replyCalls.reduce((sum, call) => sum + (call.durationMs ?? 0), 0)
          : null,
      generic: generic?.kind ?? null,
      intent: pathway?.intent?.label ?? null,
      strategy: pathway?.strategy ?? null,
      tool: pathway?.tools?.[0]?.name ?? null,
      flow: pathway?.flow?.id ?? null,
      proactive: pathway?.proactive?.action ?? null,
      sentiment:
        stance?.categories?.find((category) => category.category === 'sentiment')
          ?.dominant ?? null,
      stanceCategories: stance?.categories ?? [],
      temporal,
      evidenceItems: evidence?.items ?? [],
      pendingConfirmation: replyTrace?.pendingConfirmation ?? null,
    });

    console.log(`\n[${index + 1}/10] ${label}`);
    console.log(`USER: ${message}`);
    console.log(`ASSISTANT: ${turn.reply}`);
    console.log(
      `OBSERVED: llm=${llmCallCount}, intent=${pathway?.intent?.label ?? '-'}, ` +
        `strategy=${pathway?.strategy ?? '-'}, tool=${pathway?.tools?.[0]?.name ?? '-'}, ` +
        `sentiment=${results.at(-1)?.sentiment ?? '-'}, evidence=${evidence?.items?.length ?? 0}, ` +
        `total=${totalMs.toFixed(0)}ms`,
    );
  }

  // Aspect discovery is deliberately fire-and-forget in the turn path. Give
  // those observations time to finish before tearing down their AelioDb store.
  await new Promise((resolve) => setTimeout(resolve, 3000));
  const axes = (
    await Promise.all(
      [...externalIds.values()].map(async (externalId) => {
        const customer = await customerStore.getByExternalId(externalId);
        return customer ? axisStore.listAxes(customer.id) : [];
      }),
    )
  ).flat();
  const llmTurns = results.filter((row) => row.llmCallCount > 0);
  const discoveryCalls = llmObservations.filter(
    (call) => call.purpose === 'aspect_discovery',
  ).length;
  const avgLlm =
    llmTurns.reduce((sum, row) => sum + (row.llmDurationMs ?? 0), 0) /
    Math.max(1, llmTurns.length);
  const avgTotal =
    results.reduce((sum, row) => sum + row.totalMs, 0) / results.length;

  const lines = [
    '# Harness: 10 Real-LLM Interaction Observation',
    '',
    `- Recorded: ${new Date().toISOString()}`,
    `- Model: ${model}`,
    `- LLM turns: ${llmTurns.length}/10 (generic turns bypassed the model)`,
    `- Asynchronous aspect-discovery LLM calls: ${discoveryCalls}`,
    `- Mean LLM latency: ${avgLlm.toFixed(0)} ms`,
    `- Mean total turn latency: ${avgTotal.toFixed(0)} ms`,
    `- Customer axes created: ${[...new Set(axes.map((axis) => axis.aspectName))].join(', ') || 'none'}`,
    '- Prompts are redacted in traces; no API keys are recorded.',
    '',
    '| # | Scenario | User | Assistant | Observed decision |',
    '|---:|---|---|---|---|',
    ...results.map((row) => {
      const observed = [
        `llm=${row.llmCallCount}`,
        row.generic ? `generic=${row.generic}` : null,
        row.intent ? `intent=${row.intent}` : null,
        row.strategy ? `strategy=${row.strategy}` : null,
        row.tool ? `tool=${row.tool}` : null,
        row.flow ? `flow=${row.flow}` : null,
        row.sentiment ? `sentiment=${row.sentiment}` : null,
        row.discoveryCallCount ? `discovery_llm=${row.discoveryCallCount}` : null,
        `evidence=${row.evidenceItems.length}`,
        `total=${row.totalMs.toFixed(0)}ms`,
      ]
        .filter(Boolean)
        .join(', ');
      return `| ${row.index} | ${esc(row.label)} | ${esc(row.message)} | ${esc(row.reply)} | ${esc(observed)} |`;
    }),
    '',
    '## Detailed decisions',
    '',
    ...results.flatMap((row) => [
      `### ${row.index}. ${row.label}`,
      '',
      `- Temporal: ${esc(row.temporal?.reason ?? 'generic bypass')}`,
      `- Proactive hint: ${esc(row.proactive ?? 'none')}`,
      `- Pending confirmation: ${esc(row.pendingConfirmation ?? 'none')}`,
      `- Stance categories: ${esc(
        row.stanceCategories
          .filter((category) => category.fed)
          .map(
            (category) =>
              `${category.category}:${category.dominant}(${Number(category.strength).toFixed(2)})`,
          )
          .join(', ') || 'none',
      )}`,
      `- Evidence: ${esc(
        row.evidenceItems
          .map((item) => `${item.source}:${Number(item.score).toFixed(2)}`)
          .join(', ') || 'none',
      )}`,
      '',
    ]),
  ];
  writeFileSync(REPORT_PATH, `${lines.join('\n')}\n`, 'utf8');
  console.log(`\nReport written: ${REPORT_PATH}`);
  console.log(
    JSON.stringify({
      model,
      llmTurns: llmTurns.length,
      avgLlmMs: Number(avgLlm.toFixed(1)),
      avgTotalMs: Number(avgTotal.toFixed(1)),
      axes: [...new Set(axes.map((axis) => axis.aspectName))],
    }),
  );
}

main()
  .then(() => {
    configureEmbedder(null);
    try {
      child?.kill('SIGTERM');
    } catch {}
    if (temp) rmSync(temp, { recursive: true, force: true });
  })
  .catch((error) => {
    console.error(error);
    configureEmbedder(null);
    try {
      child?.kill('SIGTERM');
    } catch {}
    if (temp) rmSync(temp, { recursive: true, force: true });
    process.exitCode = 1;
  });
