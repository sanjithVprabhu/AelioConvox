#!/usr/bin/env node
/**
 * Upgrade / rollback drill across two adjacent Aelio DB releases.
 *
 * The column-id fix changed what a column id means on disk. This drill answers, with real binaries
 * rather than reasoning, what actually happens when a data directory crosses that boundary in
 * either direction — and specifically whether a failure is LOUD or SILENT.
 *
 * A loud failure (error, or a missing value that the client rejects) is survivable: an operator
 * sees it. A silent failure — plausible-looking but wrong values — is the dangerous outcome, and is
 * what this drill is really hunting for.
 *
 *   1. old writes → new reads       (upgrade)
 *   2. new writes → old reads       (rollback)
 *   3. a build refuses to boot against a schema newer than it knows (via the real server)
 *
 * It needs the previous release's ll-server binary:
 *   git worktree add /tmp/prev-release <previous-commit>
 *   (cd /tmp/prev-release/Sunjet/Astrolobe && cargo build -p ll-server)
 *   PREV_LL_SERVER=/tmp/prev-release/Sunjet/Astrolobe/target/debug/ll-server \
 *     node scripts/test-upgrade-rollback-drill.mjs
 *
 * Without PREV_LL_SERVER it skips the cross-version scenarios and says so rather than passing
 * vacuously.
 */

import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { SunjetClient } from '@aelio/sunjet-client';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');

let passed = 0;
const failures = [];
const notes = [];
const check = (condition, message) => {
  if (condition) {
    passed += 1;
    console.log(`  OK  ${message}`);
  } else {
    failures.push(message);
    console.error(`  FAIL ${message}`);
  }
};
const note = (message) => {
  notes.push(message);
  console.log(`  ..  ${message}`);
};
const section = (title) => console.log(`\n— ${title}`);
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

function currentBinary() {
  const binary = ['target/release/ll-server', 'target/debug/ll-server']
    .map((relative) => join(ROOT, 'Sunjet/Astrolobe', relative))
    .filter((candidate) => existsSync(candidate))
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
  if (!binary) throw new Error('No current ll-server binary; run `cargo build -p ll-server`.');
  return binary;
}

let nextPort = 20100 + Math.floor(Math.random() * 400);

/** Run `body` against an ll-server of the given binary over `dataDir`, then stop it. */
async function withServer(binary, dataDir, body) {
  const port = nextPort++;
  const url = `http://127.0.0.1:${port}`;
  const child = spawn(binary, [], {
    env: { ...process.env, LL_DATA_DIR: dataDir, LL_BIND: `127.0.0.1:${port}` },
    stdio: ['ignore', 'ignore', 'pipe'],
    detached: true,
  });
  let log = '';
  child.stderr.on('data', (chunk) => { log += chunk.toString(); });
  try {
    const deadline = Date.now() + 30_000;
    let healthy = false;
    while (Date.now() < deadline) {
      try {
        if ((await fetch(`${url}/v1/health`)).ok) { healthy = true; break; }
      } catch { /* not listening yet */ }
      await sleep(200);
    }
    if (!healthy) throw new Error(`ll-server did not start on ${url}: ${log.slice(-500)}`);
    return await body(new SunjetClient({ baseUrl: url }), url);
  } finally {
    try { process.kill(-child.pid, 'SIGKILL'); } catch { child.kill('SIGKILL'); }
    child.stderr.destroy();
    await sleep(250);
  }
}

/**
 * Two tables whose column ordinals carry DIFFERENT types — the exact shape that collided under
 * per-table column ids. `alpha.n` (i64) sits at the same ordinal as `beta.s` (utf8).
 */
const ALPHA = [{ name: 'k', kind: 'utf8' }, { name: 'n', kind: 'i64' }, { name: 'blob', kind: 'text' }];
const BETA = [{ name: 'k', kind: 'utf8' }, { name: 's', kind: 'utf8' }, { name: 'm', kind: 'i64' }];

async function seed(client, label) {
  await client.ensureTable('alpha', ALPHA);
  await client.ensureTable('beta', BETA);
  for (let i = 1; i <= 4; i += 1) {
    await client.transact([{
      op: 'insert', table: 'alpha',
      values: { k: { type: 'utf8', value: `${label}-a${i}` }, n: { type: 'i64', value: i * 100 }, blob: { type: 'utf8', value: 'x' } },
    }]);
    await client.transact([{
      op: 'insert', table: 'beta',
      values: { k: { type: 'utf8', value: `${label}-b${i}` }, s: { type: 'utf8', value: `s${i}` }, m: { type: 'i64', value: i } },
    }]);
  }
  // Flush is what moves rows into a segment keyed by column id — the moment the bug bit.
  await client.flush();
}

/**
 * Classify what a reader sees across BOTH tables.
 *
 * Checking only one side of a collision is how this drill first passed vacuously: when two tables
 * share a column id, `infer_kind` types it from whichever row it meets first, so exactly one side
 * survives and the other is dropped. Whichever side "wins" depends on row order, so the only
 * meaningful assertion covers every column that could have lost.
 *
 * `intact` = every value read back as written. `lossy` = a column is gone (loud; the client sees a
 * missing field). `corrupt` = a value came back CHANGED, the only genuinely unsafe outcome.
 */
async function classify(client, label) {
  const expectations = [
    { table: 'alpha', prefix: `${label}-a`, column: 'n', type: 'i64', expected: (i) => i * 100 },
    { table: 'beta', prefix: `${label}-b`, column: 's', type: 'utf8', expected: (i) => `s${i}` },
    { table: 'beta', prefix: `${label}-b`, column: 'm', type: 'i64', expected: (i) => i },
  ];
  const lossy = [];
  const corrupt = [];
  let seen = 0;

  for (const expectation of expectations) {
    let rows;
    try {
      rows = (await client.scanRows(expectation.table, { k: 50 })).rows
        .filter((row) => row.values.k?.value?.startsWith(expectation.prefix));
    } catch (error) {
      return { verdict: 'unreadable', detail: `${expectation.table}: ${String(error).slice(0, 100)}` };
    }
    if (rows.length === 0) {
      return { verdict: 'missing_rows', detail: `no ${expectation.table} rows for ${label}` };
    }
    seen += rows.length;
    for (const row of rows) {
      const index = Number(row.values.k.value.split(expectation.prefix)[1]);
      const cell = row.values[expectation.column];
      if (cell === undefined) {
        lossy.push(`${expectation.table}.${expectation.column}`);
      } else if (cell.type !== expectation.type || cell.value !== expectation.expected(index)) {
        corrupt.push(`${expectation.table}.${expectation.column}=${JSON.stringify(cell)}`);
      }
    }
  }

  if (corrupt.length > 0) {
    return { verdict: 'corrupt', detail: `changed values: ${[...new Set(corrupt)].slice(0, 3).join(', ')}` };
  }
  if (lossy.length > 0) {
    const dropped = [...new Set(lossy)];
    return { verdict: 'lossy', detail: `dropped ${dropped.join(', ')} across ${lossy.length} cells` };
  }
  return { verdict: 'intact', detail: `${seen} cells across both tables read back exactly` };
}

async function main() {
  const workdir = mkdtempSync(join(tmpdir(), 'aelio-upgrade-drill-'));
  const current = currentBinary();
  const previous = process.env.PREV_LL_SERVER;

  try {
    section('Baseline: the current release round-trips its own data through a flush');
    {
      const dir = join(workdir, 'current-only');
      await withServer(current, dir, (client) => seed(client, 'cur'));
      const verdict = await withServer(current, dir, (client) => classify(client, 'cur'));
      check(verdict.verdict === 'intact', `current → current is intact (${verdict.verdict}: ${verdict.detail})`);
    }

    if (!previous || !existsSync(previous)) {
      section('Cross-version scenarios SKIPPED');
      note('PREV_LL_SERVER is not set to an existing binary, so upgrade/rollback were not exercised.');
      note('Build the previous release in a worktree and re-run — see this file\'s header.');
    } else {
      section('Scenario 1 — UPGRADE: previous release writes, current release reads');
      {
        const dir = join(workdir, 'upgrade');
        await withServer(previous, dir, (client) => seed(client, 'old'));
        const asOld = await withServer(previous, dir, (client) => classify(client, 'old'));
        note(`the previous release reading its own flushed data: ${asOld.verdict} — ${asOld.detail}`);

        const asNew = await withServer(current, dir, (client) => classify(client, 'old'));
        check(
          asNew.verdict !== 'corrupt',
          `upgrading does not return changed values (${asNew.verdict}: ${asNew.detail})`,
        );
        if (asNew.verdict === 'intact') {
          note('the current release read the previous release\'s data intact — an in-place upgrade is viable here.');
        } else {
          note(`an in-place upgrade is NOT viable: ${asNew.verdict} — ${asNew.detail}`);
          note('a pre-fix database must be recreated, and the loss is visible rather than silent.');
        }
      }

      section('Scenario 2 — ROLLBACK: current release writes, previous release reads');
      {
        const dir = join(workdir, 'rollback');
        await withServer(current, dir, (client) => seed(client, 'new'));
        const asOld = await withServer(previous, dir, (client) => classify(client, 'new'));
        check(
          asOld.verdict !== 'corrupt',
          `rolling back does not return changed values (${asOld.verdict}: ${asOld.detail})`,
        );
        if (asOld.verdict === 'intact') {
          note('the previous release read the current release\'s data intact — rollback is read-safe.');
          note('WARNING: it would still re-introduce the id collision on any table it creates itself.');
        } else {
          note(`rollback is not read-safe: ${asOld.verdict} — ${asOld.detail}`);
        }
      }

      section('Scenario 3 — the previous release must not silently corrupt data it then writes');
      {
        // Roll back and WRITE, then come forward again. This is the dangerous real-world sequence:
        // a deploy is reverted, serves traffic, and is then rolled forward.
        const dir = join(workdir, 'roundtrip');
        await withServer(current, dir, (client) => seed(client, 'fwd'));
        await withServer(previous, dir, (client) => seed(client, 'back'));
        const forward = await withServer(current, dir, async (client) => ({
          fwd: await classify(client, 'fwd'),
          back: await classify(client, 'back'),
        }));
        check(
          forward.fwd.verdict !== 'corrupt' && forward.back.verdict !== 'corrupt',
          `a rollback-then-roll-forward never returns changed values (fwd: ${forward.fwd.verdict}, back: ${forward.back.verdict})`,
        );
        note(`rows written before the rollback: ${forward.fwd.verdict} — ${forward.fwd.detail}`);
        note(`rows written during the rollback: ${forward.back.verdict} — ${forward.back.detail}`);
      }
    }

    section('Scenario 4 — a build refuses a schema newer than it knows');
    {
      const { AelioMigrationRunner, aelioSchemaMigrations, migrateAelioStorage } = await import('@aelio/core');
      const dir = join(workdir, 'future-schema');
      const tables = futureSchemaTables();
      const outcome = await withServer(current, dir, async (client) => {
        await migrateAelioStorage(client, tables, 64);
        // Simulate the next release having already migrated this database.
        const runner = new AelioMigrationRunner(client, tables, 'future-release');
        await runner.migrate([
          ...aelioSchemaMigrations(64),
          { id: '9999-from-the-future', version: 99, kind: 'additive', description: 'a later release', apply: async () => {} },
        ]);
        // Now this build boots again, knowing nothing about v99.
        try {
          await migrateAelioStorage(client, tables, 64);
          return { refused: false, message: '' };
        } catch (error) {
          return { refused: true, message: String(error) };
        }
      });
      check(outcome.refused, 'this build refuses to boot against a database migrated by a later release');
      check(
        /does not know about/.test(outcome.message),
        `the refusal explains why (${outcome.message.slice(0, 140)})`,
      );
      note('This guard only exists from the migration-ledger release onward: rolling back to a build');
      note('that predates the ledger is unprotected, because that build has nothing to check.');
    }
  } finally {
    rmSync(workdir, { recursive: true, force: true });
  }

  console.log(`\n${passed} checks passed, ${failures.length} failed, ${notes.length} findings recorded.`);
  if (failures.length > 0) {
    for (const failure of failures) console.error(` - ${failure}`);
    process.exit(1);
  }
}

function futureSchemaTables() {
  const keys = [
    'messages', 'conversations', 'memories', 'compactions', 'runtimeState',
    'harnessTools', 'harnessCapabilities', 'harnessBindings', 'harnessSuspensions',
    'harnessLedger', 'harnessTraces', 'runtimeEvents', 'runtimeSnapshots', 'runtimeLedger',
    'runtimeOutbox', 'runtimeContinuations', 'scheduledEvents', 'workflowArtifacts',
    'workflowInstances', 'promptArtifacts', 'promptLedger', 'migrations',
  ];
  return Object.fromEntries(keys.map((key) => [key, `fut_${key}`]));
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
