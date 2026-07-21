import { spawn } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { setTimeout as sleep } from 'node:timers/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';

const tests = [
  { name: 'Phase 2 — Web widget', script: 'scripts/test-phase2-widget.mjs' },
  { name: 'Phase 3 — WhatsApp', script: 'scripts/test-phase3-whatsapp.mjs' },
  { name: 'Phase 4 — Write confirmation', script: 'scripts/test-phase4-confirmation.mjs' },
  { name: 'Phase 4 — Magic link auth', script: 'scripts/test-phase4-magic-link.mjs' },
  { name: 'Phase 4 — WhatsApp identity', script: 'scripts/test-phase4-whatsapp-identity.mjs' },
  { name: 'Phase 5 — Memory analyst', script: 'scripts/test-phase5-memory.mjs' },
];

function run(command, args, env = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      stdio: 'inherit',
      env: { ...process.env, ...env },
    });
    child.on('exit', (code) => {
      if (code === 0) resolve();
      else reject(new Error(`${command} ${args.join(' ')} exited with ${code}`));
    });
  });
}

async function getFreePort() {
  const server = createServer();
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const address = server.address();
  const port = typeof address === 'object' && address ? address.port : 0;
  await new Promise((resolve) => server.close(resolve));
  if (!port) {
    throw new Error('Unable to allocate a local test port');
  }
  return port;
}

async function waitForHealth(url, child, getLogs, attempts = 40) {
  for (let i = 0; i < attempts; i += 1) {
    if (child.exitCode !== null) {
      throw new Error(`Server exited before becoming healthy:\n${getLogs()}`);
    }
    try {
      const response = await fetch(`${url}/health`);
      if (response.ok) return;
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
          return;
        }
      }
    } catch {
      // retry
    }
    await sleep(500);
  }
  throw new Error('SDK did not register getOrderStatus and cancelOrder in time');
}

const root = process.cwd();

// Standalone suites: each spawns its own Sunjet (ll-server) and needs no Aelio
// server — run them first so engine regressions fail fast.
const standalone = [
  { name: 'Harness executor', script: 'scripts/test-harness-executor.mjs' },
  { name: 'Archetype valence engine', script: 'scripts/test-archetype-engine.mjs' },
  { name: 'Semantic pathway engine', script: 'scripts/test-semantic-pathway.mjs' },
  { name: 'Aspect discovery (self-learning)', script: 'scripts/test-aspect-discovery.mjs' },
  { name: 'Immediate context engine', script: 'scripts/test-immediate-context.mjs' },
  { name: 'Reply relevance + decision journal', script: 'scripts/test-turn-relevance.mjs' },
  { name: 'Admin DB + journal endpoint', script: 'scripts/test-admin-db.mjs' },
];
for (const test of standalone) {
  console.log(`\n=== Running ${test.name} ===`);
  await run('node', [test.script]);
}

const tmpRoot = mkdtempSync(join(tmpdir(), 'aelio-test-run-'));
const testPort = await getFreePort();
const baseUrl = `http://127.0.0.1:${testPort}`;
const wsUrl = `ws://127.0.0.1:${testPort}`;
const configPath = join(tmpRoot, 'config.yaml');

const configTemplate = readFileSync(join(root, 'config.yaml'), 'utf8');
const testConfig = configTemplate
  .replace(/port:\s*\d+$/m, `port: ${testPort}`)
  .replace(/provider:\s*\w+/m, 'provider: mock')
  .replace(/- http:\/\/localhost:\d+$/m, `- ${baseUrl}`);
writeFileSync(configPath, testConfig);

const server = spawn('npx', ['tsx', 'src/main.ts'], {
  cwd: join(root, 'server'),
  stdio: ['ignore', 'pipe', 'pipe'],
  env: {
    ...process.env,
    AELIO_CONFIG: configPath,
    AELIO_SDK_SECRET: process.env.AELIO_SDK_SECRET ?? 'change-me-in-production',
    AELIO_TEST_MODE: '1',
  },
});

let sdk = null;
let serverOutput = '';
server.stdout.on('data', (chunk) => {
  const text = chunk.toString();
  serverOutput += text;
  process.stdout.write(text);
});
server.stderr.on('data', (chunk) => {
  const text = chunk.toString();
  serverOutput += text;
  process.stderr.write(text);
});
const getServerLogs = () => serverOutput.slice(-8000);

try {
  // Server-independent unit tests run first (fast, no LLM/network).
  console.log('\n=== Running LLM provider-wiring tests ===');
  await run('node', [join(root, 'scripts/test-llm-providers.mjs')], {});

  console.log('\n=== Running harness executor unit tests ===');
  await run('node', [join(root, 'scripts/test-harness-executor.mjs')], {});

  console.log('\n=== Running turn relevance audit ===');
  await run('node', [join(root, 'scripts/test-turn-relevance.mjs')], {});

  await waitForHealth(baseUrl, server, getServerLogs);

  sdk = spawn('npx', ['tsx', 'src/index.ts'], {
    cwd: join(root, 'examples/nodejs-express'),
    stdio: 'inherit',
    env: {
      ...process.env,
      AELIO_SDK_SECRET: process.env.AELIO_SDK_SECRET ?? 'change-me-in-production',
      AELIO_SERVER_URL: wsUrl,
    },
  });

  await waitForSdk(baseUrl);

  for (const test of tests) {
    console.log(`\n=== Running ${test.name} ===`);
    const runner = test.script.includes('phase5') ? 'npx' : 'node';
    const args = test.script.includes('phase5') ? ['tsx', test.script] : [test.script];
    await run(runner, args, {
      AELIO_TEST_MODE: '1',
      AELIO_SERVER_URL: baseUrl,
      AELIO_WS_URL: wsUrl,
    });
  }

  console.log('\nAll phase tests passed.');
  process.exit(0);
} catch (error) {
  console.error('\nTest run failed:', error);
  process.exit(1);
} finally {
  server.kill('SIGTERM');
  sdk?.kill('SIGTERM');
  rmSync(tmpRoot, { recursive: true, force: true });
}
