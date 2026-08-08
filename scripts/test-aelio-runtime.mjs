#!/usr/bin/env node
/**
 * End-to-end Aelio Runtime test against a real Aelio DB (ll-server).
 *
 * It boots its own ll-server on a temporary data directory, then exercises the runtime the way
 * production does — no mocks below the storage client:
 *
 *   1. direct reply turn commits event + snapshot + ledger atomically
 *   2. duplicate idempotency key is rejected without a second commit
 *   3. read-tool turn plans, binds, executes, and synthesizes
 *   4. write-tool turn parks a plan in Aelio DB and survives a full process-state reset
 *   5. resumed plan executes the write exactly once
 *   6. denial abandons the parked plan without executing
 *   7. lifecycle transition on tool success lands in the durable snapshot
 *   8. artifact runs step-wise with a durable cursor
 *   9. artifact effect journal makes a replayed tool node non-repeating
 *  10. outbox lease/dispatch/complete and expired-lease requeue
 *  11. scheduler lease → complete, and expired lease → requeue
 *  12. memory write + recall round-trip
 *
 * Usage: node scripts/test-aelio-runtime.mjs
 *        AELIO_DB_URL=http://127.0.0.1:18080 node scripts/test-aelio-runtime.mjs   (use a running server)
 */

import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync, existsSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import {
  AelioMemoryStore,
  AelioRuntimeStore,
  AelioSuspensionStore,
  bootstrapSunjetTables,
  createAelioConductor,
  dispatchPendingEffects,
  runConductorEvent,
  startArtifactCursor,
  stepRuntimeArtifact,
} from '@aelio/core';
import { SunjetClient } from '@aelio/sunjet-client';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const TENANT = 'aelio-runtime-test';
const EMBED_DIM = 1536;

let passed = 0;
const failures = [];

function check(condition, message) {
  if (condition) {
    passed += 1;
    console.log(`  OK  ${message}`);
  } else {
    failures.push(message);
    console.error(`  FAIL ${message}`);
  }
}

function section(title) {
  console.log(`\n— ${title}`);
}

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

// ---------------------------------------------------------------- Aelio DB lifecycle

async function waitForHealth(url, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`${url}/v1/health`);
      if (response.ok) return true;
    } catch {
      // Not listening yet.
    }
    await sleep(200);
  }
  return false;
}

async function startAelioDb() {
  const external = process.env.AELIO_DB_URL;
  if (external) {
    if (!(await waitForHealth(external, 5_000))) {
      throw new Error(`AELIO_DB_URL=${external} is not healthy`);
    }
    return { url: external, stop: async () => {} };
  }

  // Pick the most recently built binary: an old release build predating the transaction API
  // would fail this suite for the wrong reason.
  const binary = ['target/release/ll-server', 'target/debug/ll-server']
    .map((relative) => join(ROOT, 'Sunjet/Astrolobe', relative))
    .filter((candidate) => existsSync(candidate))
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
  if (!binary) {
    throw new Error(
      'No ll-server binary found. Build it first: cargo build -p ll-server (in Sunjet/Astrolobe), ' +
        'or point AELIO_DB_URL at a running Aelio DB.',
    );
  }

  const port = 19000 + Math.floor(Math.random() * 900);
  const dataDir = mkdtempSync(join(tmpdir(), 'aelio-runtime-test-'));
  const child = spawn(binary, [], {
    env: { ...process.env, LL_DATA_DIR: dataDir, LL_BIND: `127.0.0.1:${port}`, LL_PORT: String(port) },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let log = '';
  child.stdout.on('data', (chunk) => { log += chunk.toString(); });
  child.stderr.on('data', (chunk) => { log += chunk.toString(); });

  const url = `http://127.0.0.1:${port}`;
  if (!(await waitForHealth(url))) {
    child.kill('SIGKILL');
    throw new Error(`Aelio DB did not become healthy on ${url}.\n${log}`);
  }
  return {
    url,
    stop: async () => {
      child.kill('SIGTERM');
      await sleep(300);
      child.kill('SIGKILL');
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
  };
}

// ---------------------------------------------------------------- Deterministic tenant doubles

/** A scripted planner/synthesizer. It never invents behaviour, so every assertion below is exact. */
function createScriptedLlm(script) {
  const calls = [];
  return {
    calls,
    complete: async (options) => {
      const purpose = options.telemetry?.purpose ?? 'chat_completion';
      calls.push(purpose);
      if (purpose === 'plan' || purpose === 'replan') {
        const userMessage = [...options.messages].reverse().find((m) => m.role === 'user')?.content ?? '';
        const turn = script.plan(userMessage);
        return {
          text: '',
          toolCalls: [{ id: randomUUID(), name: 'emit_turn', args: turn }],
          stopReason: 'tool_use',
          usage: { inputTokens: 10, outputTokens: 10 },
        };
      }
      if (purpose === 'synthesis') {
        return { text: script.synthesis(), toolCalls: [], stopReason: 'stop', usage: { inputTokens: 5, outputTokens: 5 } };
      }
      return { text: script.fallback ?? 'ok', toolCalls: [], stopReason: 'stop', usage: { inputTokens: 1, outputTokens: 1 } };
    },
  };
}

function createStubSdk(options = {}) {
  const invocations = [];
  return {
    invocations,
    getFunctions: () => options.functions ?? [],
    getStates: () => options.states ?? [],
    getPolicies: () => [],
    getFlows: () => [],
    getPersona: () => 'You are the Aelio runtime test assistant.',
    getProductBrief: () => 'Test product.',
    invoke: async (name, args) => {
      invocations.push({ name, args });
      const handler = options.handlers?.[name];
      const data = handler ? handler(args) : { ok: true };
      return { ok: true, data, durationMs: 1 };
    },
  };
}

const READ_TOOL = {
  name: 'get_order_status',
  description: 'Look up the status of an order.',
  intent: 'read order status',
  safety: 'read',
  parameters: [{ name: 'order_id', type: 'string', required: true, description: 'The order id.' }],
};

const WRITE_TOOL = {
  name: 'cancel_order',
  description: 'Cancel an order.',
  intent: 'cancel an order',
  safety: 'write',
  parameters: [{ name: 'order_id', type: 'string', required: true, description: 'The order id.' }],
};

function harnessConfig() {
  return {
    enabled: true,
    budgets: {
      maxInstructions: 8, maxReplans: 1, maxRecoilsPerIntent: 2,
      maxToolCalls: 8, wallClockMs: 30_000, maxTurnTokens: 100_000,
    },
    binding: { scoreMin: 0.2, ambiguityGap: 0.05, cacheTtlMinutes: 10 },
  };
}

function baseConductor(overrides) {
  return {
    model: 'test-model',
    maxTokens: 512,
    historyWindow: 10,
    safety: { defaultMode: 'confirm_writes', requireConfirmationFor: ['write', 'destructive'], overrides: [] },
    harness: harnessConfig(),
    memoryEnabled: false,
    ...overrides,
  };
}

function runtimeEvent(subjectId, message, idempotencyKey, extra = {}) {
  return {
    apiVersion: 'aelio.runtime.event/v1',
    eventId: randomUUID(),
    idempotencyKey,
    tenantId: TENANT,
    subjectId,
    kind: 'user_message',
    payload: { message, channel: 'web', ...extra },
    receivedAt: Date.now(),
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
  const memory = new AelioMemoryStore(client, tables, EMBED_DIM);

  try {
    // ---- 1. Direct reply commits atomically -------------------------------------------------
    section('Direct reply turn');
    {
      const subject = `subject-${randomUUID()}`;
      const llm = createScriptedLlm({ plan: () => ({ mode: 'reply', text: 'Hello from Aelio.' }), synthesis: () => '' });
      const sdk = createStubSdk();
      const conductor = createAelioConductor(baseConductor({ llm, sdk, suspensionStore }));

      const outcome = await runConductorEvent(store, conductor, runtimeEvent(subject, 'hi', `k-${randomUUID()}`));
      check(outcome.status === 'committed', 'turn committed');
      check(outcome.status === 'committed' && outcome.decision.text === 'Hello from Aelio.', 'reply text returned');
      check(outcome.status === 'committed' && outcome.commitLsn > 0, 'commit LSN assigned');

      const snapshot = await store.loadSnapshot(TENANT, subject);
      check(snapshot?.revision === 1, 'snapshot revision is 1');
      check(snapshot?.state.history?.length === 2, 'history holds the user turn and the reply');
      check(snapshot?.state.turns === 1, 'turn counter advanced');

      const second = await runConductorEvent(store, conductor, runtimeEvent(subject, 'again', `k-${randomUUID()}`));
      check(second.status === 'committed', 'second turn committed');
      const after = await store.loadSnapshot(TENANT, subject);
      check(after?.revision === 2, 'snapshot revision advanced to 2');
      check(after?.state.history?.length === 4, 'history accumulated across turns');
    }

    // ---- 2. Idempotent ingress ---------------------------------------------------------------
    section('Duplicate event rejection');
    {
      const subject = `subject-${randomUUID()}`;
      const key = `dupe-${randomUUID()}`;
      const llm = createScriptedLlm({ plan: () => ({ mode: 'reply', text: 'once' }), synthesis: () => '' });
      const conductor = createAelioConductor(baseConductor({ llm, sdk: createStubSdk(), suspensionStore }));

      const first = await runConductorEvent(store, conductor, runtimeEvent(subject, 'hello', key));
      const repeat = await runConductorEvent(store, conductor, runtimeEvent(subject, 'hello', key));
      check(first.status === 'committed', 'first delivery committed');
      check(repeat.status === 'duplicate', 'redelivery rejected as duplicate');
      const snapshot = await store.loadSnapshot(TENANT, subject);
      check(snapshot?.revision === 1, 'duplicate did not advance the snapshot');
    }

    // ---- 3. Read tool executes ----------------------------------------------------------------
    section('Read-tool turn');
    {
      const subject = `subject-${randomUUID()}`;
      const llm = createScriptedLlm({
        plan: () => ({
          mode: 'plan', goal: 'Look up order 42',
          instructions: [{ id: 'a', capability: 'read order status', tool: 'get_order_status', args_hint: { order_id: '42' } }],
        }),
        synthesis: () => 'Order 42 is shipped.',
      });
      const sdk = createStubSdk({
        functions: [READ_TOOL],
        handlers: { get_order_status: () => ({ status: 'shipped' }) },
      });
      const conductor = createAelioConductor(baseConductor({ llm, sdk, suspensionStore }));

      const outcome = await runConductorEvent(store, conductor, runtimeEvent(subject, 'where is order 42', `k-${randomUUID()}`));
      check(outcome.status === 'committed', 'read turn committed');
      check(sdk.invocations.length === 1 && sdk.invocations[0].name === 'get_order_status', 'read tool invoked exactly once');
      check(sdk.invocations[0].args.order_id === '42', 'tool received the concrete argument');
      check(outcome.status === 'committed' && outcome.decision.text === 'Order 42 is shipped.', 'synthesized reply returned');
    }

    // ---- 4/5/6. Write confirmation park, restart, resume ---------------------------------------
    section('Write confirmation park + restart + resume');
    {
      const subject = `subject-${randomUUID()}`;
      const sessionId = `${TENANT}:${subject}`;
      const plan = () => ({
        mode: 'plan', goal: 'Cancel order 42',
        instructions: [{ id: 'a', capability: 'cancel an order', tool: 'cancel_order', args_hint: { order_id: '42' } }],
      });
      const sdk = createStubSdk({ functions: [WRITE_TOOL], handlers: { cancel_order: () => ({ cancelled: true }) } });
      const llm = createScriptedLlm({ plan, synthesis: () => 'Order 42 is cancelled.' });
      const conductor = createAelioConductor(baseConductor({ llm, sdk, suspensionStore }));

      const parkTurn = await runConductorEvent(store, conductor, runtimeEvent(subject, 'cancel order 42', `k-${randomUUID()}`));
      check(parkTurn.status === 'committed', 'write turn committed a decision');
      check(sdk.invocations.length === 0, 'no write executed before confirmation');

      // Simulate a full restart: brand-new client, store, and suspension store over the same DB.
      const restartClient = new SunjetClient({ baseUrl: db.url });
      const restartStore = new AelioRuntimeStore(restartClient, tables);
      const restartSuspension = new AelioSuspensionStore(restartClient, tables, TENANT);
      const parked = await restartSuspension.get(sessionId);
      check(parked !== null, 'parked plan survives a restart');
      check(parked?.reason === 'awaiting_confirmation', 'parked plan is awaiting confirmation');

      const resumeConductor = createAelioConductor(baseConductor({ llm, sdk, suspensionStore: restartSuspension }));
      const resumed = await runConductorEvent(restartStore, resumeConductor, runtimeEvent(subject, 'yes', `k-${randomUUID()}`));
      check(resumed.status === 'committed', 'resume turn committed');
      check(sdk.invocations.length === 1, 'the write executed exactly once after confirmation');
      check(sdk.invocations[0]?.name === 'cancel_order', 'the confirmed write is the parked one');
      check((await restartSuspension.get(sessionId)) === null, 'parked plan cleared after resume');

      const again = await runConductorEvent(restartStore, resumeConductor, runtimeEvent(subject, 'yes', `k-${randomUUID()}`));
      check(again.status === 'committed', 'a repeated yes is still a valid turn');
      check(sdk.invocations.length === 1, 'a repeated yes does not re-run the write');
    }

    section('Write denial abandons the plan');
    {
      const subject = `subject-${randomUUID()}`;
      const sdk = createStubSdk({ functions: [WRITE_TOOL], handlers: { cancel_order: () => ({ cancelled: true }) } });
      const llm = createScriptedLlm({
        plan: () => ({
          mode: 'plan', goal: 'Cancel order 7',
          instructions: [{ id: 'a', capability: 'cancel an order', tool: 'cancel_order', args_hint: { order_id: '7' } }],
        }),
        synthesis: () => 'done',
      });
      const conductor = createAelioConductor(baseConductor({ llm, sdk, suspensionStore }));
      const ask = await runConductorEvent(store, conductor, runtimeEvent(subject, 'cancel order 7', `k-${randomUUID()}`));
      check(ask.status === 'committed' && ask.decision.text.includes('cancel_order'), 'the confirmation names the exact write');

      // An ambiguous reply is neither consent nor a cancel: the plan must stay parked and re-ask.
      const ambiguous = await runConductorEvent(store, conductor, runtimeEvent(subject, 'hmm not sure', `k-${randomUUID()}`));
      check(sdk.invocations.length === 0, 'an ambiguous reply did not execute the write');
      check(
        ambiguous.status === 'committed' && ambiguous.decision.text.includes('cancel_order'),
        'an ambiguous reply re-asks the same confirmation',
      );
      check((await suspensionStore.get(`${TENANT}:${subject}`)) !== null, 'an ambiguous reply keeps the plan parked');

      await runConductorEvent(store, conductor, runtimeEvent(subject, 'no', `k-${randomUUID()}`));
      check(sdk.invocations.length === 0, 'denial prevented the write entirely');
      check((await suspensionStore.get(`${TENANT}:${subject}`)) === null, 'denial cleared the parked plan');
    }

    // ---- 7. Lifecycle transition into the durable snapshot -------------------------------------
    section('Lifecycle transition');
    {
      const subject = `subject-${randomUUID()}`;
      const states = [{
        id: 'cart_active',
        description: 'Cart is open.',
        allowed_tools: ['get_order_status'],
        transitions: [{ on_tool_success: 'get_order_status', to: 'order_tracked' }],
      }];
      const sdk = createStubSdk({ functions: [READ_TOOL], states, handlers: { get_order_status: () => ({ status: 'shipped' }) } });
      const llm = createScriptedLlm({
        plan: () => ({
          mode: 'plan', goal: 'Track it',
          instructions: [{ id: 'a', capability: 'read order status', tool: 'get_order_status', args_hint: { order_id: '9' } }],
        }),
        synthesis: () => 'Tracked.',
      });
      const conductor = createAelioConductor(baseConductor({ llm, sdk, suspensionStore }));

      // Seed the subject into the state whose transition we want to observe.
      await store.commit({
        event: runtimeEvent(subject, 'seed', `k-${randomUUID()}`),
        state: { version: 1, history: [], lifecycleState: 'cart_active', lifecycleReason: null, profile: {}, turns: 0, lastChannel: 'web', lastEventAt: Date.now() },
        instanceId: randomUUID(),
        ledger: [],
      });

      await runConductorEvent(store, conductor, runtimeEvent(subject, 'track order 9', `k-${randomUUID()}`));
      const snapshot = await store.loadSnapshot(TENANT, subject);
      check(snapshot?.state.lifecycleState === 'order_tracked', 'tool success advanced the durable lifecycle state');
    }

    // ---- 8/9. Artifact stepping + effect journal -----------------------------------------------
    section('Artifact execution');
    {
      const artifact = {
        apiVersion: 'aelio.runtime.artifact/v1',
        artifactId: 'test.sum_then_double',
        version: '1.0.0',
        digest: 'test-digest',
        status: 'approved',
        entryNodeId: 'sum',
        nodes: [
          { id: 'sum', type: 'compute', operation: 'sum', input: { $ref: 'input' }, next: 'double' },
          { id: 'double', type: 'compute', operation: 'multiply', input: [{ $ref: 'sum' }, 2], next: 'done' },
          { id: 'done', type: 'end', output: { $ref: 'double' } },
        ],
        createdAt: Date.now(),
      };
      check(await store.installArtifact(artifact), 'artifact installed');
      check(!(await store.installArtifact(artifact)), 'reinstalling the same artifact version is rejected');
      check((await store.loadArtifact('test.sum_then_double', '1.0.0')) !== null, 'approved artifact loads back');

      const failClosed = {
        memory: { get: async () => null, set: async () => {}, delete: async () => false },
        runPrompt: async () => { throw new Error('unexpected prompt'); },
        runTool: async () => { throw new Error('unexpected tool'); },
        spawn: async () => { throw new Error('unexpected spawn'); },
      };
      let cursor = startArtifactCursor(artifact, [1, 2, 3]);
      const seen = [];
      for (;;) {
        const step = await stepRuntimeArtifact(artifact, cursor, failClosed);
        if (step.status === 'completed') { seen.push(step.output); break; }
        seen.push(step.cursor.nodeId);
        cursor = step.cursor;
      }
      check(seen[0] === 'double' && seen[1] === 'done', 'artifact advanced one node per step');
      check(seen[2] === 12, 'artifact produced (1+2+3)*2 = 12');
    }

    section('Artifact effect journal');
    {
      const instanceId = `instance-${randomUUID()}`;
      const subject = `subject-${randomUUID()}`;
      const key = `${instanceId}:node-a:digest`;
      const begin = { key, tenantId: TENANT, subjectId: subject, instanceId, eventId: randomUUID(), kind: 'artifact.tool' };

      const first = await store.beginEffect({ ...begin, request: { capability: 'x' } });
      check(first.state === 'new', 'first attempt may perform the effect');

      const interrupted = await store.beginEffect({ ...begin, request: { capability: 'x' } });
      check(interrupted.state === 'unknown', 'a replay before the result is recorded reports unknown, not new');

      await store.recordEffectResult({ ...begin, result: { done: true } });
      const replayed = await store.beginEffect({ ...begin, request: { capability: 'x' } });
      check(replayed.state === 'completed', 'a replay after the result reuses it');
      check(replayed.state === 'completed' && replayed.result.done === true, 'the recorded result is returned verbatim');
    }

    // ---- 10. Outbox ----------------------------------------------------------------------------
    section('Outbox lease, dispatch, retry');
    {
      const subject = `subject-${randomUUID()}`;
      const effectId = randomUUID();
      await store.commit({
        event: runtimeEvent(subject, 'deliver me', `k-${randomUUID()}`),
        state: {},
        instanceId: randomUUID(),
        ledger: [],
        effects: [{ effectId, idempotencyKey: `reply:${effectId}`, kind: 'reply.send', payload: { channel: 'whatsapp', to: '+100', text: 'hi' } }],
      });

      const pending = await store.listPendingEffects();
      check(pending.some((effect) => effect.effectId === effectId), 'reply effect is pending in the outbox');

      const delivered = [];
      const result = await dispatchPendingEffects(store, async (effect) => { delivered.push(effect.effectId); });
      check(delivered.includes(effectId), 'dispatcher delivered the effect');
      check(result.delivered >= 1, 'dispatcher reported a delivery');
      const stillPending = await store.listPendingEffects();
      check(!stillPending.some((effect) => effect.effectId === effectId), 'delivered effect left the pending set');

      // A failing handler must schedule a durable retry, not lose the effect.
      const retryId = randomUUID();
      await store.commit({
        event: runtimeEvent(subject, 'retry me', `k-${randomUUID()}`),
        state: {},
        instanceId: randomUUID(),
        ledger: [],
        effects: [{ effectId: retryId, idempotencyKey: `reply:${retryId}`, kind: 'reply.send', payload: { channel: 'whatsapp', to: '+100', text: 'retry' } }],
      });
      await dispatchPendingEffects(store, async () => { throw new Error('provider down'); });
      const [retryRow] = (await store.listPendingEffects(64, Date.now() + 3_600_000)).filter((e) => e.effectId === retryId);
      check(retryRow !== undefined, 'a failed effect stays in the outbox for retry');
      check(retryRow?.attempts === 1, 'the failed attempt was counted');
      check(retryRow?.availableAt > Date.now(), 'retry is backed off into the future');
      check((retryRow?.lastError ?? '').includes('provider down'), 'the failure reason was captured');
    }

    // ---- 11. Scheduler --------------------------------------------------------------------------
    section('Scheduled event leases');
    {
      const scheduleId = `sched-${randomUUID()}`;
      const subject = `subject-${randomUUID()}`;
      check(await store.scheduleEvent({ scheduleId, tenantId: TENANT, subjectId: subject, dueAt: Date.now() - 1, payload: { message: 'ping' } }), 'event scheduled');
      check(!(await store.scheduleEvent({ scheduleId, tenantId: TENANT, subjectId: subject, dueAt: Date.now(), payload: {} })), 'duplicate schedule id rejected');

      const due = await store.listDueScheduledEvents(Date.now());
      const target = due.find((event) => event.scheduleId === scheduleId);
      check(target !== undefined, 'scheduled event is due');

      const claimed = await store.claimScheduledEvent(target, 'worker-a');
      check(claimed !== null, 'worker A leased the event');
      check((await store.claimScheduledEvent(target, 'worker-b')) === null, 'worker B could not lease the same event');
      check(await store.completeScheduledEvent(claimed), 'lease holder completed the event');

      // Expired lease must return to the ready pool for another worker.
      const staleId = `sched-${randomUUID()}`;
      await store.scheduleEvent({ scheduleId: staleId, tenantId: TENANT, subjectId: subject, dueAt: Date.now() - 1, payload: {} });
      const stale = (await store.listDueScheduledEvents(Date.now())).find((event) => event.scheduleId === staleId);
      await store.claimScheduledEvent(stale, 'worker-crashed', -1_000);
      check((await store.requeueExpiredScheduledEvents(Date.now())) >= 1, 'expired lease was requeued');
      check(
        (await store.listDueScheduledEvents(Date.now())).some((event) => event.scheduleId === staleId),
        'requeued event is due again',
      );
    }

    // ---- 12. Memory ------------------------------------------------------------------------------
    section('Subject memory');
    {
      const subject = `subject-${randomUUID()}`;
      const wrote = await memory.remember({ subjectId: subject, content: 'User prefers email over phone', category: 'preference' });
      check(wrote !== null, 'memory written');
      check((await memory.remember({ subjectId: subject, content: 'User prefers email over phone' })) === null, 'duplicate memory deduplicated');
      const recalled = await memory.recall(subject, 'how should we contact them', 5);
      check(recalled.length === 1, 'memory recalled for the subject');
      check(recalled[0]?.content === 'User prefers email over phone', 'recalled content matches');
      check((await memory.recall(`other-${randomUUID()}`, 'anything', 5)).length === 0, 'memory is scoped to its subject');
    }

    // ---- 13. SDK state push reaches the runtime ---------------------------------------------------
    section('SDK state push into the durable snapshot');
    {
      const subject = `subject-${randomUUID()}`;
      const llm = createScriptedLlm({ plan: () => ({ mode: 'reply', text: 'ack' }), synthesis: () => '' });
      const conductor = createAelioConductor(baseConductor({ llm, sdk: createStubSdk(), suspensionStore }));
      await runConductorEvent(store, conductor, runtimeEvent(subject, 'hello', `k-${randomUUID()}`));

      await store.patchSubjectState(TENANT, subject, { lifecycleState: 'subscription_active', lifecycleReason: 'sdk push' });
      const patched = await store.loadSnapshot(TENANT, subject);
      check(patched?.state.lifecycleState === 'subscription_active', 'SDK push landed in the durable snapshot');
      check(patched?.state.history?.length === 2, 'the push merged into state instead of replacing it');

      await store.patchSubjectState(TENANT, subject, { flowProgress: { onboarding: { currentStepIndex: 2, completedSteps: ['a'] } } });
      const merged = await store.loadSnapshot(TENANT, subject);
      check(merged?.state.lifecycleState === 'subscription_active', 'a second push preserved the first');
      check(merged?.state.flowProgress?.onboarding?.currentStepIndex === 2, 'flow progress landed');

      // A brand-new subject must be creatable by a push alone.
      const fresh = `subject-${randomUUID()}`;
      await store.patchSubjectState(TENANT, fresh, { lifecycleState: 'onboarding' });
      check((await store.loadSnapshot(TENANT, fresh))?.state.lifecycleState === 'onboarding', 'push creates a snapshot for an unseen subject');
    }

    // ---- 14. Continuations -----------------------------------------------------------------------
    section('Continuation single-consumption');
    {
      const token = `wake-${randomUUID()}`;
      const subject = `subject-${randomUUID()}`;
      check(
        await store.createContinuation({
          token, tenantId: TENANT, subjectId: subject, instanceId: randomUUID(),
          prompt: 'Which size?', payload: {}, expiresAt: Date.now() + 60_000,
        }),
        'continuation created',
      );
      check((await store.consumeContinuation(TENANT, token)) !== null, 'continuation consumed once');
      check((await store.consumeContinuation(TENANT, token)) === null, 'continuation cannot be consumed twice');
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
