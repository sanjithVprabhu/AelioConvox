/**
 * One-command live demo: boots the Aelio server (OpenAI by default) + the ShopCo
 * sample backend, then keeps both running so you can chat at the widget demo page.
 *
 *   node scripts/demo.mjs
 *
 * Reads OPENAI_API_KEY from the environment, or GPT_KEY from secrets.md if present.
 * Set AELIO_LLM=mock to run fully offline (only order/cancel intents work with mock).
 */
import { spawn } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';
import { readFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';

const root = process.cwd();
const secret = process.env.AELIO_SDK_SECRET ?? 'change-me-in-production';

// Resolve an OpenAI key from env or secrets.md (GPT_KEY=...), unless mock is forced.
let openaiKey = process.env.OPENAI_API_KEY ?? '';
if (!openaiKey && existsSync(join(root, 'secrets.md'))) {
  const m = readFileSync(join(root, 'secrets.md'), 'utf8').match(/^GPT_KEY=(.+)$/m);
  if (m) openaiKey = m[1].trim();
}
// Verify the key actually works (it may be revoked) — fall back to mock if not.
async function keyWorks(key) {
  if (!key) return false;
  try {
    const r = await fetch('https://api.openai.com/v1/chat/completions', {
      method: 'POST',
      headers: { 'content-type': 'application/json', authorization: `Bearer ${key}` },
      body: JSON.stringify({ model: 'gpt-4o-mini', max_tokens: 5, messages: [{ role: 'user', content: 'hi' }] }),
    });
    return r.ok;
  } catch {
    return false;
  }
}

const forceMock = process.env.AELIO_LLM === 'mock';
const useMock = forceMock || !(await keyWorks(openaiKey));
const config = useMock ? 'config.yaml' : 'config.openai.yaml';

if (useMock) {
  console.log('No working OpenAI key — running with the MOCK LLM (try "what is my order status?").');
  console.log('For full tool selection across all 6 functions, put a valid key in secrets.md (GPT_KEY=...) or set OPENAI_API_KEY.\n');
} else {
  console.log('Using OpenAI (gpt-4o-mini) for real tool selection.\n');
}

const childEnv = {
  ...process.env,
  AELIO_CONFIG: join(root, config),
  AELIO_SDK_SECRET: secret,
  OPENAI_API_KEY: openaiKey,
  AELIO_TEST_MODE: '1',
};

const procs = [];
function run(name, cmd, args, cwd) {
  const child = spawn(cmd, args, { cwd, env: childEnv, stdio: 'inherit' });
  procs.push(child);
  child.on('exit', (code) => console.log(`[${name}] exited (${code})`));
  return child;
}

async function waitFor(url, attempts = 60) {
  for (let i = 0; i < attempts; i++) {
    try { if ((await fetch(url)).ok) return true; } catch {}
    await sleep(500);
  }
  throw new Error(`timed out waiting for ${url}`);
}

const shutdown = () => { for (const p of procs) p.kill('SIGTERM'); process.exit(0); };
process.on('SIGINT', shutdown);
process.on('SIGTERM', shutdown);

run('server', 'npx', ['tsx', 'src/main.ts'], join(root, 'apps/server'));
await waitFor('http://127.0.0.1:3000/health');
run('shopco', 'npx', ['tsx', 'src/index.ts'], join(root, 'examples/sample-saas'));
await waitFor('http://127.0.0.1:3000/__test__/sdk/functions');

console.log('\n────────────────────────────────────────────────────────');
console.log('  ✅ AelioConvox is live');
console.log('  💬 Open the chat demo:  http://localhost:3000/demo.html');
console.log('  (Ctrl+C to stop both processes)');
console.log('────────────────────────────────────────────────────────\n');
