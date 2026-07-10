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
import { createServer } from 'node:net';
import { existsSync, copyFileSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { DEFAULT_AELIO_PORT, resolveAelioPort } from './lib/aelio-port.mjs';

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
    !existsSync(join(root, 'server/public/widget.js')) ||
    !existsSync(join(root, 'sdk/node/dist/index.js'))
  );
}

function isPortFree(port) {
  return new Promise((resolve) => {
    const probe = createServer();
    probe.once('error', (error) => {
      if (error?.code === 'EADDRINUSE') {
        resolve(false);
        return;
      }
      fail(`cannot probe port ${port}: ${error?.message ?? error}`);
    });
    probe.once('listening', () => {
      probe.close(() => resolve(true));
    });
    probe.listen(port, '127.0.0.1');
  });
}

async function pickPort(preferred) {
  for (const port of preferred) {
    if (await isPortFree(port)) {
      return port;
    }
  }
  fail(`no free port in [${preferred.join(', ')}] — stop conflicting processes (e.g. docker stop aelio-sunjet-1)`);
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
} else {
  // Server and SDK import compiled workspace packages (e.g. @aelio/core → dist/).
  // Turbo rebuilds only what changed; without this, editing packages/*/src leaves
  // stale dist and the server crashes on missing exports.
  log('setup', 'building workspace packages (server + SDK deps)…');
  runSync('pnpm', [
    'turbo',
    'run',
    'build',
    '--filter=@aelio/server^...',
    '--filter=@aelio/sdk',
  ]);
}

// ── Start services ──────────────────────────────────────────────────────────

const serverPort = resolveAelioPort();
if (!(await isPortFree(serverPort))) {
  fail(
    `port ${serverPort} is in use — stop the other process (lsof -i :${serverPort}) or set AELIO_PORT`,
  );
}

const examplePort =
  process.env.PORT !== undefined
    ? Number(process.env.PORT)
    : await pickPort([8080, 8083, 8084, 8085, 8086, 8087, 8088, 8089, 8092, 8093, 9000, 9001]);

if (examplePort !== 8080) {
  log(
    'setup',
    `port 8080 busy (often leftover aelio-sunjet-1) — example SDK will use :${examplePort}`,
  );
}

const childEnv = {
  ...process.env,
  AELIO_CONFIG: resolvedConfig,
  AELIO_SDK_SECRET: secret,
  AELIO_PORT: String(serverPort),
  AELIO_SERVER_URL: process.env.AELIO_SERVER_URL ?? `ws://127.0.0.1:${serverPort}`,
  PORT: String(examplePort),
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
  const readyUrl = `http://127.0.0.1:${serverPort}/ready`;
  for (let i = 0; i < attempts; i += 1) {
    try {
      const response = await fetch(readyUrl);
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
// Use dev:once (no file watchers) — tsx watch hits ENOSPC when IDE/tsserver
// processes exhaust the kernel inotify limit on Linux.
run('server', 'pnpm', ['--filter', '@aelio/server', 'dev:once'], root);
await waitFor(`http://127.0.0.1:${serverPort}/health`);

log('boot', 'starting example SDK backend…');
run('sdk', 'pnpm', ['--filter', 'aelio-example-express', 'start'], root);
await waitForSdkReady();

console.log('\n────────────────────────────────────────────────────────');
console.log('  Aelio is running');
console.log(`  Chat demo:    http://localhost:${serverPort}/demo.html`);
console.log(`  Health:       http://localhost:${serverPort}/health`);
console.log(`  Ready:        http://localhost:${serverPort}/ready`);
console.log(`  Port:         ${serverPort} (set AELIO_PORT to change; default ${DEFAULT_AELIO_PORT})`);
console.log('  Config:       ' + resolvedConfig);
console.log('  LLM:          mock (edit config.yaml or .env for a real provider)');
console.log('  Stop:         Ctrl+C');
console.log('────────────────────────────────────────────────────────\n');
