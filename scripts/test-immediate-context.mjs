#!/usr/bin/env node
/**
 * Immediate Context Engine smoke test against a live aelio-server.
 *
 * Verifies:
 *  1. Hot window (last 5 min) renders verbatim
 *  2. Aged messages slide into the correct tier buckets
 *  3. Buckets persist in convox_compactions and reappear after a fresh engine
 *  4. Prompt factory places immediate context in the system prompt
 */

import { spawn } from 'node:child_process';
import { createServer } from 'node:net';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import {
  bootstrapAelioDbTables,
  createImmediateContextEngine,
  buildTurnSystemPrompt,
  HOT_WINDOW_MS,
  CONTEXT_TIERS,
} from '@aelio/core';
import { AelioDbClient } from '@aelio/db-client';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const AELIO_OS = join(ROOT, 'aelio-os');
const AELIO_SERVER_BIN = join(AELIO_OS, 'target/release/aelio-server');

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
  archetypes: 'convox_archetypes', aspects: 'convox_aspects', axisNodes: 'convox_axis_nodes',
};

const MINUTE = 60_000;
const EMBED_DIM = 8;

let llChild = null;
let tmpRoot = null;

function ok(msg) {
  console.log(`OK: ${msg}`);
}

function fail(msg) {
  console.error(`FAIL: ${msg}`);
  cleanup(1);
}

function cleanup(code = 0) {
  if (llChild) {
    try {
      llChild.kill('SIGTERM');
    } catch {
      // ignore
    }
    llChild = null;
  }
  if (tmpRoot) {
    try {
      rmSync(tmpRoot, { recursive: true, force: true });
    } catch {
      // ignore
    }
    tmpRoot = null;
  }
  process.exit(code);
}

process.on('SIGINT', () => cleanup(130));
process.on('SIGTERM', () => cleanup(143));

function getFreePort() {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      server.close((err) => (err ? reject(err) : resolve(port)));
    });
    server.on('error', reject);
  });
}

async function waitForHealth(url, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`${url}/v1/health`);
      if (res.ok) {
        const body = await res.json();
        if (body.status === 'ok') return;
      }
    } catch {
      // retry
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`aelio-server not healthy at ${url}`);
}

async function insertMessage(client, table, { customerId, role, content, createdAt }) {
  await client.insertRow(table, {
    message_id: { type: 'utf8', value: randomUUID() },
    session_id: { type: 'utf8', value: randomUUID() },
    customer_id: { type: 'utf8', value: customerId },
    role: { type: 'utf8', value: role },
    content: { type: 'utf8', value: content },
    channel: { type: 'utf8', value: 'web' },
    tier: { type: 'i64', value: 0 },
    created_at: { type: 'i64', value: createdAt },
  });
}

async function main() {
  const port = await getFreePort();
  const aelioDbUrl = `http://127.0.0.1:${port}`;
  tmpRoot = mkdtempSync(join(tmpdir(), 'aelio-ice-'));
  const llDataDir = join(tmpRoot, 'aelioDb');

  llChild = spawn(AELIO_SERVER_BIN, [], {
    cwd: AELIO_OS,
    env: {
      ...process.env,
      AELIO_ALLOW_INSECURE_OPEN: '1',
      AELIO_DATA_DIR: llDataDir,
      AELIO_RUNTIME_BIND: `127.0.0.1:${port}`,
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  llChild.stderr?.on('data', () => {});
  llChild.stdout?.on('data', () => {});

  await waitForHealth(aelioDbUrl);
  ok(`aelio-server healthy at ${aelioDbUrl}`);

  const client = new AelioDbClient({ baseUrl: aelioDbUrl });
  await bootstrapAelioDbTables(client, TABLES, EMBED_DIM);
  ok('tables bootstrapped');

  const storage = { client, tables: TABLES, embedDim: EMBED_DIM };
  const engine = createImmediateContextEngine({ storage, snapshotTtlMs: 1 });
  const customerId = `ice-test-${randomUUID()}`;
  const now = Date.now();

  // Hot window (2 min ago)
  await insertMessage(client, TABLES.messages, {
    customerId,
    role: 'user',
    content: 'HOT: I need a refund for order 4421',
    createdAt: now - 2 * MINUTE,
  });
  await insertMessage(client, TABLES.messages, {
    customerId,
    role: 'assistant',
    content: 'HOT: Sure, I can help with that refund.',
    createdAt: now - 90_000,
  });

  // Tier 1 (5–15m): 8 minutes ago
  await insertMessage(client, TABLES.messages, {
    customerId,
    role: 'user',
    content: 'T1: My shipping address is 12 Oak Street',
    createdAt: now - 8 * MINUTE,
  });
  await insertMessage(client, TABLES.messages, {
    customerId,
    role: 'assistant',
    content: 'T1: Got it, address updated to 12 Oak Street.',
    createdAt: now - 7 * MINUTE,
  });

  // Tier 2 (15–30m): 20 minutes ago
  await insertMessage(client, TABLES.messages, {
    customerId,
    role: 'user',
    content: 'T2: Prefer email over SMS for updates',
    createdAt: now - 20 * MINUTE,
  });

  // Tier 3 (30–60m): 45 minutes ago
  await insertMessage(client, TABLES.messages, {
    customerId,
    role: 'user',
    content: 'T3: Looking at the Pro plan pricing',
    createdAt: now - 45 * MINUTE,
  });

  // Tier 4 (1–24h): 3 hours ago
  await insertMessage(client, TABLES.messages, {
    customerId,
    role: 'user',
    content: 'T4: First asked about enterprise SSO yesterday-ish',
    createdAt: now - 3 * 60 * MINUTE,
  });

  const snap = await engine.getContext(customerId, now);
  console.log('--- prompt preview ---');
  console.log(snap.prompt.slice(0, 800));
  console.log('--- end preview ---');
  console.log('buckets:', snap.buckets);
  console.log('hotMessages:', snap.hotMessages);

  if (snap.hotMessages < 2) {
    fail(`expected ≥2 hot messages, got ${snap.hotMessages}`);
  }
  if (!snap.prompt.includes('HOT: I need a refund')) {
    fail('hot window missing verbatim user message');
  }
  if (!snap.prompt.includes('last 5 minutes')) {
    fail('hot window label missing from prompt');
  }

  const tierNums = new Set(snap.buckets.map((b) => b.tier));
  for (const expected of [1, 2, 3, 4]) {
    if (!tierNums.has(expected)) {
      fail(`missing tier ${expected} bucket (got tiers ${[...tierNums]})`);
    }
  }

  if (!snap.prompt.includes('12 Oak Street')) {
    fail('tier-1 content missing from prompt');
  }
  if (!snap.prompt.includes('Prefer email')) {
    fail('tier-2 content missing from prompt');
  }
  if (!snap.prompt.includes('Pro plan')) {
    fail('tier-3 content missing from prompt');
  }
  if (!snap.prompt.includes('enterprise SSO')) {
    fail('tier-4 content missing from prompt');
  }
  ok(`all ${CONTEXT_TIERS.length} tiers present + hot window (${HOT_WINDOW_MS / MINUTE}m)`);

  // Persistence: fresh engine should reload buckets from AelioDb
  const engine2 = createImmediateContextEngine({ storage, snapshotTtlMs: 1 });
  const snap2 = await engine2.getContext(customerId, now + 1_000);
  if (snap2.buckets.length < 4) {
    fail(`fresh engine only saw ${snap2.buckets.length} buckets (expected ≥4)`);
  }
  if (!snap2.prompt.includes('12 Oak Street')) {
    fail('fresh engine lost tier-1 content from AelioDb');
  }
  ok('buckets reloaded from convox_compactions');

  // Prompt factory wiring
  const system = buildTurnSystemPrompt({
    persona: 'You are Aelio.',
    toolGuidance: 'Use tools when needed.',
    immediateContext: snap.prompt,
    summary: 'Earlier session summary about onboarding.',
    memories: 'Customer prefers metric units.',
  });
  if (!system.includes('IMMEDIATE CONVERSATION CONTEXT')) {
    fail('prompt factory did not include immediate context block');
  }
  if (!system.includes('HOT: I need a refund')) {
    fail('prompt factory dropped hot-window content');
  }
  // Immediate context should appear before the rolling summary (higher priority
  // under the volatile group — composer preserves original order within group).
  const iceIdx = system.indexOf('IMMEDIATE CONVERSATION CONTEXT');
  const summaryIdx = system.indexOf('Rolling session summary');
  if (iceIdx < 0 || summaryIdx < 0 || iceIdx > summaryIdx) {
    fail('immediate context should render before the session summary');
  }
  ok('prompt factory orders immediate context ahead of session summary');

  // Slide: advance the clock so tier-1 content ages into tier-2 and verify re-tier
  const later = now + 12 * MINUTE; // 8m-ago message is now ~20m old → tier 2
  const snap3 = await engine.getContext(customerId, later);
  const t2After = snap3.buckets.find((b) => b.tier === 2);
  if (!t2After) {
    fail('after clock advance, tier-2 bucket missing');
  }
  if (!snap3.prompt.includes('12 Oak Street')) {
    fail('Oak Street content lost after slide');
  }
  ok('clock advance slides content into older tiers without loss');

  console.log(`\n✓ Immediate Context Engine smoke passed (${CONTEXT_TIERS.length} tiers + hot cache + prompt factory).`);
  cleanup(0);
}

main().catch((err) => {
  console.error(err);
  cleanup(1);
});
