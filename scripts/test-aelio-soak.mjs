#!/usr/bin/env node
/**
 * Aelio DB / runtime soak.
 *
 * A soak answers a different question from the other suites. They ask "is it correct?"; this asks
 * "does it stay correct and bounded while it runs?" — the failures it hunts are leaks, unbounded
 * storage growth, and latency drift, none of which a seconds-long test can see.
 *
 * It runs a continuous mixed workload — transactional commits, outbox dispatch, scheduled events,
 * artifact instances, memory writes and recalls, periodic flush and compact — and samples on a
 * fixed interval:
 *
 *   - ll-server RSS                (leak detection)
 *   - data directory bytes + files (compaction actually reclaiming, WAL not growing forever)
 *   - per-window latency p50/p95   (drift under sustained load)
 *   - correctness invariants       (checked continuously, not just at the end)
 *
 * The verdict compares the LAST quarter of samples against the FIRST quarter, normalised by the
 * work done, so a bounded working set passes and a genuine leak fails. Growth that merely tracks
 * the dataset is expected and is reported rather than failed.
 *
 * Honest limits: this is minutes, not days, and it runs on whatever machine invokes it. It can
 * catch an obvious leak or unbounded WAL; it cannot certify multi-day stability.
 *
 * ── THIS SUITE CURRENTLY FAILS ONE CHECK, ON PURPOSE ──────────────────────────────────────────
 * A 15-minute run on 2026-08-08 measured memory per stored row growing 122% (7.15 → 15.86 KiB),
 * i.e. RSS growing roughly twice as fast as the data: ~3 GiB resident for ~147 MiB on disk. The
 * `perRowGrowth` assertion is left FAILING deliberately — it documents an open defect rather than
 * being relaxed to go green. See the status ledger's "Soak findings" section. When compaction and
 * retention are fixed this check should pass on its own and prove it.
 * ──────────────────────────────────────────────────────────────────────────────────────────────
 *
 * Usage: node scripts/test-aelio-soak.mjs
 *        AELIO_SOAK_MINUTES=45 AELIO_SOAK_CONCURRENCY=16 node scripts/test-aelio-soak.mjs
 */

import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  AelioMemoryStore,
  AelioRuntimeStore,
  bootstrapSunjetTables,
  dispatchPendingEffects,
} from '@aelio/core';
import { SunjetClient } from '@aelio/sunjet-client';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const TENANT = 'aelio-soak';
const EMBED_DIM = 64;
const MINUTES = Number(process.env.AELIO_SOAK_MINUTES ?? 12);
const CONCURRENCY = Number(process.env.AELIO_SOAK_CONCURRENCY ?? 8);
const SAMPLE_MS = Number(process.env.AELIO_SOAK_SAMPLE_MS ?? 15_000);
/** Subjects are reused so the working set stays bounded — an ever-growing key space is not a leak. */
const SUBJECT_POOL = Number(process.env.AELIO_SOAK_SUBJECTS ?? 250);

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
  const release = join(ROOT, 'Sunjet/Astrolobe/target/release/ll-server');
  const debug = join(ROOT, 'Sunjet/Astrolobe/target/debug/ll-server');
  if (existsSync(release)) return { binary: release, isRelease: true };
  if (existsSync(debug)) return { binary: debug, isRelease: false };
  throw new Error('No ll-server binary; run `cargo build --release -p ll-server` in Sunjet/Astrolobe.');
}

function directoryBytes(dir) {
  let bytes = 0;
  let files = 0;
  const walk = (path) => {
    for (const entry of readdirSync(path, { withFileTypes: true })) {
      const child = join(path, entry.name);
      if (entry.isDirectory()) walk(child);
      else {
        try {
          bytes += statSync(child).size;
          files += 1;
        } catch { /* file vanished mid-walk (compaction); skip it */ }
      }
    }
  };
  try { walk(dir); } catch { /* dir vanished */ }
  return { bytes, files };
}

/** Resident set size in MiB for a pid, read from /proc (Linux); null where unavailable. */
function residentMib(pid) {
  try {
    const pages = Number(readFileSync(`/proc/${pid}/statm`, 'utf8').split(' ')[1]);
    return Number(((pages * 4096) / (1024 * 1024)).toFixed(1));
  } catch {
    return null;
  }
}

const percentile = (sorted, p) => sorted[Math.min(sorted.length - 1, Math.floor((sorted.length - 1) * p))] ?? 0;
const mean = (values) => (values.length === 0 ? 0 : values.reduce((sum, value) => sum + value, 0) / values.length);

function tableNames(suffix) {
  const keys = [
    'messages', 'conversations', 'memories', 'compactions', 'runtimeState',
    'harnessTools', 'harnessCapabilities', 'harnessBindings', 'harnessSuspensions',
    'harnessLedger', 'harnessTraces', 'runtimeEvents', 'runtimeSnapshots', 'runtimeLedger',
    'runtimeOutbox', 'runtimeContinuations', 'scheduledEvents', 'workflowArtifacts',
    'workflowInstances', 'promptArtifacts', 'promptLedger', 'migrations',
  ];
  return Object.fromEntries(keys.map((key) => [key, `soak_${key}_${suffix}`]));
}

const event = (subjectId, key) => ({
  apiVersion: 'aelio.runtime.event/v1',
  eventId: randomUUID(),
  idempotencyKey: key,
  tenantId: TENANT,
  subjectId,
  kind: 'user_message',
  payload: { message: 'soak', channel: 'web' },
  receivedAt: Date.now(),
});

async function main() {
  const { binary, isRelease } = llServerBinary();
  const dataDir = mkdtempSync(join(tmpdir(), 'aelio-soak-'));
  const port = 20900 + Math.floor(Math.random() * 200);
  const url = `http://127.0.0.1:${port}`;

  const child = spawn(binary, [], {
    env: { ...process.env, LL_DATA_DIR: dataDir, LL_BIND: `127.0.0.1:${port}` },
    stdio: ['ignore', 'ignore', 'pipe'],
    detached: true,
  });
  let serverLog = '';
  child.stderr.on('data', (chunk) => { serverLog = (serverLog + chunk.toString()).slice(-8_000); });

  const stop = () => {
    try { process.kill(-child.pid, 'SIGKILL'); } catch { child.kill('SIGKILL'); }
    child.stderr.destroy();
  };

  try {
    const deadline = Date.now() + 40_000;
    let healthy = false;
    while (Date.now() < deadline) {
      try { if ((await fetch(`${url}/v1/health`)).ok) { healthy = true; break; } } catch { /* booting */ }
      await sleep(200);
    }
    if (!healthy) throw new Error(`Aelio DB did not start: ${serverLog.slice(-400)}`);

    console.log(`Aelio DB: ${url} (${isRelease ? 'release' : 'DEBUG'} build, pid ${child.pid})`);
    if (!isRelease) info('DEBUG build — latency figures are not meaningful; leak/growth signals still are.');
    info(`plan: ${MINUTES} min, ${CONCURRENCY} workers, ${SUBJECT_POOL} recycled subjects, sample every ${SAMPLE_MS / 1000}s`);

    const client = new SunjetClient({ baseUrl: url });
    const tables = tableNames(String(Date.now()).slice(-8));
    await bootstrapSunjetTables(client, tables, EMBED_DIM);
    const store = new AelioRuntimeStore(client, tables);
    const memory = new AelioMemoryStore(client, tables, EMBED_DIM, TENANT);
    const subjects = Array.from({ length: SUBJECT_POOL }, (_, index) => `soak-subject-${index}`);

    const samples = [];
    let commits = 0;
    let refusals = 0;
    let errors = 0;
    const errorSamples = [];
    let windowLatencies = [];
    let running = true;

    /** One unit of mixed work. Subjects recycle, so revisions climb and rows accumulate per subject. */
    const doWork = async () => {
      const subject = subjects[Math.floor(Math.random() * subjects.length)];
      const started = Date.now();
      const current = await store.loadSnapshot(TENANT, subject);
      const result = await store.commit({
        event: event(subject, `soak-${randomUUID()}`),
        state: {
          version: 1,
          turns: (current?.state.turns ?? 0) + 1,
          // A bounded history: this models a real subject, not an ever-growing blob.
          history: [{ role: 'user', content: `turn ${(current?.state.turns ?? 0) + 1}`, at: Date.now() }],
        },
        instanceId: randomUUID(),
        ledger: [{ recordId: randomUUID(), instanceId: 'i', kind: 'decision', payload: { kind: 'reply' } }],
        effects: [{
          effectId: randomUUID(), idempotencyKey: `soak-effect-${randomUUID()}`,
          kind: 'reply.send', payload: { channel: 'web', to: subject, text: 'ok' },
        }],
        expectedRevision: current?.revision ?? 0,
      });
      windowLatencies.push(Date.now() - started);
      if (result.applied) commits += 1;
      else refusals += 1;
    };

    const worker = async () => {
      while (running) {
        try {
          await doWork();
        } catch (error) {
          errors += 1;
          if (errorSamples.length < 5) errorSamples.push(String(error).slice(0, 160));
        }
      }
    };

    // Background pressure: the outbox drains continuously, and storage maintenance runs on a
    // cycle, so compaction and WAL truncation are actually exercised rather than skipped.
    const maintenance = async () => {
      let cycle = 0;
      while (running) {
        try {
          await dispatchPendingEffects(store, async () => {}, 64);
          cycle += 1;
          if (cycle % 4 === 0) await client.flush();
          if (cycle % 12 === 0) await client.compact();
        } catch (error) {
          errors += 1;
          if (errorSamples.length < 5) errorSamples.push(`maintenance: ${String(error).slice(0, 140)}`);
        }
        await sleep(1_000);
      }
    };

    // Memory is on the hot path of a real turn, so soak it too — recall is a vector query.
    const memoryLoad = async () => {
      while (running) {
        try {
          const subject = subjects[Math.floor(Math.random() * subjects.length)];
          await memory.remember({ subjectId: subject, content: `soak fact ${randomUUID()}`, category: 'fact' });
          await memory.recall(subject, 'soak fact', 5);
        } catch (error) {
          errors += 1;
          if (errorSamples.length < 5) errorSamples.push(`memory: ${String(error).slice(0, 140)}`);
        }
        await sleep(500);
      }
    };

    const sampler = async () => {
      while (running) {
        await sleep(SAMPLE_MS);
        if (!running) break;
        const latencies = windowLatencies;
        windowLatencies = [];
        const sorted = [...latencies].sort((a, b) => a - b);
        const { bytes, files } = directoryBytes(dataDir);
        const sample = {
          at: Date.now(),
          rssMib: residentMib(child.pid),
          diskMib: Number((bytes / (1024 * 1024)).toFixed(1)),
          files,
          commits,
          // Rows written so far: each commit writes 1 event + 1 snapshot + 1 ledger + 1 outbox.
          // A proxy, but the workload is uniform, so it tracks the real row count closely.
          rows: commits * 4,
          p50: percentile(sorted, 0.5),
          p95: percentile(sorted, 0.95),
          ops: latencies.length,
        };
        samples.push(sample);
        console.log(
          `  [${new Date(sample.at).toISOString().slice(11, 19)}] rss ${sample.rssMib ?? '?'}MiB · disk ${sample.diskMib}MiB/${sample.files} files · ` +
            `${sample.ops} ops · p50 ${sample.p50}ms · p95 ${sample.p95}ms · commits ${commits} · errors ${errors}`,
        );
      }
    };

    section(`Soaking for ${MINUTES} minutes`);
    const workers = [
      ...Array.from({ length: CONCURRENCY }, () => worker()),
      maintenance(),
      memoryLoad(),
      sampler(),
    ];
    await sleep(MINUTES * 60_000);
    running = false;
    await Promise.all(workers);

    // ---- Verdict ---------------------------------------------------------------------------
    section('Verdict');
    check(errors === 0, `no errors during the soak (${errors}${errorSamples[0] ? `; first: ${errorSamples[0]}` : ''})`);
    check(commits > 0, `the workload actually ran (${commits} commits, ${refusals} CAS refusals)`);
    check(samples.length >= 4, `enough samples to see a trend (${samples.length})`);

    if (samples.length >= 4) {
      const quarter = Math.max(1, Math.floor(samples.length / 4));
      const first = samples.slice(0, quarter);
      const last = samples.slice(-quarter);
      const firstRss = mean(first.map((sample) => sample.rssMib ?? 0));
      const lastRss = mean(last.map((sample) => sample.rssMib ?? 0));
      const firstP95 = mean(first.map((sample) => sample.p95));
      const lastP95 = mean(last.map((sample) => sample.p95));
      const rssGrowth = firstRss > 0 ? (lastRss - firstRss) / firstRss : 0;
      const p95Growth = firstP95 > 0 ? (lastP95 - firstP95) / firstP95 : 0;
      const diskFirst = first[0].diskMib;
      const diskLast = last[last.length - 1].diskMib;

      info(`RSS   ${firstRss.toFixed(1)}MiB → ${lastRss.toFixed(1)}MiB (${(rssGrowth * 100).toFixed(0)}%)`);
      info(`p95   ${firstP95.toFixed(0)}ms → ${lastP95.toFixed(0)}ms (${(p95Growth * 100).toFixed(0)}%)`);
      info(`disk  ${diskFirst}MiB → ${diskLast}MiB across ${samples[samples.length - 1].files} files`);
      info(`work  ${commits} commits over ${MINUTES} min (${(commits / (MINUTES * 60)).toFixed(1)}/s)`);

      // Absolute RSS growth is NOT the leak test here: Aelio DB reads every segment fully into
      // memory (`read_file` → `fs::read`), and this workload's event/ledger/outbox tables are
      // append-only, so the dataset itself grows without bound. Memory is therefore expected to
      // track the data.
      //
      // The leak test is whether memory grows FASTER than the data. Bytes-per-row staying flat
      // means growth is proportional (architectural); bytes-per-row climbing means a real leak.
      const firstRows = mean(first.map((sample) => sample.rows));
      const lastRows = mean(last.map((sample) => sample.rows));
      const firstPerRow = firstRows > 0 ? (firstRss * 1024) / firstRows : 0;
      const lastPerRow = lastRows > 0 ? (lastRss * 1024) / lastRows : 0;
      const perRowGrowth = firstPerRow > 0 ? (lastPerRow - firstPerRow) / firstPerRow : 0;

      info(`rows  ~${Math.round(firstRows)} → ~${Math.round(lastRows)} (append-only tables never shrink)`);
      info(`KiB/row ${firstPerRow.toFixed(2)} → ${lastPerRow.toFixed(2)} (${(perRowGrowth * 100).toFixed(0)}%)`);

      check(
        perRowGrowth < 0.5,
        `memory per stored row did not grow more than 50% (${(perRowGrowth * 100).toFixed(0)}%) — growth tracks data, not a leak`,
      );
      // Latency drifting several-fold under constant offered load is the classic soak signal.
      check(p95Growth < 3.0, `p95 latency did not degrade more than 4x (${(p95Growth * 100).toFixed(0)}%)`);
      check(diskLast > 0, 'the data directory holds the dataset on disk');

      info('');
      info('FINDING — memory scales with LIFETIME data, not working set. Segments are read fully');
      info('into memory, and runtime_events / runtime_ledger / runtime_outbox are append-only with');
      info('no retention or pruning. A long-running deployment grows RSS until it is out of memory,');
      info('however small its active subject set. This needs a retention policy before any');
      info('long-lived production use; it is recorded as a hard gate in the status ledger.');
    }

    // Correctness must still hold after sustained load, not just at the start.
    section('Correctness after sustained load');
    {
      const sampled = subjects.slice(0, 5);
      const snapshots = await Promise.all(sampled.map((subject) => store.loadSnapshot(TENANT, subject)));
      check(snapshots.every((snapshot) => snapshot !== null), 'sampled subjects still resolve');
      check(
        snapshots.every((snapshot) => typeof snapshot.state.turns === 'number' && snapshot.revision >= 1),
        'sampled snapshots are structurally intact after the soak',
      );
      // Every accepted commit advanced exactly one revision, so revisions must equal turns.
      check(
        snapshots.every((snapshot) => snapshot.revision === snapshot.state.turns),
        `revision equals the turn count on every sampled subject (${snapshots.map((s) => `${s.revision}/${s.state.turns}`).join(' ')})`,
      );
      const health = await store.health();
      check(health.outbox.unknown === 0, `no effect ended unknown (${health.outbox.unknown})`);
      check(health.outbox.failed === 0, `no effect failed terminally (${health.outbox.failed})`);
    }

    section('Survives a restart after sustained load');
    {
      stop();
      await sleep(600);
      const restarted = spawn(binary, [], {
        env: { ...process.env, LL_DATA_DIR: dataDir, LL_BIND: `127.0.0.1:${port}` },
        stdio: ['ignore', 'ignore', 'ignore'],
        detached: true,
      });
      try {
        const deadline2 = Date.now() + 60_000;
        let ok = false;
        while (Date.now() < deadline2) {
          try { if ((await fetch(`${url}/v1/health`)).ok) { ok = true; break; } } catch { /* recovering */ }
          await sleep(300);
        }
        check(ok, 'Aelio DB recovers after a soak-sized WAL/segment set');
        if (ok) {
          const after = new AelioRuntimeStore(new SunjetClient({ baseUrl: url }), tables);
          const snapshot = await after.loadSnapshot(TENANT, subjects[0]);
          check(snapshot !== null && snapshot.revision >= 1, 'a sampled subject survived the restart');
        }
      } finally {
        try { process.kill(-restarted.pid, 'SIGKILL'); } catch { restarted.kill('SIGKILL'); }
      }
    }
  } finally {
    stop();
    await sleep(300);
    rmSync(dataDir, { recursive: true, force: true });
  }

  console.log(`\n${passed} checks passed, ${failures.length} failed.`);
  console.log(`Duration soaked: ${MINUTES} minutes. This is not a multi-day stability result.`);
  if (failures.length > 0) {
    for (const failure of failures) console.error(` - ${failure}`);
    process.exit(1);
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
