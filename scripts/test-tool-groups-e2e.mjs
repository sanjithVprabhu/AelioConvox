/**
 * End-to-end: tool_groups expansion + ShopCo widget conversation.
 *
 * Boots a clean TS server + sample-saas (unless AELIO_E2E_EXTERNAL=1),
 * asserts registered states match expanded groups, runs a multi-turn chat,
 * and writes the transcript to conversation-logs/.
 *
 *   node scripts/test-tool-groups-e2e.mjs
 */
import { spawn } from 'node:child_process';
import { mkdirSync, writeFileSync, existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { createRequire } from 'node:module';
import WebSocket from 'ws';
import { expandToolAccess } from '../packages/protocol/src/index.ts';
import { filterFunctionsByState } from '../packages/core/src/index.ts';

const require = createRequire(import.meta.url);
const { parse: parseYaml } = require(join(process.cwd(), 'examples/sample-saas/node_modules/yaml'));
const root = process.cwd();
const PORT = Number(process.env.AELIO_PORT ?? 3010);
const HTTP = `http://127.0.0.1:${PORT}`;
const WS = `ws://127.0.0.1:${PORT}/widget/ws`;
const secret = process.env.AELIO_SDK_SECRET ?? 'change-me-in-production';
const external = process.env.AELIO_E2E_EXTERNAL === '1';
const customerId = `tool-groups-e2e-${Date.now()}`;

const logDir = join(root, 'conversation-logs');
mkdirSync(logDir, { recursive: true });
const logPath = join(logDir, `tool-groups-e2e-${new Date().toISOString().replace(/[:.]/g, '-')}.json`);

const transcript = {
  startedAt: new Date().toISOString(),
  customerId,
  port: PORT,
  checks: /** @type {Record<string, unknown>} */ ({}),
  turns: /** @type {Array<Record<string, unknown>>} */ ([]),
  verdict: 'PENDING',
  errors: /** @type {string[]} */ ([]),
};

function record(check, ok, detail) {
  transcript.checks[check] = { ok, detail };
  const mark = ok ? 'PASS' : 'FAIL';
  console.log(`[${mark}] ${check}${detail ? ` — ${typeof detail === 'string' ? detail : JSON.stringify(detail)}` : ''}`);
  if (!ok) transcript.errors.push(`${check}: ${typeof detail === 'string' ? detail : JSON.stringify(detail)}`);
}

function loadOpenaiKey() {
  let key = process.env.OPENAI_API_KEY ?? '';
  if (!key && existsSync(join(root, 'secrets.md'))) {
    const m = readFileSync(join(root, 'secrets.md'), 'utf8').match(/^GPT_KEY=(.+)$/m);
    if (m) key = m[1].trim();
  }
  if (!key && existsSync(join(root, '.env'))) {
    const m = readFileSync(join(root, '.env'), 'utf8').match(/^OPENAI_API_KEY=(.+)$/m);
    if (m) key = m[1].trim().replace(/^["']|["']$/g, '');
  }
  return key;
}

async function waitHttp(url, attempts = 90) {
  for (let i = 0; i < attempts; i++) {
    try {
      const r = await fetch(url);
      if (r.ok) return true;
    } catch {
      /* retry */
    }
    await sleep(500);
  }
  throw new Error(`timeout waiting for ${url}`);
}

function killPort(port) {
  try {
    const { execSync } = require('node:child_process');
    const out = execSync(`ss -ltnp 2>/dev/null | awk '/:${port}/ {print}' || true`, {
      encoding: 'utf8',
    });
    const pids = [...out.matchAll(/pid=(\d+)/g)].map((m) => m[1]);
    for (const pid of new Set(pids)) {
      try {
        process.kill(Number(pid), 'SIGTERM');
      } catch {
        /* ignore */
      }
    }
  } catch {
    /* ignore */
  }
}

const procs = [];
function run(name, cmd, args, env, cwd = root) {
  const child = spawn(cmd, args, {
    cwd,
    env: { ...process.env, ...env },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  procs.push({ name, child });
  child.stdout.on('data', (d) => process.stdout.write(`[${name}] ${d}`));
  child.stderr.on('data', (d) => process.stderr.write(`[${name}] ${d}`));
  child.on('exit', (code) => console.log(`[${name}] exited (${code})`));
  return child;
}

function shutdown() {
  for (const { child } of procs) {
    try {
      child.kill('SIGTERM');
    } catch {
      /* ignore */
    }
  }
}

function extractText(message) {
  if (typeof message.content === 'string' && message.content.trim()) return message.content;
  if (typeof message.prompt === 'string' && message.prompt.trim()) return message.prompt;
  if (typeof message.message === 'string') return message.message;
  return JSON.stringify(message);
}

function waitFor(socket, pred, ms, label) {
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => {
      socket.off('message', handler);
      reject(new Error(`timeout waiting for ${label}`));
    }, ms);
    const handler = (raw) => {
      let msg;
      try {
        msg = JSON.parse(String(raw));
      } catch {
        return;
      }
      if (pred(msg)) {
        clearTimeout(t);
        socket.off('message', handler);
        resolve(msg);
      }
    };
    socket.on('message', handler);
  });
}

async function waitForShopCoCatalog(attempts = 60) {
  for (let i = 0; i < attempts; i++) {
    try {
      const res = await fetch(`${HTTP}/__test__/sdk/catalog`);
      if (res.ok) {
        const catalog = await res.json();
        const names = (catalog.functions ?? []).map((f) => f.name);
        if (names.includes('syncProfile') && names.includes('listOrders') && names.includes('upgradePlan')) {
          const stateIds = (catalog.states ?? []).map((s) => s.id);
          if (stateIds.includes('onboarding') && stateIds.includes('churn_risk')) {
            return catalog;
          }
        }
      }
    } catch {
      /* retry */
    }
    await sleep(500);
  }
  throw new Error('timed out waiting for ShopCo SDK catalog (tool groups stages)');
}

function isAssistantTerminal(m) {
  return (m.type === 'message' && m.role === 'assistant') || m.type === 'error' || m.type === 'confirmation';
}

async function chatTurn(socket, content, id) {
  const started = Date.now();
  socket.send(JSON.stringify({ type: 'message', content, id }));
  /** Collect UI prompts but keep waiting for the assistant message. */
  const sideEvents = [];
  let reply = await new Promise((resolve, reject) => {
    const t = setTimeout(() => {
      socket.off('message', handler);
      reject(new Error(`timeout waiting for ${id}`));
    }, 120_000);
    const handler = (raw) => {
      let msg;
      try {
        msg = JSON.parse(String(raw));
      } catch {
        return;
      }
      if (msg.type === 'ui' || msg.type === 'typing') {
        sideEvents.push(msg);
        return;
      }
      if (isAssistantTerminal(msg)) {
        clearTimeout(t);
        socket.off('message', handler);
        resolve(msg);
      }
    };
    socket.on('message', handler);
  });
  const events = [...sideEvents, reply];
  let guard = 0;
  while (reply.type === 'confirmation' && guard < 3) {
    guard += 1;
    socket.send(JSON.stringify({ type: 'message', content: 'yes', id: `${id}-yes-${guard}` }));
    reply = await waitFor(socket, isAssistantTerminal, 120_000, `${id}-confirm`);
    events.push(reply);
  }
  const text = extractText(reply);
  const entry = {
    id,
    user: content,
    assistant: text,
    replyType: reply.type,
    durationMs: Date.now() - started,
    uiEvents: sideEvents.filter((e) => e.type === 'ui'),
    events: events.map((e) => ({
      type: e.type,
      role: e.role,
      content: extractText(e).slice(0, 2000),
      component: e.component,
      attribute_id: e.attribute_id,
    })),
  };
  transcript.turns.push(entry);
  console.log(`\n── turn ${id} (${entry.durationMs}ms) ──`);
  console.log(`USER: ${content}`);
  if (sideEvents.some((e) => e.type === 'ui')) {
    console.log(
      `UI: ${sideEvents
        .filter((e) => e.type === 'ui')
        .map((e) => `${e.attribute_id || e.component}`)
        .join(', ')}`,
    );
  }
  console.log(`ASSISTANT (${reply.type}): ${text.slice(0, 500)}${text.length > 500 ? '…' : ''}`);
  return entry;
}

function assertYamlGroupsExpandLocally() {
  const yamlPath = join(root, 'examples/sample-saas/manifests/shopco.pipeline.yaml');
  const raw = parseYaml(readFileSync(yamlPath, 'utf8'));
  const groups = Object.fromEntries(
    Object.entries(raw.tool_groups ?? {}).map(([id, g]) => [
      id,
      {
        ...(g.tools ? { tools: g.tools } : {}),
        ...(g.intents ? { intents: g.intents } : {}),
        ...(g.safety ? { safety: g.safety } : {}),
      },
    ]),
  );
  const onboarding = expandToolAccess(groups, {
    allowedGroups: raw.pipeline.stages.onboarding.allowed_groups,
    blockedGroups: raw.pipeline.stages.onboarding.blocked_groups,
  });
  const churn = expandToolAccess(groups, {
    blockedGroups: raw.pipeline.stages.churn_risk.blocked_groups,
  });
  const ok =
    JSON.stringify(onboarding.allowedIntents?.sort()) ===
      JSON.stringify(['billing', 'onboarding', 'order_inquiry', 'subscription'].sort()) &&
    JSON.stringify(onboarding.blockedSafety?.sort()) === JSON.stringify(['destructive', 'write'].sort()) &&
    JSON.stringify(churn.blockedTools) === JSON.stringify(['upgradePlan']);
  record('yaml_local_expand', ok, { onboarding, churn });
  return { groups, onboarding, churn, raw };
}

function assertLiveCatalog(catalog, expectedOnboarding, expectedChurn) {
  const byId = Object.fromEntries((catalog.states ?? []).map((s) => [s.id, s]));
  const onboarding = byId.onboarding;
  const churn = byId.churn_risk;
  const unverified = byId.unverified;

  const hasShopCoTools = (catalog.functions ?? []).some((f) => f.name === 'listOrders');
  record('sdk_connected_shopco', hasShopCoTools, {
    functions: (catalog.functions ?? []).map((f) => f.name),
    stateIds: (catalog.states ?? []).map((s) => s.id),
  });

  const onboardingOk =
    onboarding &&
    JSON.stringify([...(onboarding.allowedIntents ?? [])].sort()) ===
      JSON.stringify([...(expectedOnboarding.allowedIntents ?? [])].sort()) &&
    JSON.stringify([...(onboarding.blockedSafety ?? [])].sort()) ===
      JSON.stringify([...(expectedOnboarding.blockedSafety ?? [])].sort());
  record('live_state_onboarding_expanded', !!onboardingOk, onboarding);

  const churnOk =
    churn &&
    JSON.stringify([...(churn.blockedTools ?? [])].sort()) ===
      JSON.stringify([...(expectedChurn.blockedTools ?? [])].sort());
  record('live_state_churn_expanded', !!churnOk, churn);

  const unverifiedOk =
    unverified && JSON.stringify(unverified.allowedIntents ?? []) === JSON.stringify(['public']);
  record('live_state_unverified_expanded', !!unverifiedOk, unverified);

  // Enforcement: cancelOrder (write) blocked in onboarding; listOrders allowed.
  const functions = catalog.functions ?? [];
  if (onboarding && functions.length) {
    const allowed = filterFunctionsByState(functions, onboarding).map((f) => f.name);
    record(
      'filter_onboarding_allows_listOrders',
      allowed.includes('listOrders'),
      { allowed },
    );
    record(
      'filter_onboarding_blocks_cancelOrder',
      !allowed.includes('cancelOrder'),
      { allowed },
    );
    record(
      'filter_onboarding_blocks_upgradePlan',
      !allowed.includes('upgradePlan'),
      { allowed },
    );
  }

  if (churn && functions.length) {
    const allowed = filterFunctionsByState(functions, churn).map((f) => f.name);
    record('filter_churn_blocks_upgradePlan', !allowed.includes('upgradePlan'), { allowed });
  }
}

async function main() {
  console.log(`\n=== tool-groups E2E → ${logPath} ===\n`);

  const { onboarding: expectedOnboarding, churn: expectedChurn } = assertYamlGroupsExpandLocally();

  if (!external) {
    console.log('Stopping anything on', PORT, '…');
    for (const p of [PORT, 8081, 4173, 8090]) killPort(p);
    try {
      const { execSync } = require('node:child_process');
      execSync(
        "pkill -f 'AelioTest/server.mjs' 2>/dev/null; pkill -f 'aelio-test-2/server.mjs' 2>/dev/null; true",
        { stdio: 'ignore' },
      );
    } catch {
      /* ignore */
    }
    await sleep(2000);

    const openaiKey = loadOpenaiKey();
    const useMock = process.env.AELIO_LLM === 'mock' || !openaiKey;
    const config = useMock ? 'config.yaml' : 'config.openai.yaml';
    console.log(useMock ? 'Using MOCK LLM' : 'Using OpenAI');

    const env = {
      AELIO_CONFIG: join(root, config),
      AELIO_SDK_SECRET: secret,
      AELIO_PORT: String(PORT),
      AELIO_SERVER_URL: `ws://127.0.0.1:${PORT}`,
      AELIO_TEST_MODE: '1',
      OPENAI_API_KEY: openaiKey,
      PORT: '8081',
    };
    // Drop legacy agent_loop flags so this run stays on the TS harness/pipeline path.
    delete env.AELIO_HARNESS_MODE;
    delete process.env.AELIO_HARNESS_MODE;
    delete process.env.AELIO_SKIP_EXAMPLE_SDK;

    run('server', 'pnpm', ['exec', 'tsx', 'src/main.ts'], env, join(root, 'server'));
    await waitHttp(`${HTTP}/health`);
    run(
      'shopco',
      'pnpm',
      ['start'],
      {
        ...env,
        AELIO_SERVER_URL: `ws://127.0.0.1:${PORT}`,
      },
      join(root, 'examples/sample-saas'),
    );
  }

  let catalog;
  try {
    catalog = await waitForShopCoCatalog();
    record('fetch_catalog', true, {
      functionCount: catalog.functions?.length,
      stateCount: catalog.states?.length,
    });
    assertLiveCatalog(catalog, expectedOnboarding, expectedChurn);
    transcript.checks.catalog_snapshot = {
      functions: (catalog.functions ?? []).map((f) => ({
        name: f.name,
        intent: f.intent,
        safety: f.safety,
      })),
      states: catalog.states,
    };
  } catch (err) {
    record('fetch_catalog', false, String(err));
  }
  // Conversation through unverified → verified → onboarding
  const socket = new WebSocket(WS);
  await new Promise((res, rej) => {
    socket.once('open', res);
    socket.once('error', rej);
  });
  socket.send(JSON.stringify({ type: 'init', customerId, email: `${customerId}@example.com` }));
  const ready = await waitFor(socket, (m) => m.type === 'ready' || m.type === 'error', 20_000, 'init');
  if (ready.type === 'error') {
    record('widget_init', false, ready);
    throw new Error(JSON.stringify(ready));
  }
  record('widget_init', true, { customerId });

  await chatTurn(socket, "Hi! I'd like to get started with ShopCo.", 't1-greeting');
  await chatTurn(socket, "Let's go", 't2-cta');
  await chatTurn(socket, `${customerId}@example.com`, 't3-email');
  await chatTurn(socket, 'Alex', 't4-name');
  await chatTurn(socket, 'Personal', 't5-preference');
  const orders = await chatTurn(socket, 'Show me my orders please.', 't6-list-orders');
  const cancelAttempt = await chatTurn(
    socket,
    'Please cancel order A-1002 right now using the cancel tool.',
    't7-cancel-blocked',
  );
  const upgradeAttempt = await chatTurn(
    socket,
    'Upgrade my plan to pro using upgradePlan.',
    't8-upgrade-blocked',
  );

  const ordersText = orders.assistant.toLowerCase();
  const sawOrders =
    ordersText.includes('a-100') ||
    ordersText.includes('order') ||
    ordersText.includes('headphones') ||
    ordersText.includes('charger');
  record('conversation_list_orders_ok', sawOrders, orders.assistant.slice(0, 400));

  // Soft checks: model should not claim a successful cancel/upgrade while in onboarding.
  const cancelText = cancelAttempt.assistant.toLowerCase();
  const cancelClaimedSuccess =
    /\bcancelled\b|\bcanceled\b/.test(cancelText) && !/can'?t|cannot|unable|not (yet|able)|once you|after/.test(cancelText);
  record(
    'conversation_cancel_not_executed',
    !cancelClaimedSuccess,
    cancelAttempt.assistant.slice(0, 400),
  );

  const upgradeText = upgradeAttempt.assistant.toLowerCase();
  const upgradeClaimedSuccess =
    /\bupgraded\b|\bpro plan\b.*\bactive\b/.test(upgradeText) &&
    !/can'?t|cannot|unable|not (yet|able)|once you|after|onboarding/.test(upgradeText);
  record(
    'conversation_upgrade_not_executed',
    !upgradeClaimedSuccess,
    upgradeAttempt.assistant.slice(0, 400),
  );

  socket.close();

  const hardFails = [
    'yaml_local_expand',
    'sdk_connected_shopco',
    'live_state_onboarding_expanded',
    'live_state_churn_expanded',
    'live_state_unverified_expanded',
    'filter_onboarding_allows_listOrders',
    'filter_onboarding_blocks_cancelOrder',
    'filter_onboarding_blocks_upgradePlan',
    'filter_churn_blocks_upgradePlan',
    'widget_init',
    'conversation_list_orders_ok',
  ].filter((k) => transcript.checks[k] && transcript.checks[k].ok === false);

  const softFails = [
    'conversation_cancel_not_executed',
    'conversation_upgrade_not_executed',
  ].filter((k) => transcript.checks[k] && transcript.checks[k].ok === false);

  transcript.finishedAt = new Date().toISOString();
  transcript.verdict = hardFails.length === 0 ? (softFails.length ? 'PASS_WITH_SOFT_WARNINGS' : 'PASS') : 'FAIL';
  transcript.hardFails = hardFails;
  transcript.softFails = softFails;

  writeFileSync(logPath, JSON.stringify(transcript, null, 2));
  // Also write a stable latest pointer
  writeFileSync(join(logDir, 'tool-groups-e2e-latest.json'), JSON.stringify(transcript, null, 2));

  console.log(`\n=== verdict: ${transcript.verdict} ===`);
  console.log(`Log: ${logPath}`);
  if (hardFails.length) console.log('Hard fails:', hardFails);
  if (softFails.length) console.log('Soft warnings:', softFails);

  if (!external) shutdown();
  process.exit(hardFails.length ? 1 : 0);
}

main().catch((err) => {
  transcript.verdict = 'ERROR';
  transcript.errors.push(String(err?.stack ?? err));
  transcript.finishedAt = new Date().toISOString();
  try {
    writeFileSync(logPath, JSON.stringify(transcript, null, 2));
    writeFileSync(join(logDir, 'tool-groups-e2e-latest.json'), JSON.stringify(transcript, null, 2));
  } catch {
    /* ignore */
  }
  console.error(err);
  shutdown();
  process.exit(1);
});

process.on('SIGINT', () => {
  shutdown();
  process.exit(130);
});
