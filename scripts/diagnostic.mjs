#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { setTimeout as sleep } from 'node:timers/promises';
import { join } from 'node:path';

const root = process.cwd();
const baseUrl = process.env.AELIO_SERVER_URL ?? 'http://127.0.0.1:3000';
const results = [];

function record(name, status, detail = '') {
  results.push({ name, status, detail });
  const icon = status === 'PASS' ? '✓' : status === 'WARN' ? '!' : '✗';
  const suffix = detail ? ` — ${detail}` : '';
  console.log(`${icon} ${name}${suffix}`);
}

function run(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd ?? root,
      stdio: options.inherit ? 'inherit' : 'pipe',
      env: { ...process.env, ...options.env },
    });
    let stdout = '';
    let stderr = '';
    if (!options.inherit) {
      child.stdout?.on('data', (chunk) => {
        stdout += chunk.toString();
      });
      child.stderr?.on('data', (chunk) => {
        stderr += chunk.toString();
      });
    }
    child.on('exit', (code) => {
      if (code === 0) resolve({ stdout, stderr });
      else reject(new Error(`${command} ${args.join(' ')} exited ${code}\n${stderr || stdout}`));
    });
  });
}

async function waitForHealth(url, attempts = 40) {
  for (let i = 0; i < attempts; i += 1) {
    try {
      const response = await fetch(`${url}/health`);
      if (response.ok) return await response.json();
    } catch {
      // retry
    }
    await sleep(500);
  }
  throw new Error(`Server not healthy at ${url}`);
}

async function waitForSdk(url, attempts = 40) {
  for (let i = 0; i < attempts; i += 1) {
    try {
      const response = await fetch(`${url}/__test__/sdk/functions`);
      if (response.ok) {
        const data = await response.json();
        const functions = data.functions ?? [];
        if (functions.includes('getOrderStatus') && functions.includes('cancelOrder')) {
          return functions;
        }
      }
    } catch {
      // retry
    }
    await sleep(500);
  }
  throw new Error('SDK functions not registered');
}

async function section(title) {
  console.log(`\n── ${title} ──`);
}

let server = null;
let sdk = null;

const serverEnv = {
  AELIO_CONFIG: join(root, 'config.yaml'),
  AELIO_SDK_SECRET: process.env.AELIO_SDK_SECRET ?? 'change-me-in-production',
  AELIO_TEST_MODE: '1',
  AELIO_DIAGNOSTICS: '1',
  AELIO_MIGRATIONS_PATH: join(root, 'packages/db/drizzle'),
  AELIO_PUBLIC_PATH: join(root, 'server/public'),
};

function spawnServer(stdio = 'pipe') {
  return spawn('node', ['dist/main.js'], {
    cwd: join(root, 'server'),
    stdio,
    env: { ...process.env, ...serverEnv },
  });
}

function spawnSdk(stdio = 'pipe') {
  return spawn('npx', ['tsx', 'src/index.ts'], {
    cwd: join(root, 'examples/nodejs-express'),
    stdio,
    env: {
      ...process.env,
      AELIO_SDK_SECRET: serverEnv.AELIO_SDK_SECRET,
      AELIO_SERVER_URL: 'ws://127.0.0.1:3000',
    },
  });
}

async function waitForPortFree(attempts = 20) {
  for (let i = 0; i < attempts; i += 1) {
    try {
      await fetch(`${baseUrl}/health`);
      await sleep(250);
    } catch {
      return;
    }
  }
}

try {
  await section('1. Prerequisites');
  const nodeVersion = process.version;
  if (Number(nodeVersion.slice(1).split('.')[0]) >= 20) {
    record('Node.js version', 'PASS', nodeVersion);
  } else {
    record('Node.js version', 'FAIL', `Need >=20, got ${nodeVersion}`);
  }

  if (existsSync(join(root, 'pnpm-lock.yaml'))) {
    record('pnpm lockfile', 'PASS');
  } else {
    record('pnpm lockfile', 'FAIL', 'pnpm-lock.yaml missing');
  }

  await section('2. Build');
  try {
    await run('pnpm', ['build'], { inherit: true });
    record('Monorepo build', 'PASS');
  } catch (error) {
    record('Monorepo build', 'FAIL', error.message.split('\n')[0]);
  }

  await section('3. Artifact checks');
  const artifacts = [
    'server/dist/main.js',
    'server/public/widget.js',
    'server/public/demo.html',
    'packages/core/dist/index.js',
    'packages/db/drizzle/meta/_journal.json',
    'packages/protocol/dist/index.js',
    'packages/llm/dist/index.js',
    'packages/channels/dist/index.js',
    'sdk/node/dist/index.js',
  ];
  for (const artifact of artifacts) {
    if (existsSync(join(root, artifact))) {
      record(`Artifact: ${artifact}`, 'PASS');
    } else {
      record(`Artifact: ${artifact}`, 'FAIL', 'missing');
    }
  }

  await section('4. Config');
  if (existsSync(join(root, 'config.yaml'))) {
    record('config.yaml present', 'PASS');
  } else {
    record('config.yaml present', 'FAIL');
  }

  await section('5. Runtime startup');
  server = spawnServer('pipe');

  const health = await waitForHealth(baseUrl);
  record('Server /health', 'PASS', `uptime=${health.uptimeSeconds ?? 0}s`);

  sdk = spawnSdk('pipe');

  const functions = await waitForSdk(baseUrl);
  record('SDK registration', 'PASS', functions.join(', '));

  const ready = await fetch(`${baseUrl}/ready`);
  const readyBody = await ready.json();
  if (ready.ok && readyBody.ready) {
    record('Server /ready', 'PASS');
  } else {
    record('Server /ready', 'FAIL', JSON.stringify(readyBody.checks ?? readyBody));
  }

  const diagnostics = await fetch(`${baseUrl}/diagnostics`);
  const diagBody = await diagnostics.json();
  if (diagnostics.ok && diagBody.status === 'ok') {
    record('Server /diagnostics', 'PASS', `llm=${diagBody.config?.llm}`);
  } else {
    record('Server /diagnostics', 'FAIL');
  }

  const widget = await fetch(`${baseUrl}/widget.js`);
  record('Widget bundle served', widget.ok ? 'PASS' : 'FAIL', `${widget.status}`);

  const demo = await fetch(`${baseUrl}/demo.html`);
  record('Demo page served', demo.ok ? 'PASS' : 'FAIL', `${demo.status}`);

  await section('6. Graceful shutdown');
  const shutdownExit = await new Promise((resolve) => {
    server.once('exit', (code) => resolve(code));
    server.kill('SIGTERM');
    globalThis.setTimeout(() => {
      if (!server.killed) server.kill('SIGKILL');
      resolve('timeout');
    }, 12_000);
  });
  server = null;

  if (shutdownExit === 0) {
    record('SIGTERM graceful shutdown', 'PASS');
  } else {
    record('SIGTERM graceful shutdown', 'WARN', `exit=${shutdownExit}`);
  }

  sdk?.kill('SIGTERM');
  sdk = null;
  await waitForPortFree();

  await section('7. Integration tests');
  server = spawnServer('inherit');

  await waitForHealth(baseUrl);
  sdk = spawnSdk('inherit');
  await waitForSdk(baseUrl);

  const phaseTests = [
    ['Phase 2 — Web widget', 'node', ['scripts/test-phase2-widget.mjs']],
    ['Phase 3 — WhatsApp', 'node', ['scripts/test-phase3-whatsapp.mjs']],
    ['Phase 4 — Confirmation', 'node', ['scripts/test-phase4-confirmation.mjs']],
    ['Phase 4 — Magic link', 'node', ['scripts/test-phase4-magic-link.mjs']],
    ['Phase 4 — WhatsApp identity', 'node', ['scripts/test-phase4-whatsapp-identity.mjs']],
    ['Phase 5 — Memory', 'npx', ['tsx', 'scripts/test-phase5-memory.mjs']],
  ];

  for (const [name, cmd, args] of phaseTests) {
    try {
      await run(cmd, args, { inherit: true, env: { AELIO_TEST_MODE: '1' } });
      record(name, 'PASS');
    } catch (error) {
      record(name, 'FAIL', error.message.split('\n')[0]);
    }
  }
} catch (error) {
  record('Diagnostic runner', 'FAIL', error.message);
} finally {
  server?.kill('SIGTERM');
  sdk?.kill('SIGTERM');
}

await section('Summary');
const passed = results.filter((entry) => entry.status === 'PASS').length;
const failed = results.filter((entry) => entry.status === 'FAIL').length;
const warned = results.filter((entry) => entry.status === 'WARN').length;

console.log(`\nTotal: ${results.length} checks — ${passed} passed, ${failed} failed, ${warned} warnings`);

if (failed > 0) {
  console.log('\nFailed checks:');
  for (const entry of results.filter((item) => item.status === 'FAIL')) {
    console.log(`  - ${entry.name}${entry.detail ? `: ${entry.detail}` : ''}`);
  }
  process.exit(1);
}

console.log('\nAelio diagnostic complete — all checks passed.');
process.exit(0);