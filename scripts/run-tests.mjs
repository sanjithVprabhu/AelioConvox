import { spawn } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';
import { join } from 'node:path';

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

async function waitForHealth(url, attempts = 40) {
  for (let i = 0; i < attempts; i += 1) {
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
const baseUrl = 'http://127.0.0.1:3000';

const server = spawn('npx', ['tsx', 'src/main.ts'], {
  cwd: join(root, 'apps/server'),
  stdio: 'inherit',
  env: {
    ...process.env,
    AELIO_CONFIG: join(root, 'config.yaml'),
    AELIO_SDK_SECRET: process.env.AELIO_SDK_SECRET ?? 'change-me-in-production',
    AELIO_TEST_MODE: '1',
  },
});

let sdk = null;

try {
  await waitForHealth(baseUrl);

  sdk = spawn('npx', ['tsx', 'src/index.ts'], {
    cwd: join(root, 'examples/nodejs-express'),
    stdio: 'inherit',
    env: {
      ...process.env,
      AELIO_SDK_SECRET: process.env.AELIO_SDK_SECRET ?? 'change-me-in-production',
      AELIO_SERVER_URL: 'ws://127.0.0.1:3000',
    },
  });

  await waitForSdk(baseUrl);

  for (const test of tests) {
    console.log(`\n=== Running ${test.name} ===`);
    const runner = test.script.includes('phase5') ? 'npx' : 'node';
    const args = test.script.includes('phase5') ? ['tsx', test.script] : [test.script];
    await run(runner, args, { AELIO_TEST_MODE: '1' });
  }

  console.log('\nAll phase tests passed.');
  process.exit(0);
} catch (error) {
  console.error('\nTest run failed:', error);
  process.exit(1);
} finally {
  server.kill('SIGTERM');
  sdk?.kill('SIGTERM');
}