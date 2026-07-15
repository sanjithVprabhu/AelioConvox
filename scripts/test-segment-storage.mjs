#!/usr/bin/env node
/**
 * Live smoke: ll-server local segment backend + optional cloud-config validation.
 * Verifies health.segment_backend, flush/publish on disk, reopen after local eviction
 * is not applicable for local-only — instead verifies flush creates seg-*.vss and
 * compact leaves a single segment.
 */

import { spawn } from 'node:child_process';
import { createServer } from 'node:net';
import { mkdtempSync, readdirSync, rmSync, existsSync } from 'node:fs';
import { setTimeout as sleep } from 'node:timers/promises';
import { join, resolve, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { SunjetClient } from '@aelio/sunjet-client';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const LL_BIN = join(ROOT, 'Sunjet/Astrolobe/target/release/ll-server');

let child = null;
let tmp = null;
let passed = 0;

function ok(m) {
  passed += 1;
  console.log(`OK: ${m}`);
}
function fail(m) {
  console.error(`FAIL: ${m}`);
  cleanup(1);
}
function cleanup(code = 0) {
  try {
    child?.kill('SIGTERM');
  } catch {
    /* ignore */
  }
  if (tmp) {
    try {
      rmSync(tmp, { recursive: true, force: true });
    } catch {
      /* ignore */
    }
  }
  process.exit(code);
}

async function freePort() {
  const s = createServer();
  await new Promise((r, j) => {
    s.once('error', j);
    s.listen(0, '127.0.0.1', r);
  });
  const p = s.address().port;
  await new Promise((r) => s.close(r));
  return p;
}

async function waitHealth(url) {
  for (let i = 0; i < 40; i++) {
    try {
      const r = await fetch(`${url}/v1/health`);
      if (r.ok) return r.json();
    } catch {
      /* retry */
    }
    await sleep(250);
  }
  throw new Error('ll-server not healthy');
}

async function main() {
  if (!existsSync(LL_BIN)) fail(`missing release binary: ${LL_BIN}`);

  tmp = mkdtempSync(join(tmpdir(), 'll-seg-smoke-'));
  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;

  child = spawn(LL_BIN, [], {
    env: {
      ...process.env,
      LL_DATA_DIR: tmp,
      LL_BIND: `127.0.0.1:${port}`,
      LL_SEGMENT_BACKEND: 'local',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let logs = '';
  child.stderr.on('data', (c) => {
    logs += c.toString();
  });
  child.stdout.on('data', (c) => {
    logs += c.toString();
  });

  const health = await waitHealth(url);
  if (health.status !== 'ok') fail(`bad health: ${JSON.stringify(health)}`);
  if (health.segment_backend !== 'local') {
    fail(`expected segment_backend=local, got ${health.segment_backend}`);
  }
  ok(`health segment_backend=${health.segment_backend}`);
  if (!logs.includes('segment store: local')) {
    fail(`startup log missing segment store line:\n${logs}`);
  }
  ok('startup logs announce local segment store');

  const client = new SunjetClient({ baseUrl: url });
  await client.createTable('smoke', [
    { name: 'msg', kind: 'utf8' },
    { name: 'n', kind: 'i64' },
  ]);
  ok('create_table');

  await client.insertRow('smoke', {
    msg: { type: 'utf8', value: 'hello-seg' },
    n: { type: 'i64', value: 42 },
  });
  await client.insertRow('smoke', {
    msg: { type: 'utf8', value: 'world-seg' },
    n: { type: 'i64', value: 7 },
  });
  ok('insert rows');

  await client.flush();
  ok('flush');

  const segs = readdirSync(tmp).filter((f) => f.endsWith('.vss'));
  if (segs.length < 1) fail(`no .vss after flush; dir=${readdirSync(tmp)}`);
  ok(`disk has segment(s): ${segs.join(',')}`);

  for (const must of ['wal.log', 'catalog.bin', 'manifest.bin']) {
    if (!existsSync(join(tmp, must))) fail(`missing local metadata ${must}`);
  }
  ok('wal/catalog/manifest present locally');

  await client.insertRow('smoke', {
    msg: { type: 'utf8', value: 'more' },
    n: { type: 'i64', value: 1 },
  });
  await client.flush();
  await client.compact();
  ok('compact');

  const after = readdirSync(tmp).filter((f) => f.endsWith('.vss'));
  if (after.length !== 1) {
    fail(`expected 1 segment after compact, got ${after.length}: ${after}`);
  }
  ok(`compact pruned to single segment: ${after[0]}`);

  const scan = await client.scanRows('smoke', { k: 20 });
  if ((scan.rows ?? []).length !== 3) {
    fail(`expected 3 rows after compact, got ${scan.rows?.length}`);
  }
  ok(`scan after compact: ${scan.rows.length} rows`);

  // Validate env rejects S3 without bucket
  const bad = spawn(LL_BIN, [], {
    env: {
      ...process.env,
      LL_DATA_DIR: join(tmp, 'bad'),
      LL_BIND: `127.0.0.1:${await freePort()}`,
      LL_SEGMENT_BACKEND: 's3',
      // deliberately no bucket
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let badOut = '';
  bad.stderr.on('data', (c) => {
    badOut += c.toString();
  });
  const badCode = await new Promise((resolve) => {
    bad.on('exit', (code) => resolve(code));
    setTimeout(() => {
      try {
        bad.kill();
      } catch {
        /* */
      }
      resolve(-1);
    }, 5000);
  });
  if (badCode === 0) fail('s3 without bucket should fail to start');
  ok(`s3 without bucket refused to start (exit ${badCode})`);

  console.log(`\n✓ All ${passed} ll-server segment-storage smoke checks passed.`);
  cleanup(0);
}

main().catch((e) => {
  console.error(e);
  cleanup(1);
});
