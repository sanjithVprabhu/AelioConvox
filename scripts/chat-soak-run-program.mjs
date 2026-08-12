/**
 * aelio-test-2 ONLY: live widget conversation for run_program write → store → reuse.
 * Uses local demo_* tools (no main-server / no harness stub).
 *
 *   node scripts/chat-soak-run-program.mjs
 */
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import WebSocket from 'ws';

const serverHttp = (process.env.AELIO_CHAT_HTTP || 'http://127.0.0.1:3010').replace(/\/$/, '');
const serverWs = (process.env.AELIO_CHAT_WS || serverHttp.replace(/^http/, 'ws')).replace(/\/$/, '');
const test2Http = (process.env.AELIO_TEST2_HTTP || 'http://127.0.0.1:4174').replace(/\/$/, '');
const customerId = process.env.AELIO_CHAT_CUSTOMER || `soak-program-t2-${Date.now()}`;
const phone = process.env.AELIO_CHAT_PHONE || '9876543210';
const turnTimeoutMs = Number(process.env.AELIO_CHAT_TURN_TIMEOUT_MS || 180_000);
const turnGapMs = Number(process.env.AELIO_CHAT_TURN_GAP_MS || 3000);
const runId = new Date().toISOString().replace(/[:.]/g, '-');
const outDir = join(process.cwd(), 'docs', 'chat-runs', runId);

const PROGRAM_SOURCE = JSON.stringify({
  version: 1,
  steps: [
    { tool: 'demo_account_snapshot', arguments: {} },
    { tool: 'demo_company_snapshot', arguments: {} },
  ],
});

const CASES = [
  {
    id: 1,
    label: 'write-run-store',
    requireProgramId: false,
    text:
      'Do not log in. Do not call send_login_otp, verify_login_otp, write_todos, update_todos, or spawn_task. ' +
      'Call kernel tool run_program exactly once. ' +
      'arguments.source must be the string ' +
      JSON.stringify(PROGRAM_SOURCE) +
      ' and arguments.rationale must be "aelio-test-2 batch demo snapshots". ' +
      'After the Observation, call finish alone. Prefer including Observation program_id and source_hash in the finish message.',
  },
  {
    id: 2,
    label: 'reuse-program',
    requireProgramId: true,
    text:
      'Do not write_todos. Call kernel tool run_program once more with the identical source string and rationale "reuse aelio-test-2 demo batch". ' +
      'Finish quoting both source_hash values and whether they match. Include program_id verbatim.',
  },
  {
    id: 3,
    label: 'close',
    requireProgramId: false,
    text:
      'In two sentences, summarize write → store → reuse on aelio-test-2. Repeat the program_id if you have it. Do not call tools.',
  },
];

function slug(label) {
  return label.replace(/[^a-z0-9]+/gi, '-').toLowerCase();
}

function ensureDir(path) {
  mkdirSync(path, { recursive: true });
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function extractText(message) {
  if (typeof message.content === 'string' && message.content.trim()) return message.content;
  if (typeof message.prompt === 'string' && message.prompt.trim()) return message.prompt;
  const blocks = message.frame?.blocks;
  if (Array.isArray(blocks)) {
    const texts = [];
    for (const block of blocks) {
      if (typeof block?.text === 'string') texts.push(block.text);
      if (typeof block?.fallback === 'string') texts.push(block.fallback);
      if (typeof block?.props?.text === 'string') texts.push(block.props.text);
      if (typeof block?.props?.markdown === 'string') texts.push(block.props.markdown);
    }
    if (texts.length) return texts.join('\n');
  }
  return '';
}

function isActionConfirmation(message) {
  if (message.type !== 'confirmation') return false;
  const text = extractText(message);
  return /confirm this exact action|reply yes to (approve|confirm)|yes to approve/i.test(text);
}

function confirmationDecision(text) {
  if (/verify login otp|send login otp|verify_login_otp|send_login_otp/i.test(text)) {
    return 'no';
  }
  if (/run_program|run program|demo_account_snapshot|demo_company_snapshot/i.test(text)) {
    return 'yes';
  }
  return 'no';
}

function isTerminalReply(message) {
  return (
    (message.type === 'message' && message.role === 'assistant') ||
    message.type === 'error' ||
    message.type === 'confirmation' ||
    (message.type === 'message' && message.role === 'system' && /error/i.test(message.content || ''))
  );
}

function isBadAssistant(text) {
  return /internal error|gateway unavailable|timed out|something went wrong|already being processed|safe execution limits|wasn'?t able to finish/i.test(
    text,
  );
}

function waitFor(socket, predicate, timeoutMs, label) {
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      socket.off('message', handler);
      reject(new Error(`Timed out waiting for ${label} (${timeoutMs}ms)`));
    }, timeoutMs);
    const handler = (raw) => {
      let message;
      try {
        message = JSON.parse(raw.toString());
      } catch {
        return;
      }
      if (predicate(message)) {
        clearTimeout(timeout);
        socket.off('message', handler);
        resolve(message);
      }
    };
    socket.on('message', handler);
  });
}

async function waitUntilIdle(socket, inbox, inboxStart, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const recent = inbox.slice(inboxStart);
    const lastTyping = [...recent].reverse().find((item) => item.message?.type === 'typing');
    if (!lastTyping || lastTyping.message.active === false) {
      await sleep(400);
      const after = inbox.slice(inboxStart);
      const newerTyping = after
        .slice(recent.length)
        .some((item) => item.message?.type === 'typing' && item.message.active);
      if (!newerTyping) return;
    }
    await sleep(200);
  }
}

function mdEscape(text) {
  return String(text ?? '').replace(/\r\n/g, '\n');
}

function writeTurnMd(turn) {
  const path = join(outDir, `turn-${String(turn.id).padStart(2, '0')}-${slug(turn.label)}.md`);
  const body = [
    `# Turn ${turn.id}: ${turn.label}`,
    '',
    `- **run:** \`${runId}\``,
    `- **scenario:** aelio-test-2 only — run_program write → store → reuse`,
    `- **customerId:** \`${customerId}\``,
    `- **sessionId:** \`${turn.sessionId || ''}\``,
    `- **status:** ${turn.ok ? 'OK' : 'ERROR'}`,
    `- **elapsed_ms:** ${turn.elapsedMs}`,
    turn.error ? `- **error:** ${turn.error}` : '',
    '',
    '## User',
    '',
    '```text',
    mdEscape(turn.user),
    '```',
    '',
    '## Assistant',
    '',
    '```text',
    mdEscape(turn.assistant || '(no assistant reply)'),
    '```',
    '',
    '## Wire events (summary)',
    '',
    '```json',
    JSON.stringify(turn.events, null, 2),
    '```',
    '',
  ]
    .filter((line) => line !== '')
    .join('\n');
  writeFileSync(path, body);
  return path;
}

async function preflight() {
  const health = await fetch(`${serverHttp}/health`);
  if (!health.ok) throw new Error(`Convox health ${health.status}`);
  const test2 = await fetch(`${test2Http}/`);
  if (!test2.ok) throw new Error(`aelio-test-2 not reachable at ${test2Http}`);
  const ready = await fetch(`${serverHttp}/ready`);
  const body = await ready.json();
  const sdk = body.sdk || {};
  if (!sdk.connected) throw new Error('SDK not connected — start aelio-test-2 only');
  const names = sdk.functions || [];
  for (const required of ['demo_account_snapshot', 'demo_company_snapshot']) {
    if (!names.includes(required)) {
      throw new Error(`aelio-test-2 catalog missing ${required}; restart aelio-test-2`);
    }
  }
  const app = String(sdk.application || sdk.app || '');
  if (app && !/aelio-test-2/i.test(app)) {
    throw new Error(`expected aelio-test-2 SDK only, got application=${app}`);
  }
  return body;
}

async function runOneTurn(socket, inbox, sessionId, testCase) {
  const started = Date.now();
  const events = [];
  let assistant = '';
  let error = null;
  let ok = false;
  const inboxStart = inbox.length;

  console.log(`\n[turn ${testCase.id}] ${testCase.label}`);
  try {
    socket.send(
      JSON.stringify({
        type: 'message',
        content: testCase.text,
        id: `t2prog-${testCase.id}-${Date.now()}`,
      }),
    );

    let reply = await waitFor(socket, isTerminalReply, turnTimeoutMs, `reply for turn ${testCase.id}`);
    const confirmations = [];
    let guard = 0;
    while (isActionConfirmation(reply) && guard < 6) {
      guard += 1;
      const prompt = extractText(reply);
      const decision = confirmationDecision(prompt);
      confirmations.push(`${decision.toUpperCase()} → ${prompt}`);
      console.log(`[turn ${testCase.id}] confirmation — ${decision}`);
      await sleep(400);
      socket.send(
        JSON.stringify({
          type: 'message',
          content: decision,
          id: `t2prog-${testCase.id}-confirm-${guard}`,
        }),
      );
      reply = await waitFor(
        socket,
        isTerminalReply,
        turnTimeoutMs,
        `post-confirm reply for turn ${testCase.id}`,
      );
    }

    await waitUntilIdle(socket, inbox, inboxStart);

    for (const item of inbox.slice(inboxStart)) {
      const m = item.message;
      events.push({
        type: m.type,
        role: m.role,
        code: m.code,
        active: m.active,
        contentPreview: extractText(m).slice(0, 320),
      });
    }

    if (reply.type === 'error') {
      error = `${reply.code || 'error'}: ${reply.message || JSON.stringify(reply)}`;
      assistant = reply.message || '';
      ok = false;
    } else {
      assistant = extractText(reply);
      if (confirmations.length) {
        assistant = `[confirmation] ${confirmations.join(' | ')}\n\n[after decide] ${assistant}`;
      }
      if (!assistant.trim()) {
        error = 'empty assistant reply';
        ok = false;
      } else if (isBadAssistant(assistant)) {
        error = assistant.slice(0, 200);
        ok = false;
      } else {
        ok = true;
        if (testCase.requireProgramId && !/prog-[a-f0-9]{8,}/i.test(assistant)) {
          error = 'missing program_id (prog-…) in assistant reply';
          ok = false;
        }
      }
    }
  } catch (err) {
    error = err instanceof Error ? err.message : String(err);
    ok = false;
    for (const item of inbox.slice(-40)) {
      const m = item.message;
      events.push({
        type: m.type,
        role: m.role,
        code: m.code,
        contentPreview: extractText(m).slice(0, 320),
      });
    }
  }

  return {
    id: testCase.id,
    label: testCase.label,
    user: testCase.text,
    assistant,
    ok,
    error,
    elapsedMs: Date.now() - started,
    sessionId,
    events,
  };
}

async function main() {
  ensureDir(outDir);
  console.log(`[program-soak:t2] writing logs to ${outDir}`);
  console.log(`[program-soak:t2] Convox ${serverWs}/widget/ws · aelio-test-2 ${test2Http}`);

  const ready = await preflight();
  const sdk = ready.sdk || {};
  console.log(
    `[program-soak:t2] sdk tools=${(sdk.functions || []).length} app=${sdk.application || '(n/a)'} customer=${customerId}`,
  );

  try {
    await fetch(`${test2Http}/api/aelio/chat-config?customerId=${encodeURIComponent(customerId)}`);
  } catch {
    // optional
  }

  const socket = new WebSocket(`${serverWs}/widget/ws`);
  await new Promise((resolve, reject) => {
    socket.once('open', resolve);
    socket.once('error', reject);
  });

  const inbox = [];
  socket.on('message', (raw) => {
    try {
      inbox.push({ at: Date.now(), message: JSON.parse(raw.toString()) });
    } catch {
      inbox.push({ at: Date.now(), message: { type: 'parse_error', raw: raw.toString() } });
    }
  });

  socket.send(
    JSON.stringify({
      type: 'init',
      customerId,
      email: `${phone}@employer.test`,
    }),
  );

  const readyMsg = await waitFor(
    socket,
    (m) => m.type === 'ready' || m.type === 'error',
    20_000,
    'init ready',
  );
  if (readyMsg.type === 'error') {
    throw new Error(`init failed: ${readyMsg.code || ''} ${readyMsg.message || JSON.stringify(readyMsg)}`);
  }
  const sessionId = readyMsg.sessionId || '';

  const turns = [];
  for (const testCase of CASES) {
    if (turns.length) await sleep(turnGapMs);
    let turn = await runOneTurn(socket, inbox, sessionId, testCase);

    // If a requireProgramId turn finished without ids, ask once to quote Observation fields.
    if (!turn.ok && testCase.requireProgramId && /missing program_id/i.test(turn.error || '')) {
      console.log(`[turn ${testCase.id}] follow-up — quote Observation program_id/source_hash`);
      await sleep(turnGapMs);
      const follow = {
        ...testCase,
        text:
          'Do not call any tools. Reply with only the program_id and source_hash from the latest run_program Observation (verbatim).',
        requireProgramId: true,
      };
      const quoted = await runOneTurn(socket, inbox, sessionId, follow);
      const merged = `${turn.assistant}\n${quoted.assistant}`;
      if (/prog-[a-f0-9]{8,}/i.test(merged)) {
        turn = {
          ...turn,
          assistant: `${turn.assistant}\n\n[follow-up quote]\n${quoted.assistant}`,
          ok: !isBadAssistant(quoted.assistant),
          error: null,
          elapsedMs: turn.elapsedMs + quoted.elapsedMs,
          events: [...turn.events, ...quoted.events],
        };
      }
    }

    turns.push(turn);
    writeTurnMd(turn);
    console.log(
      `[turn ${testCase.id}] ${turn.ok ? 'OK' : 'ERROR'} (${turn.elapsedMs}ms) ${(turn.assistant || turn.error || '')
        .slice(0, 140)
        .replace(/\s+/g, ' ')}`,
    );
  }

  socket.close();

  const joined = turns.map((t) => t.assistant || '').join('\n');
  const programIdMatch = joined.match(/prog-[a-f0-9]{8,}/i);
  const hashMatch = joined.match(/\b[a-f0-9]{32,}\b/i);
  const sawReuse = /reuse|same source|source_hash|identical|matched|match/i.test(
    turns.filter((t) => /reuse|close/i.test(t.label)).map((t) => t.assistant || '').join('\n'),
  );
  const demoHit = /demo_account_snapshot|demo_company_snapshot|Aelio Test Co|Sanjith Demo Employer/i.test(
    joined,
  );
  const scenarioOk =
    turns.every((t) => t.ok) && Boolean(programIdMatch) && Boolean(hashMatch) && sawReuse;

  const turnsMd = [
    `# aelio-test-2 program soak — ${runId}`,
    '',
    `- target: aelio-test-2 ONLY (\`${test2Http}\`) via Convox widget`,
    `- customerId: \`${customerId}\``,
    `- sessionId: \`${sessionId}\``,
    `- evidence: program_id=${programIdMatch?.[0] || 'MISSING'} hash=${hashMatch ? 'present' : 'MISSING'} reuse=${sawReuse} demo=${demoHit}`,
    '',
    '| # | case | status | ms | preview |',
    '|---|---|---|---|---|',
    ...turns.map((t) => {
      const preview = (t.assistant || t.error || '').replace(/\|/g, '\\|').replace(/\s+/g, ' ').slice(0, 100);
      return `| ${t.id} | ${t.label} | ${t.ok ? 'OK' : 'ERROR'} | ${t.elapsedMs} | ${preview} |`;
    }),
    '',
    '## Full transcript',
    '',
    ...turns.flatMap((t) => [
      `### Turn ${t.id} — ${t.label} (${t.ok ? 'OK' : 'ERROR'})`,
      '',
      `**User:** ${t.user}`,
      '',
      '**Assistant:**',
      '',
      '```text',
      mdEscape(t.assistant || t.error || '(empty)'),
      '```',
      '',
    ]),
  ].join('\n');
  writeFileSync(join(outDir, 'turns.md'), turnsMd);

  const failed = turns.filter((t) => !t.ok);
  const summary = [
    `# aelio-test-2 program soak — ${runId}`,
    '',
    `- **result:** ${scenarioOk ? 'PASS' : 'FAIL'}`,
    `- **authority:** aelio-test-2 only`,
    `- **server:** \`${serverHttp}\``,
    `- **aelio-test-2:** \`${test2Http}\``,
    `- **customerId:** \`${customerId}\``,
    `- **program_id:** ${programIdMatch?.[0] ? `\`${programIdMatch[0]}\`` : '_missing_'}`,
    `- **source_hash_seen:** ${hashMatch ? 'yes' : 'no'}`,
    `- **reuse_language:** ${sawReuse}`,
    `- **demo payloads mentioned:** ${demoHit}`,
    '',
    '## Failures',
    '',
    failed.length
      ? failed.map((t) => `- turn ${t.id} (${t.label}): ${t.error || 'empty/bad reply'}`).join('\n')
      : scenarioOk
        ? '_none_'
        : '- scenario incomplete: missing program_id / source_hash / reuse evidence',
    '',
    '## Per-turn files',
    '',
    ...turns.map((t) => `- \`turn-${String(t.id).padStart(2, '0')}-${slug(t.label)}.md\``),
    '',
    'See `turns.md`.',
    '',
  ].join('\n');
  writeFileSync(join(outDir, 'SUMMARY.md'), summary);

  console.log(`\n[program-soak:t2] ${scenarioOk ? 'PASS' : 'FAIL'} — ${outDir}`);
  if (!scenarioOk) process.exitCode = 1;
}

main().catch((error) => {
  console.error('[program-soak:t2] fatal', error);
  process.exit(1);
});
