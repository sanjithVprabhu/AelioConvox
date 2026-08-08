#!/usr/bin/env node
/**
 * Aelio Runtime fault-injection, concurrency, and recovery suite.
 *
 * The happy-path suite (`test-aelio-runtime.mjs`) proves the runtime works. This one tries to
 * break it, against a real Aelio DB:
 *
 *   A. crash at EVERY storage boundary of a write turn — no duplicate side effect, ever
 *   B. concurrent turns for one subject — exactly one commits, the snapshot advances by one
 *   C. provider redelivery storm — one claim, one effect, regardless of arrival order
 *   D. crash mid-dispatch — an unverifiable customer send parks as `unknown`, never resends
 *   E. crash mid-dispatch on a deduplicating channel — safely retried instead
 *   F. artifact crash between nodes — resumes at the next node, never re-runs a completed one
 *   G. detached child spawn + join — parent parks, child wakes it, output arrives
 *   H. Aelio DB restart — WAL recovery preserves state, ledger, outbox, and idempotency exactly
 *   I. schema migrations — applied once, locked against concurrent migrators, fail closed on a
 *      newer schema or an edited migration, and refuse destructive steps by default
 *   J. cross-channel continuity — one subject over web, WhatsApp, and SDK shares durable state
 *   K. tenant isolation — no read, resume, or inference across tenants
 *   L. restore drill — flush to segments, hard restart, and every runtime row hashes identically
 *
 * Usage: node scripts/test-aelio-runtime-faults.mjs
 */

import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync, existsSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash, randomUUID } from 'node:crypto';
import {
  AelioMigrationRunner,
  AelioRuntimeStore,
  AelioSuspensionStore,
  aelioSchemaMigrations,
  bootstrapSunjetTables,
  createAelioConductor,
  dispatchPendingEffects,
  migrateAelioStorage,
  MigrationLockedError,
  runConductorEvent,
} from '@aelio/core';
import { SunjetClient } from '@aelio/sunjet-client';
// The real production runner, not a reimplementation of it. This script therefore runs under tsx.
import { runHarnessEffect, resumeHarnessInstance } from '../server/src/runtime-artifact-runner.js';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const TENANT = 'aelio-fault-test';
const EMBED_DIM = 1536;

let passed = 0;
const failures = [];
const check = (condition, message) => {
  if (condition) {
    passed += 1;
    console.log(`  OK  ${message}`);
  } else {
    failures.push(message);
    console.error(`  FAIL ${message}`);
  }
};
const section = (title) => console.log(`\n— ${title}`);
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

// ---------------------------------------------------------------- Aelio DB lifecycle

async function waitForHealth(url, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      if ((await fetch(`${url}/v1/health`)).ok) return true;
    } catch {
      // not listening yet
    }
    await sleep(200);
  }
  return false;
}

function llServerBinary() {
  const binary = ['target/release/ll-server', 'target/debug/ll-server']
    .map((relative) => join(ROOT, 'Sunjet/Astrolobe', relative))
    .filter((candidate) => existsSync(candidate))
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
  if (!binary) throw new Error('No ll-server binary found; run `cargo build -p ll-server` in Sunjet/Astrolobe.');
  return binary;
}

/** A restartable Aelio DB over a stable data directory, so recovery can actually be exercised. */
async function startAelioDb() {
  const binary = llServerBinary();
  const port = 19900 + Math.floor(Math.random() * 90);
  const dataDir = mkdtempSync(join(tmpdir(), 'aelio-fault-test-'));
  const url = `http://127.0.0.1:${port}`;
  let child = null;

  const boot = async () => {
    child = spawn(binary, [], {
      env: { ...process.env, LL_DATA_DIR: dataDir, LL_BIND: `127.0.0.1:${port}` },
      stdio: ['ignore', 'ignore', 'pipe'],
    });
    let log = '';
    child.stderr.on('data', (chunk) => { log += chunk.toString(); });
    if (!(await waitForHealth(url))) {
      child.kill('SIGKILL');
      throw new Error(`Aelio DB did not become healthy on ${url}.\n${log}`);
    }
  };

  await boot();
  return {
    url,
    /** SIGKILL: no graceful shutdown, so restart must come back through WAL recovery alone. */
    hardRestart: async () => {
      child.kill('SIGKILL');
      await sleep(400);
      await boot();
    },
    stop: async () => {
      child?.kill('SIGKILL');
      await sleep(200);
      rmSync(dataDir, { recursive: true, force: true });
    },
  };
}

function tableNames(suffix) {
  const name = (base) => `${base}_${suffix}`;
  return {
    messages: name('messages'), conversations: name('conversations'), memories: name('memories'),
    compactions: name('compactions'), runtimeState: name('runtime_state'),
    harnessTools: name('harness_tools'), harnessCapabilities: name('harness_capabilities'),
    harnessBindings: name('harness_bindings'), harnessSuspensions: name('harness_suspensions'),
    harnessLedger: name('harness_ledger'), harnessTraces: name('harness_traces'),
    runtimeEvents: name('runtime_events'), runtimeSnapshots: name('runtime_snapshots'),
    runtimeLedger: name('runtime_ledger'), runtimeOutbox: name('runtime_outbox'),
    runtimeContinuations: name('runtime_continuations'), scheduledEvents: name('scheduled_events'),
    workflowArtifacts: name('workflow_artifacts'), workflowInstances: name('workflow_instances'),
    promptArtifacts: name('prompt_artifacts'), promptLedger: name('prompt_ledger'),
    migrations: name('schema_migrations'),
  };
}

// ---------------------------------------------------------------- Fault injection

class InjectedCrash extends Error {
  constructor(op) {
    super(`injected crash before storage op #${op}`);
    this.name = 'InjectedCrash';
  }
}

/**
 * Wrap a storage client so the Nth mutating call dies before it reaches Aelio DB.
 *
 * Killing the call *before* it is sent is the honest model of a process that died mid-turn: the
 * transaction never reached the WAL, and everything already committed stays committed.
 */
function crashingClient(baseUrl, crashAtOp) {
  const inner = new SunjetClient({ baseUrl });
  let ops = 0;
  const proxy = Object.create(Object.getPrototypeOf(inner));
  Object.assign(proxy, inner);
  proxy.transact = async (...args) => {
    ops += 1;
    if (ops === crashAtOp) throw new InjectedCrash(ops);
    return SunjetClient.prototype.transact.apply(inner, args);
  };
  proxy.scanRows = (...args) => SunjetClient.prototype.scanRows.apply(inner, args);
  proxy.insertRow = (...args) => SunjetClient.prototype.insertRow.apply(inner, args);
  proxy.ensureTable = (...args) => SunjetClient.prototype.ensureTable.apply(inner, args);
  proxy.getSchema = (...args) => SunjetClient.prototype.getSchema.apply(inner, args);
  return { client: proxy, opCount: () => ops };
}

// ---------------------------------------------------------------- Tenant doubles

function scriptedLlm(script) {
  return {
    complete: async (options) => {
      const purpose = options.telemetry?.purpose ?? 'chat_completion';
      if (purpose === 'plan' || purpose === 'replan') {
        return {
          text: '', stopReason: 'tool_use', usage: { inputTokens: 5, outputTokens: 5 },
          toolCalls: [{ id: randomUUID(), name: 'emit_turn', args: script.plan() }],
        };
      }
      if (purpose === 'synthesis') {
        return { text: script.synthesis(), toolCalls: [], stopReason: 'stop', usage: { inputTokens: 3, outputTokens: 3 } };
      }
      return { text: 'ok', toolCalls: [], stopReason: 'stop', usage: { inputTokens: 1, outputTokens: 1 } };
    },
  };
}

function stubSdk(options = {}) {
  const invocations = [];
  return {
    invocations,
    getFunctions: () => options.functions ?? [],
    getStates: () => [], getPolicies: () => [], getFlows: () => [],
    getPersona: () => 'Fault-injection test assistant.',
    invoke: async (name, args) => {
      invocations.push({ name, args });
      return { ok: true, data: { done: true }, durationMs: 1 };
    },
  };
}

const WRITE_TOOL = {
  name: 'cancel_order', description: 'Cancel an order.', intent: 'cancel an order', safety: 'write',
  parameters: [{ name: 'order_id', type: 'string', required: true, description: 'The order id.' }],
};

const harness = {
  enabled: true,
  budgets: { maxInstructions: 8, maxReplans: 1, maxRecoilsPerIntent: 2, maxToolCalls: 8, wallClockMs: 30_000, maxTurnTokens: 100_000 },
  binding: { scoreMin: 0.2, ambiguityGap: 0.05, cacheTtlMinutes: 10 },
};

const conductorConfig = (overrides) => ({
  model: 'test-model', maxTokens: 512, historyWindow: 10,
  safety: { defaultMode: 'confirm_writes', requireConfirmationFor: ['write', 'destructive'], overrides: [] },
  harness, memoryEnabled: false, ...overrides,
});

const event = (subjectId, message, idempotencyKey, extra = {}) => ({
  apiVersion: 'aelio.runtime.event/v1',
  eventId: randomUUID(), idempotencyKey, tenantId: TENANT, subjectId,
  kind: 'user_message', payload: { message, channel: 'web', ...extra }, receivedAt: Date.now(),
});

const object = (value) => (value && typeof value === 'object' && !Array.isArray(value) ? value : {});

const cancelPlan = () => ({
  mode: 'plan', goal: 'Cancel order 42',
  instructions: [{ id: 'a', capability: 'cancel an order', tool: 'cancel_order', args_hint: { order_id: '42' } }],
});

/**
 * Content hash of every runtime table, row by row. Sorting the per-row hashes makes the digest
 * independent of scan order, so a difference means the DATA changed, not the ordering.
 */
async function hashRuntimeTables(client, tables) {
  const runtimeTables = [
    tables.runtimeEvents, tables.runtimeSnapshots, tables.runtimeLedger, tables.runtimeOutbox,
    tables.runtimeContinuations, tables.scheduledEvents, tables.workflowArtifacts, tables.workflowInstances,
  ];
  const byTable = {};
  let total = 0;
  for (const table of runtimeTables) {
    const rows = await client.scanRows(table, { k: 1024 });
    const perRow = rows.rows
      .map((row) => createHash('sha256').update(JSON.stringify(canonicalRow(row.values))).digest('hex'))
      .sort();
    total += perRow.length;
    byTable[table] = createHash('sha256').update(perRow.join('|')).digest('hex').slice(0, 32);
  }
  return { byTable, total };
}

/** Key-sorted so two encodings of the same row hash the same. Vectors are reduced to a length. */
function canonicalRow(values) {
  return Object.keys(values)
    .sort()
    .map((key) => {
      const value = values[key];
      if (value?.type === 'vector') return [key, `vector(${value.value.length})`];
      return [key, value];
    });
}

/** The narrow slice of RuntimeDeps the artifact runner needs for compute-only artifacts. */
function artifactDeps(store) {
  return {
    runtimeStore: store,
    llm: { complete: async () => { throw new Error('no prompt node in these artifacts'); } },
    sdkBridge: {
      getFunctions: () => [], getStates: () => [], getPolicies: () => [], getFlows: () => [],
      invoke: async () => { throw new Error('no tool node in these artifacts'); },
      hasSendCapability: () => false,
    },
    config: { name: TENANT, llm: { model: 'test-model', max_tokens: 256 } },
  };
}

// ---------------------------------------------------------------- The suite

async function main() {
  const db = await startAelioDb();
  console.log(`Aelio DB: ${db.url}`);
  const client = new SunjetClient({ baseUrl: db.url });
  const tables = tableNames(String(Date.now()).slice(-8));
  await bootstrapSunjetTables(client, tables, EMBED_DIM);

  const store = new AelioRuntimeStore(client, tables);
  const suspensionStore = new AelioSuspensionStore(client, tables, TENANT);

  try {
    // ---- A. Crash at every storage boundary of a confirmed write ---------------------------
    section('A. Crash at every storage boundary of a confirmed write');
    {
      // First, learn how many storage operations a clean park+confirm turn performs.
      const probeSubject = `subject-${randomUUID()}`;
      const probe = crashingClient(db.url, Number.POSITIVE_INFINITY);
      const probeStore = new AelioRuntimeStore(probe.client, tables);
      const probeSuspension = new AelioSuspensionStore(probe.client, tables, TENANT);
      const probeSdk = stubSdk({ functions: [WRITE_TOOL] });
      const probeConductor = createAelioConductor(conductorConfig({
        llm: scriptedLlm({ plan: cancelPlan, synthesis: () => 'Cancelled.' }),
        sdk: probeSdk, suspensionStore: probeSuspension, effectJournal: probeStore,
      }));
      await runConductorEvent(probeStore, probeConductor, event(probeSubject, 'cancel order 42', `k-${randomUUID()}`));
      await runConductorEvent(probeStore, probeConductor, event(probeSubject, 'yes', `k-${randomUUID()}`));
      const boundaries = probe.opCount();
      check(probeSdk.invocations.length === 1, 'baseline turn executed the write exactly once');
      check(boundaries >= 3, `the turn has ${boundaries} storage boundaries to crash at`);

      let survived = 0;
      let duplicated = 0;
      let stuck = 0;
      for (let crashAt = 1; crashAt <= boundaries; crashAt += 1) {
        const subject = `subject-${randomUUID()}`;
        const sdk = stubSdk({ functions: [WRITE_TOOL] });
        const llm = scriptedLlm({ plan: cancelPlan, synthesis: () => 'Cancelled.' });
        const parkKey = `k-${randomUUID()}`;
        const confirmKey = `k-${randomUUID()}`;

        // Run the two-turn write with the process "dying" at boundary `crashAt`.
        const crashed = crashingClient(db.url, crashAt);
        const crashedStore = new AelioRuntimeStore(crashed.client, tables);
        const crashedSuspension = new AelioSuspensionStore(crashed.client, tables, TENANT);
        const crashedConductor = createAelioConductor(conductorConfig({
          llm, sdk, suspensionStore: crashedSuspension, effectJournal: crashedStore,
        }));
        for (const [message, key] of [['cancel order 42', parkKey], ['yes', confirmKey]]) {
          try {
            await runConductorEvent(crashedStore, crashedConductor, event(subject, message, key));
          } catch (error) {
            if (!(error instanceof InjectedCrash)) throw error;
            break;
          }
        }
        const writesBeforeRecovery = sdk.invocations.length;

        // Recover: a fresh process retries both messages with the SAME idempotency keys.
        const recoveredConductor = createAelioConductor(conductorConfig({
          llm, sdk, suspensionStore, effectJournal: store,
        }));
        for (const [message, key] of [['cancel order 42', parkKey], ['yes', confirmKey]]) {
          await runConductorEvent(store, recoveredConductor, event(subject, message, key));
        }

        if (sdk.invocations.length > 1) {
          duplicated += 1;
          console.error(`  crash@${crashAt}: ${sdk.invocations.length} writes (was ${writesBeforeRecovery} before recovery)`);
        } else if (sdk.invocations.length === 1) {
          survived += 1;
        } else {
          stuck += 1;
          console.error(`  crash@${crashAt}: the write never happened, even after recovery`);
        }
      }
      check(duplicated === 0, `no crash boundary produced a duplicate write (${boundaries} boundaries tested)`);
      check(stuck === 0, 'every crash boundary still completed the write after recovery');
      check(survived === boundaries, `all ${boundaries} crash boundaries recovered to exactly one write`);
    }

    // ---- B. Concurrent turns for one subject -------------------------------------------------
    section('B. Concurrent turns for one subject');
    {
      const subject = `subject-${randomUUID()}`;
      const conductors = Array.from({ length: 6 }, () =>
        createAelioConductor(conductorConfig({
          llm: scriptedLlm({ plan: () => ({ mode: 'reply', text: 'hi' }), synthesis: () => '' }),
          sdk: stubSdk(), suspensionStore,
        })));
      // No session lock here on purpose: this tests Aelio DB's CAS, not the in-process lock.
      const results = await Promise.all(
        conductors.map((conductor) => runConductorEvent(store, conductor, event(subject, 'hello', `k-${randomUUID()}`))),
      );
      const committed = results.filter((result) => result.status === 'committed');
      const conflicted = results.filter((result) => result.status === 'conflict');
      check(committed.length === 1, `exactly one of 6 concurrent turns committed (got ${committed.length})`);
      check(conflicted.length === 5, `the other 5 were rejected as conflicts (got ${conflicted.length})`);
      const snapshot = await store.loadSnapshot(TENANT, subject);
      check(snapshot?.revision === 1, 'the snapshot advanced by exactly one revision');
      check(snapshot?.state.history?.length === 2, 'no lost update: history holds one turn, not six');
    }

    // ---- C. Provider redelivery storm ---------------------------------------------------------
    section('C. Provider redelivery storm');
    {
      const subject = `subject-${randomUUID()}`;
      const key = `provider-${randomUUID()}`;
      const sdk = stubSdk({ functions: [WRITE_TOOL] });
      const conductor = createAelioConductor(conductorConfig({
        llm: scriptedLlm({ plan: () => ({ mode: 'reply', text: 'ack' }), synthesis: () => '' }),
        sdk, suspensionStore,
      }));
      const results = await Promise.all(
        Array.from({ length: 5 }, () =>
          runConductorEvent(store, conductor, event(subject, 'same message', key, {
            delivery: { channel: 'whatsapp', to: '+15550001' },
          }))),
      );
      const committed = results.filter((result) => result.status === 'committed');
      check(committed.length === 1, `5 redeliveries of one provider id produced 1 commit (got ${committed.length})`);
      const outbox = await store.listPendingEffects(64, Date.now() + 60_000);
      const replies = outbox.filter((effect) => effect.subjectId === subject && effect.kind === 'reply.send');
      check(replies.length === 1, `exactly one outbound reply was queued (got ${replies.length})`);
    }

    // ---- D. Crash mid-dispatch on a channel with no provider idempotency ----------------------
    section('D. Crash mid-dispatch — WhatsApp send outcome unknown');
    {
      const subject = `subject-${randomUUID()}`;
      const effectId = randomUUID();
      await store.commit({
        event: event(subject, 'notify me', `k-${randomUUID()}`),
        state: {}, instanceId: randomUUID(), ledger: [],
        effects: [{
          effectId, idempotencyKey: `reply:${effectId}`, kind: 'reply.send',
          payload: { channel: 'whatsapp', to: '+15550002', text: 'your order shipped' },
        }],
      });
      const [pending] = (await store.listPendingEffects(64)).filter((e) => e.effectId === effectId);
      // Lease it, then "die": never complete, never fail. The lease is already expired.
      const leased = await store.claimEffect(pending, 'worker-that-dies', -1_000);
      check(leased !== null, 'the effect was leased for dispatch');

      let sends = 0;
      await dispatchPendingEffects(store, async (dispatched) => {
        if (dispatched.effectId === effectId) sends += 1;
      });
      check(sends === 0, 'the reaper did not resend a message whose delivery cannot be verified');
      const unknown = (await store.listUnknownEffects(64)).filter((e) => e.effectId === effectId);
      check(unknown.length === 1, 'the ambiguous send was parked as `unknown` for reconciliation');
      check(
        (unknown[0]?.lastError ?? '').includes('reconciliation'),
        'the unknown effect records why it needs a human or a reconcile job',
      );
      check(await store.resolveUnknownEffect(unknown[0], 'delivered', 'confirmed in provider console'), 'an operator can reconcile it');
      check((await store.listUnknownEffects(64)).every((e) => e.effectId !== effectId), 'the reconciled effect left the unknown queue');
    }

    // ---- E. Crash mid-dispatch on an internal effect ------------------------------------------
    section('E. Crash mid-dispatch — internal effect retries safely');
    {
      const subject = `subject-${randomUUID()}`;
      const effectId = randomUUID();
      await store.commit({
        event: event(subject, 'start work', `k-${randomUUID()}`),
        state: {}, instanceId: randomUUID(), ledger: [],
        effects: [{
          effectId, idempotencyKey: `continuation:${effectId}`, kind: 'continuation.create',
          payload: { token: `t-${effectId}`, prompt: 'which size?', expiresAt: Date.now() + 60_000, instanceId: randomUUID() },
        }],
      });
      const [pending] = (await store.listPendingEffects(64)).filter((e) => e.effectId === effectId);
      await store.claimEffect(pending, 'worker-that-dies', -1_000);

      let handled = 0;
      await dispatchPendingEffects(store, async (dispatched) => {
        if (dispatched.effectId === effectId) handled += 1;
      });
      check(handled === 1, 'an internal effect was safely re-driven after the crash');
      check((await store.listUnknownEffects(64)).every((e) => e.effectId !== effectId), 'an internal effect never becomes unknown');
    }

    // ---- F/G. Artifact crash between nodes, and detached child join ---------------------------
    section('F. Artifact resumes at the next node after a crash');
    {
      const artifact = {
        apiVersion: 'aelio.runtime.artifact/v1', artifactId: 'fault.three_steps', version: '1.0.0',
        digest: 'fault-digest', status: 'approved', entryNodeId: 'one',
        nodes: [
          { id: 'one', type: 'compute', operation: 'sum', input: { $ref: 'input' }, next: 'two' },
          { id: 'two', type: 'compute', operation: 'multiply', input: [{ $ref: 'one' }, 2], next: 'three' },
          { id: 'three', type: 'compute', operation: 'add', input: [{ $ref: 'two' }, 1], next: 'done' },
          { id: 'done', type: 'end', output: { $ref: 'three' } },
        ],
        createdAt: Date.now(),
      };
      await store.installArtifact(artifact);

      const instanceId = `instance-${randomUUID()}`;
      const subject = `subject-${randomUUID()}`;
      await store.createInstance({
        instanceId, tenantId: TENANT, subjectId: subject,
        artifactId: artifact.artifactId, artifactVersion: artifact.version,
        status: 'running', state: { input: [1, 2, 3], cursor: { nodeId: 'one', values: { input: [1, 2, 3] }, steps: 0 } },
      });

      // Drive one node, then "crash": reload from storage and continue.
      const { startArtifactCursor, stepRuntimeArtifact } = await import('@aelio/core');
      const host = {
        memory: { get: async () => null, set: async () => {}, delete: async () => false },
        runPrompt: async () => { throw new Error('unexpected'); },
        runTool: async () => { throw new Error('unexpected'); },
        spawn: async () => { throw new Error('unexpected'); },
        joinChildren: async () => ({ ready: true, outputs: {} }),
      };
      let instance = await store.loadInstance(TENANT, instanceId);
      let cursor = startArtifactCursor(artifact, [1, 2, 3]);
      const visited = [];
      for (let step = 0; step < 8; step += 1) {
        const result = await stepRuntimeArtifact(artifact, cursor, host);
        visited.push(cursor.nodeId);
        if (result.status === 'completed') {
          await store.commitInstanceTransition({
            instance, eventId: randomUUID(), status: 'completed',
            state: { input: [1, 2, 3], output: result.output }, ledger: result.ledger,
          });
          break;
        }
        cursor = result.cursor;
        await store.commitInstanceTransition({
          instance, eventId: randomUUID(), status: 'running',
          state: { input: [1, 2, 3], cursor }, ledger: result.ledger,
        });
        // Simulate the crash: drop every in-memory handle and reload the durable cursor.
        instance = await store.loadInstance(TENANT, instanceId);
        cursor = instance.state.cursor;
      }
      const finished = await store.loadInstance(TENANT, instanceId);
      check(finished?.status === 'completed', 'the artifact completed across simulated crashes');
      check(finished?.state.output === 13, 'each node ran exactly once: (1+2+3)*2+1 = 13');
      check(new Set(visited).size === visited.length, 'no node was replayed after a crash');
    }

    section('G. Detached child spawn and join');
    {
      const child = {
        apiVersion: 'aelio.runtime.artifact/v1', artifactId: 'fault.child', version: '1.0.0',
        digest: 'child-digest', status: 'approved', entryNodeId: 'double',
        nodes: [
          { id: 'double', type: 'compute', operation: 'multiply', input: { $ref: 'input' }, next: 'done' },
          { id: 'done', type: 'end', output: { $ref: 'double' } },
        ],
        createdAt: Date.now(),
      };
      const parent = {
        apiVersion: 'aelio.runtime.artifact/v1', artifactId: 'fault.parent', version: '1.0.0',
        digest: 'parent-digest', status: 'approved', entryNodeId: 'kick',
        nodes: [
          { id: 'kick', type: 'spawn', artifactId: 'fault.child', version: '1.0.0', input: [5, 2], mode: 'child', next: 'wait' },
          { id: 'wait', type: 'join', spawnNodes: ['kick'], next: 'done' },
          { id: 'done', type: 'end', output: { $ref: 'wait' } },
        ],
        createdAt: Date.now(),
      };
      check(await store.installArtifact(child), 'child artifact installed');
      check(await store.installArtifact(parent), 'parent artifact with a join installed');

      // A join that names a node which is not a spawn must be rejected at admission.
      const invalid = { ...parent, artifactId: 'fault.bad_join', nodes: [
        { id: 'kick', type: 'compute', operation: 'sum', input: [1], next: 'wait' },
        { id: 'wait', type: 'join', spawnNodes: ['kick'], next: 'done' },
        { id: 'done', type: 'end', output: null },
      ] };
      let rejected = false;
      try {
        await store.installArtifact(invalid);
      } catch (error) {
        rejected = /not a spawn node/.test(String(error));
      }
      check(rejected, 'a join that awaits a non-spawn node is rejected at admission');

      // Drive the real runner: parent spawns a detached child, parks on the join, and the child's
      // completion wakes it through the outbox.
      const subject = `subject-${randomUUID()}`;
      const parentInstanceId = `instance-${randomUUID()}`;
      const deps = artifactDeps(store);
      const startEvent = randomUUID();
      await store.commit({
        event: event(subject, 'run parent', `k-${randomUUID()}`),
        state: {}, instanceId: parentInstanceId, ledger: [],
        effects: [{
          effectId: `${parentInstanceId}:start`, idempotencyKey: `harness.start:${parentInstanceId}`,
          kind: 'harness.start',
          payload: { artifactId: 'fault.parent', version: '1.0.0', input: null, instanceId: parentInstanceId },
        }],
      });

      // Pump the outbox the way the worker does, until it quiesces.
      let drained = 0;
      for (let pass = 0; pass < 12; pass += 1) {
        const outcome = await dispatchPendingEffects(store, async (effect) => {
          if (effect.kind === 'harness.start') return runHarnessEffect(deps, effect);
          if (effect.kind === 'harness.resume') return resumeHarnessInstance(deps, effect);
          // Anything else in the queue belongs to an earlier section; leave it alone.
          return undefined;
        });
        drained += outcome.delivered;
        const parentNow = await store.loadInstance(TENANT, parentInstanceId);
        if (parentNow?.status === 'completed') break;
      }

      const finishedParent = await store.loadInstance(TENANT, parentInstanceId);
      check(finishedParent?.status === 'completed', 'the parent completed after its child finished');
      const childId = `${parentInstanceId}:kick`;
      const finishedChild = await store.loadInstance(TENANT, childId);
      check(finishedChild?.status === 'completed', 'the detached child actually ran');
      check(finishedChild?.parentInstanceId === parentInstanceId, 'the child records its parent');
      check(finishedChild?.state.output === 10, 'the child computed 5*2 = 10');
      check(finishedParent?.state.output?.[childId] === 10, "the join delivered the child's output to the parent");
      check(startEvent !== '' && drained > 0, 'the run was driven entirely through the durable outbox');

      // A redelivered wake must not re-drive a finished parent.
      const wakes = (await store.listPendingEffects(64, Date.now() + 60_000)).filter((e) => e.kind === 'harness.resume');
      check(wakes.length === 0, 'no wake effect was left pending after the join resolved');
    }

    // ---- I. Schema migrations -------------------------------------------------------------------
    section('I. Schema migration ledger and lock');
    {
      // A fresh namespace so migration state is not shared with the bootstrapped tables above.
      const migTables = tableNames(`mig${String(Date.now()).slice(-6)}`);
      const first = await migrateAelioStorage(client, migTables, EMBED_DIM);
      check(first.applied.length === 3, `first boot applied all 3 migrations (got ${first.applied.length})`);
      check(first.schemaVersion === 3, 'schema version is 3 after first boot');

      const second = await migrateAelioStorage(client, migTables, EMBED_DIM);
      check(second.applied.length === 0, 'a second boot applies nothing');
      check(second.skipped.length === 3, 'a second boot recognises all 3 as already applied');

      const runner = new AelioMigrationRunner(client, migTables, 'owner-a');
      check((await runner.schemaVersion()) === 3, 'the recorded schema version is readable');

      // An older binary must refuse to boot against a schema it does not know.
      let refusedFuture = false;
      try {
        await runner.migrate(aelioSchemaMigrations(EMBED_DIM).slice(0, 2));
      } catch (error) {
        refusedFuture = /does not know about/.test(String(error));
      }
      check(refusedFuture, 'an older build refuses to boot against a newer schema');

      // An applied migration whose definition changed must be caught, not silently ignored.
      const edited = aelioSchemaMigrations(EMBED_DIM).map((migration) =>
        migration.id === '0002-harness-catalog' ? { ...migration, description: 'edited after shipping' } : migration);
      let refusedEdit = false;
      try {
        await runner.migrate(edited);
      } catch (error) {
        refusedEdit = /different definition/.test(String(error));
      }
      check(refusedEdit, 'an edited already-applied migration is refused');

      // Destructive steps are refused unless explicitly enabled.
      let destructiveRan = false;
      const destructive = [
        ...aelioSchemaMigrations(EMBED_DIM),
        {
          id: '0004-drop-something', version: 4, kind: 'destructive',
          description: 'A non-additive change.',
          apply: async () => { destructiveRan = true; },
        },
      ];
      let refusedDestructive = false;
      try {
        await runner.migrate(destructive);
      } catch (error) {
        refusedDestructive = /destructive/.test(String(error));
      }
      check(refusedDestructive, 'a destructive migration is refused by default');
      check(!destructiveRan, 'the refused destructive migration did not run');

      const allowed = await runner.migrate(destructive, { allowDestructive: true });
      check(destructiveRan, 'the destructive migration runs when explicitly allowed');
      check(allowed.schemaVersion === 4, 'schema version advanced to 4');

      // Two migrators cannot hold the lock at once; the loser is told who holds it.
      const lockTables = tableNames(`lock${String(Date.now()).slice(-6)}`);
      await client.ensureTable(lockTables.migrations, [
        { name: 'migration_id', kind: 'utf8' }, { name: 'version', kind: 'i64' }, { name: 'status', kind: 'utf8' },
        { name: 'checksum', kind: 'utf8' }, { name: 'applied_at', kind: 'i64' }, { name: 'lease_owner', kind: 'utf8' },
        { name: 'lease_expires_at', kind: 'i64' }, { name: 'revision', kind: 'i64' }, { name: 'description', kind: 'utf8' },
      ]);
      const slow = new AelioMigrationRunner(client, lockTables, 'owner-slow');
      let held = null;
      const blocker = slow.migrate([{
        id: 'lock-test', version: 1, kind: 'additive', description: 'holds the lock',
        apply: async () => { held = new Promise((done) => setTimeout(done, 700)); await held; },
      }]);
      await sleep(150);
      let lockRefused = false;
      try {
        await new AelioMigrationRunner(client, lockTables, 'owner-fast').migrate([{
          id: 'lock-test-2', version: 2, kind: 'additive', description: 'wants the lock',
          apply: async () => {},
        }]);
      } catch (error) {
        lockRefused = error instanceof MigrationLockedError;
      }
      await blocker;
      check(lockRefused, 'a second migrator is refused while the lock is held');

      // Once released, the lock is takeable again.
      const after = await new AelioMigrationRunner(client, lockTables, 'owner-fast').migrate([
        { id: 'lock-test', version: 1, kind: 'additive', description: 'holds the lock', apply: async () => {} },
        { id: 'lock-test-2', version: 2, kind: 'additive', description: 'wants the lock', apply: async () => {} },
      ]);
      check(after.applied.length === 1 && after.applied[0].id === 'lock-test-2', 'the released lock lets the next migrator finish the job');
    }

    // ---- J. Cross-channel continuity --------------------------------------------------------------
    section('J. Cross-channel subject continuity');
    {
      const subject = `subject-${randomUUID()}`;
      const sdk = stubSdk();
      const conductor = createAelioConductor(conductorConfig({
        llm: scriptedLlm({ plan: () => ({ mode: 'reply', text: 'noted' }), synthesis: () => '' }),
        sdk, suspensionStore, effectJournal: store,
      }));

      // The same subject arrives on three channels. Web is synchronous; the other two ask for
      // durable delivery.
      const web = await runConductorEvent(store, conductor, event(subject, 'from the website', `k-${randomUUID()}`));
      const wa = await runConductorEvent(store, conductor, event(subject, 'now from whatsapp', `k-${randomUUID()}`, {
        channel: 'whatsapp', delivery: { channel: 'whatsapp', to: '+15550100' },
      }));
      const sdkTurn = await runConductorEvent(store, conductor, event(subject, 'and from the sdk', `k-${randomUUID()}`, {
        channel: 'sdk', delivery: { channel: 'slack', to: 'U123' },
      }));
      check([web, wa, sdkTurn].every((result) => result.status === 'committed'), 'all three channel turns committed');

      const snapshot = await store.loadSnapshot(TENANT, subject);
      check(snapshot?.revision === 3, 'one subject, one state, three revisions');
      check(snapshot?.state.turns === 3, 'the turn counter is shared across channels');
      const contents = (snapshot?.state.history ?? []).map((entry) => entry.content);
      check(
        contents.includes('from the website') && contents.includes('now from whatsapp') && contents.includes('and from the sdk'),
        'history from every channel is in the one shared subject state',
      );
      check(snapshot?.state.lastChannel === 'sdk', 'the snapshot records the channel of the latest turn');

      // Rendering stays channel-specific: web replies synchronously, the others go via the outbox.
      const queued = (await store.listPendingEffects(64, Date.now() + 60_000)).filter((effect) => effect.subjectId === subject);
      const channels = queued.map((effect) => object(effect.payload).channel).sort();
      check(queued.length === 2, `only the two asynchronous channels queued a delivery (got ${queued.length})`);
      check(JSON.stringify(channels) === JSON.stringify(['slack', 'whatsapp']), 'each queued delivery targets its own channel');
    }

    // ---- K. Tenant isolation -----------------------------------------------------------------------
    section('K. Tenant isolation');
    {
      const subject = `shared-${randomUUID()}`;
      const other = 'aelio-other-tenant';
      const conductor = createAelioConductor(conductorConfig({
        llm: scriptedLlm({ plan: () => ({ mode: 'reply', text: 'tenant a secret' }), synthesis: () => '' }),
        sdk: stubSdk(), suspensionStore, effectJournal: store,
      }));
      await runConductorEvent(store, conductor, event(subject, 'tenant A private message', `k-${randomUUID()}`));

      // Deliberately the SAME subject id under a different tenant: isolation must not depend on
      // subject ids happening to be unique.
      check((await store.loadSnapshot(other, subject)) === null, 'tenant B cannot read tenant A\'s subject snapshot');
      check((await store.loadInstance(other, subject)) === null, 'tenant B cannot read tenant A\'s instances');
      check((await store.listSubjectInstances(other, subject)).length === 0, 'tenant B lists no instances for the shared subject id');
      check((await store.listLedgerForEvent(other, 'any-event')).length === 0, 'tenant B reads none of tenant A\'s ledger');

      const token = `wake-${randomUUID()}`;
      await store.createContinuation({
        token, tenantId: TENANT, subjectId: subject, instanceId: randomUUID(),
        prompt: 'tenant A question', payload: {}, expiresAt: Date.now() + 60_000,
      });
      check((await store.consumeContinuation(other, token)) === null, 'tenant B cannot consume tenant A\'s continuation');
      check((await store.consumeContinuation(TENANT, token)) !== null, 'tenant A still can');

      const otherSuspension = new AelioSuspensionStore(client, tables, other);
      check((await otherSuspension.get(`${TENANT}:${subject}`)) === null, 'a parked plan is not visible to another tenant');

      // The same idempotency key is a different logical event for a different tenant.
      const sharedKey = `k-shared-${randomUUID()}`;
      const a = await store.commit({ event: event(subject, 'A', sharedKey), state: {}, instanceId: randomUUID(), ledger: [] });
      const b = await store.commit({
        event: { ...event(subject, 'B', sharedKey), tenantId: other }, state: {}, instanceId: randomUUID(), ledger: [],
      });
      check(a.applied && b.applied, 'the same idempotency key is scoped per tenant, not global');
      check((await store.loadSnapshot(other, subject))?.revision === 1, 'tenant B built its own independent snapshot');
    }

    // ---- L. Restore drill --------------------------------------------------------------------------
    section('L. Restore drill: flush to segments, hard restart, compare every runtime row');
    {
      const subjects = [];
      const conductor = createAelioConductor(conductorConfig({
        llm: scriptedLlm({ plan: () => ({ mode: 'reply', text: 'durable' }), synthesis: () => '' }),
        sdk: stubSdk(), suspensionStore, effectJournal: store,
      }));
      for (let index = 0; index < 8; index += 1) {
        const subject = `restore-${randomUUID()}`;
        subjects.push(subject);
        await runConductorEvent(store, conductor, event(subject, `message ${index}`, `k-${randomUUID()}`));
      }
      // Force the memtable into an immutable segment so recovery must read segments AND the WAL.
      await client.flush();
      const before = await hashRuntimeTables(client, tables);

      await db.hardRestart();
      const restored = new SunjetClient({ baseUrl: db.url });
      const after = await hashRuntimeTables(restored, tables);

      check(before.total > 0, `there was runtime data to lose (${before.total} rows)`);
      const mismatched = Object.keys(before.byTable).filter((table) => before.byTable[table] !== after.byTable[table]);
      check(mismatched.length === 0, `every runtime table hashes identically after restore (${mismatched.join(', ') || 'none differ'})`);
      check(after.total === before.total, `row count is unchanged (${before.total} → ${after.total})`);

      const restoredStore = new AelioRuntimeStore(restored, tables);
      const sampled = await restoredStore.loadSnapshot(TENANT, subjects[0]);
      check(sampled?.state.history?.length === 2, 'a sampled subject still has its full turn after restore');
    }

    // ---- H. Aelio DB hard restart --------------------------------------------------------------
    section('H. Aelio DB hard restart (SIGKILL, WAL recovery only)');
    {
      const subject = `subject-${randomUUID()}`;
      const key = `restart-${randomUUID()}`;
      const effectId = randomUUID();
      const conductor = createAelioConductor(conductorConfig({
        llm: scriptedLlm({ plan: () => ({ mode: 'reply', text: 'before restart' }), synthesis: () => '' }),
        sdk: stubSdk(), suspensionStore,
      }));
      const before = await runConductorEvent(store, conductor, event(subject, 'remember this', key));
      check(before.status === 'committed', 'a turn committed before the restart');
      // A separate subject: `commit` with an explicit state replaces it, and this check is about
      // the first subject's history surviving the restart.
      const outboxSubject = `subject-${randomUUID()}`;
      await store.commit({
        event: event(outboxSubject, 'queue this', `k-${randomUUID()}`),
        state: {}, instanceId: randomUUID(), ledger: [],
        effects: [{ effectId, idempotencyKey: `reply:${effectId}`, kind: 'reply.send', payload: { channel: 'web', to: outboxSubject, text: 'queued' } }],
      });

      await db.hardRestart();

      const recovered = new AelioRuntimeStore(new SunjetClient({ baseUrl: db.url }), tables);
      const snapshot = await recovered.loadSnapshot(TENANT, subject);
      check(snapshot !== null, 'the subject snapshot survived a SIGKILL restart');
      check(snapshot?.state.history?.some((entry) => entry.content === 'remember this'), 'the committed turn survived intact');

      const recoveredConductor = createAelioConductor(conductorConfig({
        llm: scriptedLlm({ plan: () => ({ mode: 'reply', text: 'after restart' }), synthesis: () => '' }),
        sdk: stubSdk(), suspensionStore: new AelioSuspensionStore(new SunjetClient({ baseUrl: db.url }), tables, TENANT),
        effectJournal: recovered,
      }));
      const replayed = await runConductorEvent(recovered, recoveredConductor, event(subject, 'remember this', key));
      check(replayed.status === 'duplicate', 'idempotency survived the restart: the same key is still a duplicate');

      const queued = await recovered.listPendingEffects(64, Date.now() + 60_000);
      check(queued.some((effect) => effect.effectId === effectId), 'the queued outbox effect survived the restart');
    }
  } finally {
    await db.stop();
  }

  console.log(`\n${passed} checks passed, ${failures.length} failed.`);
  if (failures.length > 0) {
    for (const failure of failures) console.error(` - ${failure}`);
    process.exit(1);
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
