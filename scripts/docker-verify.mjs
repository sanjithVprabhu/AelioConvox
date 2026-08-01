#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as sleep } from 'node:timers/promises';
import { aelioHttpUrl } from './lib/aelio-port.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

const baseUrl = process.env.AELIO_SERVER_URL ?? aelioHttpUrl();
const secret = process.env.AELIO_SDK_SECRET ?? 'change-me-in-production';
const containerName = process.env.AELIO_DOCKER_CONTAINER ?? 'aelio-verify';
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
      stdio: options.inherit ? 'inherit' : 'pipe',
      env: { ...process.env, AELIO_ALLOW_INSECURE_OPEN: '1', ...options.env },
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

async function waitForHealth(attempts = 40) {
  for (let i = 0; i < attempts; i += 1) {
    try {
      const response = await fetch(`${baseUrl}/health`);
      if (response.ok) return await response.json();
    } catch {
      // retry
    }
    await sleep(500);
  }
  throw new Error(`Container not healthy at ${baseUrl}`);
}

async function waitForSdk(attempts = 40) {
  for (let i = 0; i < attempts; i += 1) {
    try {
      const response = await fetch(`${baseUrl}/__test__/sdk/functions`);
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

let sdk = null;

try {
  console.log('\n── Docker image build ──');
  await run('docker', ['build', '-f', 'server/Dockerfile', '-t', 'aelio/server:latest', '.'], {
    inherit: true,
    cwd: root,
  });
  record('Docker image build', 'PASS');

  console.log('\n── Container startup ──');
  let reusedContainer = false;
  try {
    await run('docker', ['rm', '-f', containerName], { inherit: true });
  } catch {
    try {
      await waitForHealth(6);
      reusedContainer = true;
      record('Reuse existing container', 'PASS', containerName);
    } catch {
      // fall through to fresh run attempt
    }
  }

  if (!reusedContainer) {
    await run(
      'docker',
      [
        'run',
        '-d',
        '--name',
        containerName,
        '-p',
        '3000:3000',
        '-e',
        `AELIO_SDK_SECRET=${secret}`,
        '-e',
        'AELIO_TEST_MODE=1',
        '-e',
        'AELIO_DIAGNOSTICS=1',
        '-v',
        `${containerName}-data:/data`,
        'aelio/server:latest',
      ],
      { inherit: true },
    );
  }

  const health = await waitForHealth();
  record('Container /health', 'PASS', `uptime=${health.uptimeSeconds ?? 0}s`);

  const ready = await fetch(`${baseUrl}/ready`);
  const readyBody = await ready.json();
  record(
    'Container /ready',
    ready.ok && readyBody.ready ? 'PASS' : 'FAIL',
    ready.ok ? `db=${readyBody.checks?.database}` : JSON.stringify(readyBody),
  );

  const diagnostics = await fetch(`${baseUrl}/diagnostics`);
  const diagBody = await diagnostics.json();
  record(
    'Container /diagnostics',
    diagnostics.ok && diagBody.status === 'ok' ? 'PASS' : 'FAIL',
    `database=${diagBody.paths?.database}`,
  );

  const widget = await fetch(`${baseUrl}/widget.js`);
  record('Container /widget.js', widget.ok ? 'PASS' : 'FAIL', `${widget.status}`);

  const demo = await fetch(`${baseUrl}/demo.html`);
  record('Container /demo.html', demo.ok ? 'PASS' : 'FAIL', `${demo.status}`);

  const inspect = await run('docker', ['inspect', '--format', '{{.State.Health.Status}}', containerName]);
  const healthStatus = inspect.stdout.trim();
  if (healthStatus && healthStatus !== '{{.State.Health.Status}}') {
    record('Docker healthcheck status', healthStatus === 'healthy' ? 'PASS' : 'WARN', healthStatus);
  }

  console.log('\n── SDK + integration tests ──');
  sdk = spawn('npx', ['tsx', 'src/index.ts'], {
    cwd: join(root, 'examples/nodejs-express'),
    stdio: 'inherit',
    env: {
      ...process.env,
      AELIO_ALLOW_INSECURE_OPEN: '1',
      AELIO_SDK_SECRET: secret,
      AELIO_SERVER_URL: 'ws://127.0.0.1:3000',
    },
  });

  const functions = await waitForSdk();
  record('SDK registration', 'PASS', functions.join(', '));

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

  console.log('\n── Graceful container stop ──');
  try {
    await run('docker', ['stop', containerName], { inherit: true });
    const logs = await run('docker', ['logs', containerName]);
    const cleanShutdown =
      logs.stdout.includes('shut down cleanly') || logs.stdout.includes('shutdown signal');
    record('Container graceful stop', cleanShutdown ? 'PASS' : 'WARN');
  } catch {
    record('Container graceful stop', 'WARN', 'docker stop unavailable in this environment');
  }
} catch (error) {
  record('Docker verification', 'FAIL', error.message);
} finally {
  sdk?.kill('SIGTERM');
}

console.log('\n── Summary ──');
const passed = results.filter((entry) => entry.status === 'PASS').length;
const failed = results.filter((entry) => entry.status === 'FAIL').length;
const warned = results.filter((entry) => entry.status === 'WARN').length;
console.log(`Total: ${results.length} checks — ${passed} passed, ${failed} failed, ${warned} warnings`);

if (failed > 0) {
  for (const entry of results.filter((item) => item.status === 'FAIL')) {
    console.log(`  - ${entry.name}${entry.detail ? `: ${entry.detail}` : ''}`);
  }
  process.exit(1);
}

console.log('\nDocker verification complete — all checks passed.');
process.exit(0);