/**
 * Local dev startup: install deps, build if needed, then run the Aelio server
 * and the example Express SDK backend.
 *
 *   node scripts/start.mjs
 *   pnpm start
 *
 * Uses config.yaml (mock LLM, no API key required). Copy .env.example → .env
 * to customize secrets and provider keys.
 */
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, copyFileSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

const root = process.cwd();

function log(step, message) {
  console.log(`[start] ${step} ${message}`);
}

function fail(message) {
  console.error(`[start] error: ${message}`);
  process.exit(1);
}

function runSync(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: 'inherit',
    ...options,
  });
  if (result.status !== 0) {
    fail(`${command} ${args.join(' ')} failed`);
  }
}

function loadEnvFile(path) {
  if (!existsSync(path)) return;
  for (const line of readFileSync(path, 'utf8').split('\n')) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith('#')) continue;
    const eq = trimmed.indexOf('=');
    if (eq === -1) continue;
    const key = trimmed.slice(0, eq).trim();
    const value = trimmed.slice(eq + 1).trim();
    if (!(key in process.env)) {
      process.env[key] = value;
    }
  }
}

function hasPnpm() {
  return spawnSync('pnpm', ['--version'], { stdio: 'ignore' }).status === 0;
}

function needsBuild() {
  return (
    !existsSync(join(root, 'apps/server/public/widget.js')) ||
    !existsSync(join(root, 'packages/sdk-node/dist/index.js'))
  );
}

// ── Prerequisites ───────────────────────────────────────────────────────────

const nodeMajor = Number(process.versions.node.split('.')[0]);
if (nodeMajor < 20) {
  fail(`Node.js >= 20 required (found ${process.versions.node})`);
}

if (!hasPnpm()) {
  fail('pnpm is required. Run: corepack enable && corepack prepare pnpm@9 --activate');
}

// ── Environment ─────────────────────────────────────────────────────────────

const envPath = join(root, '.env');
const envExamplePath = join(root, '.env.example');

if (!existsSync(envPath) && existsSync(envExamplePath)) {
  copyFileSync(envExamplePath, envPath);
  log('setup', 'created .env from .env.example');
}

loadEnvFile(envPath);

const secret = process.env.AELIO_SDK_SECRET ?? 'change-me-in-production';
const configPath = process.env.AELIO_CONFIG ?? join(root, 'config.yaml');
const resolvedConfig = configPath.startsWith('/') ? configPath : join(root, configPath);

// ── Install & build ─────────────────────────────────────────────────────────

if (!existsSync(join(root, 'node_modules'))) {
  log('setup', 'installing dependencies…');
  runSync('pnpm', ['install']);
}

if (needsBuild()) {
  log('setup', 'building packages and widget…');
  runSync('pnpm', ['build']);
}

// ── Start services ──────────────────────────────────────────────────────────

const childEnv = {
  ...process.env,
  AELIO_CONFIG: resolvedConfig,
  AELIO_SDK_SECRET: secret,
  AELIO_SERVER_URL: process.env.AELIO_SERVER_URL ?? 'ws://127.0.0.1:3000',
};

const procs = [];

function run(name, cmd, args, cwd) {
  const child = spawn(cmd, args, { cwd, env: childEnv, stdio: 'inherit' });
  procs.push(child);
  child.on('exit', (code) => {
    if (code !== 0 && code !== null) {
      console.log(`[${name}] exited (${code})`);
    }
  });
  return child;
}

async function waitFor(url, attempts = 60) {
  for (let i = 0; i < attempts; i += 1) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {
      // retry
    }
    await sleep(500);
  }
  throw new Error(`timed out waiting for ${url}`);
}

async function waitForSdkReady(attempts = 60) {
  for (let i = 0; i < attempts; i += 1) {
    try {
      const response = await fetch('http://127.0.0.1:3000/ready');
      if (response.ok) {
        const body = await response.json();
        const functions = body.sdk?.functions ?? [];
        if (body.sdk?.connected && functions.includes('getOrderStatus')) {
          return;
        }
      }
    } catch {
      // retry
    }
    await sleep(500);
  }
  throw new Error('timed out waiting for SDK to connect (check AELIO_SDK_SECRET matches in .env)');
}

const shutdown = () => {
  for (const proc of procs) proc.kill('SIGTERM');
  process.exit(0);
};

process.on('SIGINT', shutdown);
process.on('SIGTERM', shutdown);

log('boot', 'starting Aelio server…');
run('server', 'pnpm', ['--filter', '@aelio/server', 'dev'], root);
await waitFor('http://127.0.0.1:3000/health');

log('boot', 'starting example SDK backend…');
run('sdk', 'pnpm', ['--filter', 'aelio-example-express', 'start'], root);
await waitForSdkReady();

console.log('\n────────────────────────────────────────────────────────');
console.log('  Aelio is running');
console.log('  Chat demo:    http://localhost:3000/demo.html');
console.log('  Health:       http://localhost:3000/health');
console.log('  Ready:        http://localhost:3000/ready');
console.log('  Config:       ' + resolvedConfig);
console.log('  LLM:          mock (edit config.yaml or .env for a real provider)');
console.log('  Stop:         Ctrl+C');
console.log('────────────────────────────────────────────────────────\n');
