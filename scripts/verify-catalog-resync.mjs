/**
 * Live check: SDK connect → mid-session expose() → expect catalog_update resync.
 *
 *   node scripts/verify-catalog-resync.mjs
 */
import { Aelio } from '../sdk/node/dist/index.js';

const secret = process.env.AELIO_SDK_SECRET ?? 'change-me-in-production';
const url = process.env.AELIO_SERVER_URL ?? 'ws://127.0.0.1:3010';
const timeoutMs = 15_000;

function waitFor(predicate, label) {
  return new Promise((resolve, reject) => {
    const started = Date.now();
    const timer = setInterval(() => {
      if (predicate()) {
        clearInterval(timer);
        resolve();
        return;
      }
      if (Date.now() - started > timeoutMs) {
        clearInterval(timer);
        reject(new Error(`timeout waiting for ${label}`));
      }
    }, 50);
  });
}

const aelio = new Aelio();
const events = [];

// Monkey-patch console.log to capture SDK banners / heartbeats from this process.
const origLog = console.log.bind(console);
console.log = (...args) => {
  const line = args.map(String).join(' ');
  events.push(line);
  origLog(...args);
};

aelio.application('catalog-resync-probe');
aelio.expose(
  'probe_ping',
  async () => ({ ok: true }),
  { description: 'Probe tool present at connect', params: {}, safety: 'read' },
);

origLog(`\n[verify] connecting to ${url} …`);
await aelio.listen({ secret, url });

await waitFor(
  () => events.some((line) => line.includes('successfully connected to Aelio')),
  'initial connect banner',
);
origLog('[verify] ✓ initial connect registered');

const beforeUpdate = events.length;
origLog('[verify] adding tool probe_after_connect via expose() …');
aelio.expose(
  'probe_after_connect',
  async () => ({ ok: true, phase: 'after' }),
  {
    description: 'Tool added after connect to force catalog resync',
    params: {},
    safety: 'read',
  },
);

await waitFor(
  () =>
    events.slice(beforeUpdate).some(
      (line) => line.includes('catalog updated on Aelio') && line.includes('probe_after_connect'),
    )
    || events.slice(beforeUpdate).some((line) => line.includes('+ tools: probe_after_connect')),
  'catalog_update with + tools: probe_after_connect',
);

const updateChunk = events.slice(beforeUpdate).join('\n');
if (!updateChunk.includes('probe_after_connect')) {
  throw new Error('catalog update banner did not mention probe_after_connect');
}

origLog('[verify] ✓ catalog resync observed (probe_after_connect)');
await aelio.disconnect();
origLog('[verify] PASS — connect + mid-session catalog sync both worked\n');
process.exit(0);
