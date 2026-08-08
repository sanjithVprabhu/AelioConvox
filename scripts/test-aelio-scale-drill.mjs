#!/usr/bin/env node
/**
 * Aelio DB scale drill: restore integrity at volume, plus a concurrency burst.
 *
 * Two things this is NOT:
 *  - It is not a soak test. A soak needs hours of continuous run on representative hardware to say
 *    anything about memory growth or compaction behaviour over time. This is a burst.
 *  - It is not a capacity benchmark. It runs on whatever machine invokes it, against the debug
 *    build if that is the newest binary. The latency numbers are a smoke signal, not a target.
 *
 * What it does establish:
 *  1. RESTORE AT VOLUME — write a large runtime dataset, flush to segments, compact, SIGKILL,
 *     restart, and prove every row hashes identically. The small restore drill in the fault suite
 *     runs 167 rows; a segment/compaction bug can easily hide below that.
 *  2. CONCURRENCY BURST — drive many subjects through real transactional commits in parallel and
 *     assert correctness under contention: every commit is accounted for, no snapshot is lost, and
 *     no duplicate is admitted. Throughput and p50/p95 are reported for information only.
 *
 * Usage: node scripts/test-aelio-scale-drill.mjs
 *        AELIO_SCALE_ROWS=20000 AELIO_SCALE_SUBJECTS=400 node scripts/test-aelio-scale-drill.mjs
 */

import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, mkdtempSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { AelioRuntimeStore, bootstrapSunjetTables } from '@aelio/core';
import { SunjetClient } from '@aelio/sunjet-client';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const TENANT = 'aelio-scale-drill';
const EMBED_DIM = 64;
const TARGET_ROWS = Number(process.env.AELIO_SCALE_ROWS ?? 6_000);
const BURST_SUBJECTS = Number(process.env.AELIO_SCALE_SUBJECTS ?? 200);
const BURST_CONCURRENCY = Number(process.env.AELIO_SCALE_CONCURRENCY ?? 32);

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
const info = (message) => console.log(`  ..  ${message}`);
const section = (title) => console.log(`\n— ${title}`);
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

function llServerBinary() {
  const binary = ['target/release/ll-server', 'target/debug/ll-server']
    .map((relative) => join(ROOT, 'Sunjet/Astrolobe', relative))
    .filter((candidate) => existsSync(candidate))
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
  if (!binary) throw new Error('No ll-server binary; run `cargo build -p ll-server` in Sunjet/Astrolobe.');
  return { binary, isRelease: binary.includes('/release/') };
}

async function startAelioDb() {
  const { binary, isRelease } = llServerBinary();
  const port = 20600 + Math.floor(Math.random() * 200);
  const dataDir = mkdtempSync(join(tmpdir(), 'aelio-scale-'));
  const url = `http://127.0.0.1:${port}`;
  let child = null;

  const boot = async () => {
    child = spawn(binary, [], {
      env: { ...process.env, LL_DATA_DIR: dataDir, LL_BIND: `127.0.0.1:${port}` },
      stdio: ['ignore', 'ignore', 'pipe'],
      detached: true,
    });
    let log = '';
    child.stderr.on('data', (chunk) => { log += chunk.toString(); });
    const deadline = Date.now() + 40_000;
    while (Date.now() < deadline) {
      try {
        if ((await fetch(`${url}/v1/health`)).ok) return;
      } catch { /* not listening yet */ }
      await sleep(200);
    }
    throw new Error(`Aelio DB did not start: ${log.slice(-500)}`);
  };
  const stop = () => {
    try { process.kill(-child.pid, 'SIGKILL'); } catch { child?.kill('SIGKILL'); }
    child?.stderr?.destroy();
  };

  await boot();
  return {
    url,
    isRelease,
    hardRestart: async () => { stop(); await sleep(500); await boot(); },
    stop: async () => { stop(); await sleep(300); rmSync(dataDir, { recursive: true, force: true }); },
  };
}

function tableNames(suffix) {
  const keys = [
    'messages', 'conversations', 'memories', 'compactions', 'runtimeState',
    'harnessTools', 'harnessCapabilities', 'harnessBindings', 'harnessSuspensions',
    'harnessLedger', 'harnessTraces', 'runtimeEvents', 'runtimeSnapshots', 'runtimeLedger',
    'runtimeOutbox', 'runtimeContinuations', 'scheduledEvents', 'workflowArtifacts',
    'workflowInstances', 'promptArtifacts', 'promptLedger', 'migrations',
  ];
  return Object.fromEntries(keys.map((key) => [key, `scale_${key}_${suffix}`]));
}

const event = (subjectId, key) => ({
  apiVersion: 'aelio.runtime.event/v1',
  eventId: randomUUID(),
  idempotencyKey: key,
  tenantId: TENANT,
  subjectId,
  kind: 'user_message',
  payload: { message: 'scale drill', channel: 'web' },
  receivedAt: Date.now(),
});

/** Content hash of every runtime table, order-independent (see the fault suite's restore drill). */
async function hashRuntimeTables(client, tables) {
  const runtimeTables = [
    tables.runtimeEvents, tables.runtimeSnapshots, tables.runtimeLedger, tables.runtimeOutbox,
    tables.runtimeContinuations, tables.scheduledEvents, tables.workflowArtifacts, tables.workflowInstances,
  ];
  const byTable = {};
  let total = 0;
  for (const table of runtimeTables) {
    // The cap must exceed the dataset or the comparison silently comes from a truncated sample.
    const rows = await client.scanRows(table, { k: Math.max(TARGET_ROWS * 4, 4_096) });
    const perRow = rows.rows
      .map((row) => createHash('sha256').update(JSON.stringify(canonicalRow(row.values))).digest('hex'))
      .sort();
    total += perRow.length;
    byTable[table] = createHash('sha256').update(perRow.join('|')).digest('hex').slice(0, 32);
  }
  return { byTable, total };
}

function canonicalRow(values) {
  return Object.keys(values)
    .sort()
    .map((key) => {
      const value = values[key];
      if (value?.type === 'vector') return [key, `vector(${value.value.length})`];
      return [key, value];
    });
}

/** Run `tasks` with bounded parallelism, collecting per-task duration. */
async function runPool(tasks, concurrency) {
  const durations = [];
  const results = [];
  let cursor = 0;
  const workers = Array.from({ length: Math.min(concurrency, tasks.length) }, async () => {
    for (;;) {
      const index = cursor;
      cursor += 1;
      if (index >= tasks.length) return;
      const started = Date.now();
      try {
        results.push({ ok: true, value: await tasks[index]() });
      } catch (error) {
        results.push({ ok: false, error: String(error).slice(0, 160) });
      }
      durations.push(Date.now() - started);
    }
  });
  await Promise.all(workers);
  return { durations, results };
}

const percentile = (sorted, p) => sorted[Math.min(sorted.length - 1, Math.floor((sorted.length - 1) * p))] ?? 0;

async function main() {
  const db = await startAelioDb();
  console.log(`Aelio DB: ${db.url} (${db.isRelease ? 'release' : 'debug'} build)`);
  if (!db.isRelease) {
    info('Debug build: treat every timing below as a smoke signal only, never as a capacity number.');
  }
  const client = new SunjetClient({ baseUrl: db.url });
  const tables = tableNames(String(Date.now()).slice(-8));
  await bootstrapSunjetTables(client, tables, EMBED_DIM);
  const store = new AelioRuntimeStore(client, tables);

  try {
    // ---- 1. Restore integrity at volume -------------------------------------------------------
    section(`Restore at volume (~${TARGET_ROWS} runtime rows, flush + compact + SIGKILL)`);
    {
      // Each commit writes an event, a snapshot, two ledger records and an effect, so the row count
      // grows several times faster than the loop counter.
      const commits = Math.max(1, Math.floor(TARGET_ROWS / 5));
      const started = Date.now();
      for (let index = 0; index < commits; index += 1) {
        const subject = `bulk-${index}`;
        await store.commit({
          event: event(subject, `bulk-${index}-${randomUUID()}`),
          state: { version: 1, history: [{ role: 'user', content: `m${index}`, at: index }], turns: 1 },
          instanceId: randomUUID(),
          ledger: [
            { recordId: randomUUID(), instanceId: 'i', kind: 'event_claimed', payload: { index } },
            { recordId: randomUUID(), instanceId: 'i', kind: 'decision', payload: { kind: 'reply' } },
          ],
          effects: [{
            effectId: randomUUID(), idempotencyKey: `bulk-effect-${index}-${randomUUID()}`,
            kind: 'reply.send', payload: { channel: 'web', to: subject, text: 'ok' },
          }],
        });
      }
      const writeMs = Date.now() - started;
      info(`wrote ${commits} transactional commits in ${(writeMs / 1000).toFixed(1)}s`);

      await client.flush();
      await client.compact();
      const before = await hashRuntimeTables(client, tables);
      check(before.total >= TARGET_ROWS * 0.5, `the dataset is substantial (${before.total} rows)`);

      await db.hardRestart();
      const restored = new SunjetClient({ baseUrl: db.url });
      const after = await hashRuntimeTables(restored, tables);

      const mismatched = Object.keys(before.byTable).filter((table) => before.byTable[table] !== after.byTable[table]);
      check(mismatched.length === 0, `every table hashes identically after flush+compact+restart (${mismatched.join(', ') || 'none differ'})`);
      check(after.total === before.total, `row count is unchanged (${before.total} → ${after.total})`);

      // Spot-check readability, not just byte equality: a hash match on unreadable rows is hollow.
      const restoredStore = new AelioRuntimeStore(restored, tables);
      const sampled = await Promise.all([0, Math.floor(commits / 2), commits - 1].map((index) =>
        restoredStore.loadSnapshot(TENANT, `bulk-${index}`)));
      check(sampled.every((snapshot) => snapshot?.revision === 1), 'sampled snapshots across the dataset read back correctly');
      check(
        sampled.every((snapshot) => Array.isArray(snapshot?.state.history) && snapshot.state.history.length === 1),
        'sampled snapshot payloads survived compaction intact',
      );
    }

    // ---- 2. Concurrency burst ------------------------------------------------------------------
    section(`Concurrency burst (${BURST_SUBJECTS} subjects, ${BURST_CONCURRENCY} in flight)`);
    {
      const subjects = Array.from({ length: BURST_SUBJECTS }, () => `burst-${randomUUID()}`);
      const tasks = subjects.map((subject) => async () => {
        const result = await store.commit({
          event: event(subject, `burst-${randomUUID()}`),
          state: { version: 1, turns: 1 },
          instanceId: randomUUID(),
          ledger: [{ recordId: randomUUID(), instanceId: 'i', kind: 'decision', payload: {} }],
        });
        if (!result.applied) throw new Error(`commit refused: ${result.reason}`);
        return result.commitLsn;
      });

      const started = Date.now();
      const { durations, results } = await runPool(tasks, BURST_CONCURRENCY);
      const elapsed = Date.now() - started;
      const errors = results.filter((result) => !result.ok);
      check(errors.length === 0, `every commit succeeded under contention (${errors.length} errors${errors[0] ? `: ${errors[0].error}` : ''})`);

      // Distinct subjects must never contend, so all of them must be durable afterwards.
      const snapshots = await Promise.all(subjects.map((subject) => store.loadSnapshot(TENANT, subject)));
      check(snapshots.every((snapshot) => snapshot?.revision === 1), `all ${BURST_SUBJECTS} subjects are durable at revision 1`);

      // Commit LSNs are the durability ordering; duplicates would mean two batches shared a commit.
      const lsns = results.filter((result) => result.ok).map((result) => result.value);
      check(new Set(lsns).size === lsns.length, `every commit got its own LSN (${new Set(lsns).size}/${lsns.length} distinct)`);

      const sorted = [...durations].sort((a, b) => a - b);
      info(`${durations.length} commits in ${(elapsed / 1000).toFixed(1)}s — ${(durations.length / (elapsed / 1000)).toFixed(0)}/s`);
      info(`per-commit latency p50 ${percentile(sorted, 0.5)}ms · p95 ${percentile(sorted, 0.95)}ms · max ${sorted[sorted.length - 1]}ms`);
      info('Informational only. This is a burst on this machine, not a soak and not a capacity target.');
    }

    // ---- 3. Contention on ONE subject ----------------------------------------------------------
    section('Contention on a single subject (optimistic commits must not lose an update)');
    {
      const subject = `hot-${randomUUID()}`;
      const attempts = 40;
      // All of these target one subject, so most must lose the CAS rather than silently overwrite.
      const tasks = Array.from({ length: attempts }, (_, index) => async () => {
        const current = await store.loadSnapshot(TENANT, subject);
        return store.commit({
          event: event(subject, `hot-${randomUUID()}`),
          state: { version: 1, turns: (current?.state.turns ?? 0) + 1, writer: index },
          instanceId: randomUUID(),
          ledger: [],
          expectedRevision: current?.revision ?? 0,
        });
      });
      const { results } = await runPool(tasks, 16);
      const applied = results.filter((result) => result.ok && result.value.applied).length;
      const refused = results.filter((result) => result.ok && !result.value.applied).length;
      const snapshot = await store.loadSnapshot(TENANT, subject);
      check(applied + refused === attempts, `every attempt resolved cleanly (${applied} applied, ${refused} refused)`);
      check(applied >= 1, 'at least one writer won');
      check(
        snapshot?.revision === applied,
        `the snapshot revision equals the number of accepted writes (${snapshot?.revision} vs ${applied})`,
      );
      info(`${refused}/${attempts} writers were correctly refused instead of overwriting a newer state.`);
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
