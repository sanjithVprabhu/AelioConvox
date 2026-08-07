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

/** Root node_modules can exist but be stale after branch checkout — workspace bins must be present. */
function needsInstall() {
  if (!existsSync(join(root, 'node_modules'))) return true;
  return !existsSync(join(root, 'node_modules/.bin/turbo'));
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
  fail(`no free port in [${preferred.join(', ')}] — stop conflicting processes (e.g. docker stop aelio-aelioDb-1)`);
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

if (needsInstall()) {
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

log('setup', 'building authoritative Rust runtime…');
runSync('cargo', [
  'build',
  '--manifest-path',
  join(root, 'aelio-os/Cargo.toml'),
  '-p',
  'aelio-server',
]);

// ── Start services ──────────────────────────────────────────────────────────

const serverPort = resolveAelioPort();
if (!(await isPortFree(serverPort))) {
  fail(
    `port ${serverPort} is in use — stop the other process (lsof -i :${serverPort}) or set AELIO_PORT`,
  );
}

const rustPort =
  process.env.AELIO_RUNTIME_PORT !== undefined
    ? Number(process.env.AELIO_RUNTIME_PORT)
    : await pickPort([8090, 8091, 8094, 8095, 8096, 8097, 8098, 8099]);
const rustUrl = `http://127.0.0.1:${rustPort}`;
const internalToken =
  process.env.AELIO_INTERNAL_TOKEN ?? 'aelio-local-internal-token-32-chars';
if (internalToken.length < 32) {
  fail('AELIO_INTERNAL_TOKEN must contain at least 32 characters');
}
const configText = readFileSync(resolvedConfig, 'utf8');
const tenantName =
  process.env.AELIO_TENANT_ID ??
  configText.match(/^name:\s*['"]?([^'"\s#]+)['"]?/m)?.[1] ??
  'aelio-local';

const examplePort =
  process.env.PORT !== undefined
    ? Number(process.env.PORT)
    : await pickPort([8080, 8083, 8084, 8085, 8086, 8087, 8088, 8089, 8092, 8093, 9000, 9001]);

if (examplePort !== 8080) {
  log(
    'setup',
    `port 8080 busy (often leftover aelio-aelioDb-1) — example SDK will use :${examplePort}`,
  );
}

const childEnv = {
  ...process.env,
  AELIO_CONFIG: resolvedConfig,
  AELIO_SDK_SECRET: secret,
  AELIO_PORT: String(serverPort),
  AELIO_RUNTIME_BIND: `127.0.0.1:${rustPort}`,
  AELIO_RUST_RUNTIME_URL: rustUrl,
  AELIO_RUNTIME_TOKENS: internalToken,
  AELIO_RUNTIME_TOKEN: internalToken,
  AELIO_EVENT_KEY_SECRET: internalToken,
  AELIO_HOST_URL: `http://127.0.0.1:${serverPort}`,
  AELIO_HOST_TOKEN: internalToken,
  AELIO_LLM_GATEWAY_TOKEN: internalToken,
  AELIO_LLM_GATEWAY_URL: `http://127.0.0.1:${serverPort}/internal/aelio/llm/complete`,
  AELIO_LLM_EMBED_URL: `http://127.0.0.1:${serverPort}/internal/aelio/llm/embed`,
  AELIO_TENANT_ID: tenantName,
  AELIO_DATA_DIR: process.env.AELIO_DATA_DIR ?? join(root, 'data/aelio-os'),
  AELIO_DB_URL: rustUrl,
  DB_API_KEY: process.env.DB_API_KEY ?? internalToken,
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

log('boot', 'starting authoritative Rust runtime and Aelio database…');
run(
  'rust',
  join(root, 'aelio-os/target/debug/aelio-server'),
  [],
  root,
);
await waitFor(`${rustUrl}/readyz`);

log('boot', 'starting TypeScript channel/model edge…');
// Use dev:once (no file watchers) — tsx watch hits ENOSPC when IDE/tsserver
// processes exhaust the kernel inotify limit on Linux.
run('server', 'pnpm', ['--filter', '@aelio/server', 'dev:once'], root);
await waitFor(`http://127.0.0.1:${serverPort}/health`);

const skipExampleSdk =
  process.env.AELIO_SKIP_EXAMPLE_SDK === '1' ||
  process.env.AELIO_SKIP_EXAMPLE_SDK === 'true';

if (skipExampleSdk) {
  log('boot', 'skipping bundled example SDK (AELIO_SKIP_EXAMPLE_SDK) — connect your own backend');
} else {
  log('boot', 'starting example SDK backend…');
  run('sdk', 'pnpm', ['--filter', 'aelio-example-express', 'start'], root);
  await waitForSdkReady();
}

console.log('\n────────────────────────────────────────────────────────');
console.log('  Aelio is running');
console.log(`  Chat demo:    http://localhost:${serverPort}/demo.html`);
console.log(`  Health:       http://localhost:${serverPort}/health`);
console.log(`  Ready:        http://localhost:${serverPort}/ready`);
console.log(`  Rust runtime: ${rustUrl}`);
console.log(`  Port:         ${serverPort} (set AELIO_PORT to change; default ${DEFAULT_AELIO_PORT})`);
console.log('  Config:       ' + resolvedConfig);
console.log('  LLM:          mock (edit config.yaml or .env for a real provider)');
if (skipExampleSdk) {
  console.log('  SDK:          (none — start your backend, e.g. aelio-test-3 ./start.sh)');
} else {
  console.log(`  Example SDK:  http://127.0.0.1:${examplePort} (ShopCo demo tools)`);
  console.log('  Tip:          testing aelio-test-3? restart with AELIO_SKIP_EXAMPLE_SDK=1');
}
console.log('  Stop:         Ctrl+C');
console.log('────────────────────────────────────────────────────────\n');
