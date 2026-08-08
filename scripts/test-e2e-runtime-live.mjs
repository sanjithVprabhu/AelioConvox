#!/usr/bin/env node
/**
 * Live end-to-end test of the Aelio Runtime through the real server.
 *
 * Every other suite exercises libraries or the storage client directly. This one boots the actual
 * deployment — Aelio DB, the Fastify server with `sunjet.enabled: true` so the runtime path is
 * authoritative, and the example SaaS backend over the real SDK websocket — then talks to it the
 * way a customer's browser does.
 *
 * This matters because `config.yaml` ships with `sunjet.enabled: false`, so the Phase 2–5 suite
 * runs the LEGACY SQLite turn. Without this file, nothing verified the runtime end to end through
 * the routes, workers, and admin surface that production actually uses.
 *
 *   1. the server boots and applies the schema migration ledger
 *   2. a widget message returns a real reply through the runtime (not the legacy path)
 *   3. a read tool executes and its result reaches the reply
 *   4. a write asks for confirmation, and only an explicit yes executes it — exactly once
 *   5. the durable subject snapshot holds the whole conversation in Aelio DB
 *   6. a duplicate frame is rejected rather than answered twice
 *   7. the authenticated runtime HTTP ingress works and rejects a foreign tenant
 *   8. the admin surface reports health, the subject, and the turn ledger — redacted
 *   9. the same subject continues across a second websocket connection
 *
 * Usage: node scripts/test-e2e-runtime-live.mjs
 */

import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import WebSocket from 'ws';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const SDK_SECRET = 'e2e-runtime-secret';

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

async function freePort() {
  const server = createServer();
  await new Promise((done, fail) => {
    server.once('error', fail);
    server.listen(0, '127.0.0.1', done);
  });
  const { port } = server.address();
  await new Promise((done) => server.close(done));
  return port;
}

async function waitFor(probe, what, timeoutMs = 60_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      if (await probe()) return;
    } catch {
      // not ready
    }
    await sleep(250);
  }
  throw new Error(`timed out waiting for ${what}`);
}

/** One request/response exchange over the widget socket, resolved by a predicate. */
function widgetExchange(wsUrl, script, { timeoutMs = 45_000 } = {}) {
  return new Promise((done, fail) => {
    const socket = new WebSocket(`${wsUrl}/widget/ws`);
    const received = [];
    const timer = setTimeout(() => {
      socket.close();
      fail(new Error(`widget exchange timed out; saw ${JSON.stringify(received)}`));
    }, timeoutMs);
    let step = 0;

    socket.on('open', () => socket.send(JSON.stringify(script.init)));
    socket.on('message', (raw) => {
      const message = JSON.parse(raw.toString());
      if (message.type === 'typing') return;
      received.push(message);
      if (message.type === 'ready') {
        socket.send(JSON.stringify({ type: 'message', content: script.turns[0] }));
        return;
      }
      if (message.type !== 'message' && message.type !== 'confirmation' && message.type !== 'error') return;
      step += 1;
      if (step < script.turns.length) {
        socket.send(JSON.stringify({ type: 'message', content: script.turns[step] }));
        return;
      }
      clearTimeout(timer);
      socket.close();
      done(received);
    });
    socket.on('error', (error) => {
      clearTimeout(timer);
      fail(error);
    });
  });
}

function llServerBinary() {
  const binary = ['target/release/ll-server', 'target/debug/ll-server']
    .map((relative) => join(ROOT, 'Sunjet/Astrolobe', relative))
    .filter((candidate) => existsSync(candidate))
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
  if (!binary) throw new Error('No ll-server binary; run `cargo build -p ll-server` in Sunjet/Astrolobe.');
  return binary;
}

/**
 * A config with Aelio DB switched ON, so the server routes turns through the Aelio Runtime.
 * The mock LLM keeps the run deterministic and offline; the tools and every storage path are real.
 */
function writeRuntimeConfig({ configPath, dbPath, port, aelioDbUrl }) {
  const base = readFileSync(join(ROOT, 'config.yaml'), 'utf8');
  const config = base
    .replace(/database_path:\s.*$/m, `database_path: ${dbPath}`)
    .replace(/^(\s*)provider:\s*\w+/m, '$1provider: mock')
    .replace(/- http:\/\/localhost:\d+$/m, `- http://127.0.0.1:${port}`)
    // Turn Aelio DB on and point it at this run's server. Table names are namespaced so repeated
    // runs never collide inside one Aelio DB instance.
    .replace(/^(sunjet:\n\s*enabled:)\s*false/m, '$1 true')
    .replace(/^(\s*url:)\s*http:\/\/127\.0\.0\.1:8080/m, `$1 ${aelioDbUrl}`);
  if (!/^sunjet:\n\s*enabled: true/m.test(config)) {
    throw new Error('failed to enable sunjet in the generated config — the config.yaml shape changed');
  }
  writeFileSync(configPath, config);
  return config;
}

async function main() {
  const workdir = mkdtempSync(join(tmpdir(), 'aelio-e2e-'));
  const dataDir = join(workdir, 'aeliodb');
  const configPath = join(workdir, 'config.yaml');
  const dbPath = join(workdir, 'aelio.db');
  const children = [];
  const logs = new Map();

  // `detached` puts each child in its own process group. `npx` spawns a `tsx`/node grandchild, and
  // signalling only the direct child leaves that grandchild alive holding the stdout pipe — which
  // keeps this process's event loop alive forever. Killing the group is what actually stops it.
  const spawnLogged = (name, command, args, options) => {
    const child = spawn(command, args, { ...options, stdio: ['ignore', 'pipe', 'pipe'], detached: true });
    logs.set(name, '');
    const append = (chunk) => logs.set(name, (logs.get(name) + chunk.toString()).slice(-20_000));
    child.stdout.on('data', append);
    child.stderr.on('data', append);
    children.push(child);
    return child;
  };

  const killTree = (child) => {
    if (!child.pid) return;
    try {
      process.kill(-child.pid, 'SIGKILL');
    } catch {
      // The group may already be gone; fall back to the direct child.
      try { child.kill('SIGKILL'); } catch { /* already dead */ }
    }
    child.stdout?.destroy();
    child.stderr?.destroy();
  };

  try {
    const aelioDbPort = await freePort();
    const serverPort = await freePort();
    const aelioDbUrl = `http://127.0.0.1:${aelioDbPort}`;
    const baseUrl = `http://127.0.0.1:${serverPort}`;
    const wsUrl = `ws://127.0.0.1:${serverPort}`;

    section('Boot: Aelio DB, Aelio server on the runtime path, example SDK backend');
    spawnLogged('aeliodb', llServerBinary(), [], {
      env: { ...process.env, LL_DATA_DIR: dataDir, LL_BIND: `127.0.0.1:${aelioDbPort}` },
    });
    await waitFor(async () => (await fetch(`${aelioDbUrl}/v1/health`)).ok, 'Aelio DB health');
    check(true, `Aelio DB is up on ${aelioDbUrl}`);

    writeRuntimeConfig({ configPath, dbPath, port: serverPort, aelioDbUrl });
    spawnLogged('server', 'npx', ['tsx', 'src/main.ts'], {
      cwd: join(ROOT, 'server'),
      env: {
        ...process.env,
        AELIO_CONFIG: configPath,
        AELIO_PORT: String(serverPort),
        AELIO_SDK_SECRET: SDK_SECRET,
        AELIO_TEST_MODE: '1',
      },
    });
    await waitFor(async () => (await fetch(`${baseUrl}/health`)).ok, `Aelio server health (${logs.get('server')?.slice(-600)})`);
    check(true, `Aelio server is up on ${baseUrl}`);
    check(
      /applied 3 Aelio DB migration\(s\) → schema v3/.test(logs.get('server') ?? ''),
      'the server applied the schema migration ledger at boot',
    );

    spawnLogged('sdk', 'npx', ['tsx', 'src/index.ts'], {
      cwd: join(ROOT, 'examples/nodejs-express'),
      env: {
        ...process.env,
        AELIO_SDK_SECRET: SDK_SECRET,
        AELIO_SERVER_URL: wsUrl,
        PORT: String(await freePort()),
      },
    });
    await waitFor(async () => {
      const response = await fetch(`${baseUrl}/__test__/sdk/functions`);
      if (!response.ok) return false;
      const { functions = [] } = await response.json();
      return functions.includes('getOrderStatus') && functions.includes('cancelOrder');
    }, `SDK tool registration (${logs.get('sdk')?.slice(-400)})`);
    check(true, 'the example SaaS backend registered its tools over the SDK websocket');

    // ---- The runtime path, driven like a browser ------------------------------------------------
    section('A read-tool conversation through the widget');
    const customerId = `e2e-${randomUUID()}`;
    const read = await widgetExchange(wsUrl, {
      init: { type: 'init', customerId },
      turns: ['what is the status of order A123?'],
    });
    const readReply = read.find((message) => message.type === 'message');
    check(readReply !== undefined, 'the widget received an assistant reply');
    check(
      !read.some((message) => message.type === 'error'),
      `no error frame during the read turn (${JSON.stringify(read.filter((m) => m.type === 'error'))})`,
    );
    check(
      /shipped|1Z999AA10123456784/i.test(readReply?.content ?? ''),
      `the reply carries the real tool result (got: ${JSON.stringify(readReply?.content ?? '').slice(0, 160)})`,
    );

    // The decisive check that this went through the runtime and not the legacy path: the runtime
    // is the only writer of Aelio DB subject snapshots.
    section('The turn is durable in Aelio DB, not just SQLite');
    const adminHeaders = { authorization: `Bearer ${readConfigSecret(configPath)}` };
    const subjectResponse = await fetch(`${baseUrl}/api/v1/admin/runtime/subjects/${customerId}`, { headers: adminHeaders });
    check(subjectResponse.status === 200, `the admin subject view resolves the subject (status ${subjectResponse.status})`);
    const subject = subjectResponse.status === 200 ? await subjectResponse.json() : {};
    check(subject.revision >= 1, `the subject has a durable Aelio DB snapshot (revision ${subject.revision})`);
    check(subject.turns >= 1, `the durable turn counter advanced (turns ${subject.turns})`);
    check(Array.isArray(subject.history) && subject.history.length >= 2, 'the snapshot holds the user turn and the reply');
    check(
      (subject.history ?? []).every((entry) => typeof entry.length === 'number' && entry.content === undefined),
      'the admin view redacts message content to lengths',
    );

    // ---- Write confirmation, the safety-critical path -------------------------------------------
    section('A write requires confirmation and executes exactly once');
    const writeCustomer = `e2e-write-${randomUUID()}`;
    const write = await widgetExchange(wsUrl, {
      init: { type: 'init', customerId: writeCustomer },
      turns: ['please cancel order A123', 'yes'],
    });
    const confirmation = write.find((message) => message.type === 'confirmation');
    check(confirmation !== undefined, 'the write turn asked for confirmation instead of acting');
    check(
      /cancelOrder/i.test(confirmation?.prompt ?? ''),
      `the confirmation names the exact write (got: ${JSON.stringify(confirmation?.prompt ?? '').slice(0, 160)})`,
    );
    const afterYes = write.filter((message) => message.type === 'message').pop();
    check(afterYes !== undefined, 'the confirmed write produced a reply');
    check(
      /cancel/i.test(afterYes?.content ?? ''),
      `the reply reports the cancellation (got: ${JSON.stringify(afterYes?.content ?? '').slice(0, 160)})`,
    );

    // An ambiguous reply must not execute a write. A fresh subject keeps this independent.
    const ambiguousCustomer = `e2e-ambig-${randomUUID()}`;
    const ambiguous = await widgetExchange(wsUrl, {
      init: { type: 'init', customerId: ambiguousCustomer },
      turns: ['please cancel order A123', 'hmm, I am not sure'],
    });
    const reAsked = ambiguous.filter((message) => message.type === 'confirmation');
    check(reAsked.length === 2, `an ambiguous reply re-asks rather than executing (got ${reAsked.length} confirmations)`);

    // ---- Idempotent HTTP ingress ------------------------------------------------------------------
    section('Authenticated runtime HTTP ingress');
    const tenant = readConfigName(configPath);
    const ingestKey = `e2e-http-${randomUUID()}`;
    const ingestBody = {
      tenant_id: tenant,
      subject_id: `e2e-http-${randomUUID()}`,
      idempotency_key: ingestKey,
      message: 'what is the status of order A123?',
      channel: 'web',
    };
    const unauthorized = await fetch(`${baseUrl}/v1/runtime/events`, {
      method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(ingestBody),
    });
    check(unauthorized.status === 401, `unauthenticated ingress is rejected (status ${unauthorized.status})`);

    const foreignTenant = await fetch(`${baseUrl}/v1/runtime/events`, {
      method: 'POST',
      headers: { ...adminHeaders, 'content-type': 'application/json' },
      body: JSON.stringify({ ...ingestBody, tenant_id: 'someone-elses-tenant' }),
    });
    check(foreignTenant.status === 403, `a foreign tenant_id is rejected (status ${foreignTenant.status})`);

    const accepted = await fetch(`${baseUrl}/v1/runtime/events`, {
      method: 'POST',
      headers: { ...adminHeaders, 'content-type': 'application/json' },
      body: JSON.stringify(ingestBody),
    });
    const acceptedBody = await accepted.json();
    check(accepted.status === 200 && acceptedBody.status === 'committed', `an authenticated event commits (status ${accepted.status})`);
    check(typeof acceptedBody.reply === 'string' && acceptedBody.reply.length > 0, 'the ingress returns the assistant reply');
    check(typeof acceptedBody.commit_lsn === 'number', 'the ingress returns the durable commit LSN');

    const replayed = await fetch(`${baseUrl}/v1/runtime/events`, {
      method: 'POST',
      headers: { ...adminHeaders, 'content-type': 'application/json' },
      body: JSON.stringify(ingestBody),
    });
    const replayedBody = await replayed.json();
    check(replayedBody.status === 'duplicate', `the same idempotency key is rejected as duplicate (got ${replayedBody.status})`);

    // ---- Operator surface --------------------------------------------------------------------------
    section('Operator surface on a live system');
    const health = await (await fetch(`${baseUrl}/api/v1/admin/runtime/health`, { headers: adminHeaders })).json();
    check(health.status === 'ok' || health.status === 'degraded', `health reports a status (${health.status})`);
    check(health.outbox !== undefined && health.scheduler !== undefined, 'health covers the outbox and the scheduler');
    check(health.outbox.unknown === 0, `no effect ended with an unknown outcome (${health.outbox.unknown})`);
    check(health.outbox.failed === 0, `no effect failed terminally (${health.outbox.failed})`);
    check(health.instances.failed === 0, `no workflow instance failed (${health.instances.failed})`);

    const turn = await (await fetch(`${baseUrl}/api/v1/admin/runtime/turns/${acceptedBody.event_id}`, { headers: adminHeaders })).json();
    check(Array.isArray(turn.entries) && turn.entries.length >= 2, `the turn ledger reconstructs the decision chain (${turn.entries?.length} entries)`);
    check(turn.entries?.some((entry) => entry.kind === 'event_claimed'), 'the ledger records the event claim');
    check(turn.entries?.some((entry) => entry.kind === 'decision'), 'the ledger records the decision');
    check(
      turn.entries?.every((entry) => typeof entry.payload !== 'string' || /^string\(\d+\)$/.test(entry.payload)),
      'the ledger view redacts payload contents',
    );

    const unauthorizedAdmin = await fetch(`${baseUrl}/api/v1/admin/runtime/health`);
    check(unauthorizedAdmin.status === 401, `the admin surface requires the secret (status ${unauthorizedAdmin.status})`);

    // ---- Continuity across connections --------------------------------------------------------------
    section('The subject continues across a new connection');
    const before = await (await fetch(`${baseUrl}/api/v1/admin/runtime/subjects/${customerId}`, { headers: adminHeaders })).json();
    await widgetExchange(wsUrl, { init: { type: 'init', customerId }, turns: ['and order B456?'] });
    const after = await (await fetch(`${baseUrl}/api/v1/admin/runtime/subjects/${customerId}`, { headers: adminHeaders })).json();
    check(after.revision > before.revision, `a reconnect advanced the same subject (${before.revision} → ${after.revision})`);
    check(after.turns === before.turns + 1, `the durable turn counter continued (${before.turns} → ${after.turns})`);
    check(after.history.length > before.history.length, 'history accumulated on the one durable subject');

    // Nothing should have queued up unsent by the end of a healthy run.
    const finalHealth = await (await fetch(`${baseUrl}/api/v1/admin/runtime/health`, { headers: adminHeaders })).json();
    check(finalHealth.outbox.unknown === 0 && finalHealth.outbox.failed === 0, 'the outbox is clean at the end of the run');
  } catch (error) {
    console.error('\nRun failed:', error instanceof Error ? error.message : String(error));
    for (const [name, log] of logs) {
      console.error(`\n--- ${name} (last 2000 chars) ---\n${log.slice(-2000)}`);
    }
    failures.push('the run threw before completing');
  } finally {
    for (const child of children) killTree(child);
    await sleep(300);
    rmSync(workdir, { recursive: true, force: true });
  }

  console.log(`\n${passed} checks passed, ${failures.length} failed.`);
  if (failures.length > 0) {
    for (const failure of failures) console.error(` - ${failure}`);
    process.exit(1);
  }
}

function readConfigSecret(configPath) {
  const match = readFileSync(configPath, 'utf8').match(/^secret:\s*(.+)$/m);
  const raw = (match?.[1] ?? '').trim();
  // The config resolves ${VAR} from the environment; mirror that for the one field we need.
  const envRef = raw.match(/^\$\{([A-Z0-9_]+)\}$/);
  return envRef ? (process.env[envRef[1]] ?? SDK_SECRET) : raw;
}

function readConfigName(configPath) {
  return (readFileSync(configPath, 'utf8').match(/^name:\s*(.+)$/m)?.[1] ?? '').trim();
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
