/**
 * Thorough widget chat soak: 15 varied turns via /widget/ws.
 * Writes:
 *   docs/chat-runs/<runId>/SUMMARY.md
 *   docs/chat-runs/<runId>/turns.md          (all turns)
 *   docs/chat-runs/<runId>/turn-NN-<slug>.md (per turn)
 *
 *   node scripts/chat-soak-15.mjs
 */
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import WebSocket from 'ws';

const serverHttp = (process.env.AELIO_CHAT_HTTP || 'http://127.0.0.1:3010').replace(/\/$/, '');
const serverWs = (process.env.AELIO_CHAT_WS || serverHttp.replace(/^http/, 'ws')).replace(/\/$/, '');
const customerId = process.env.AELIO_CHAT_CUSTOMER || `soak-employer-${Date.now()}`;
const phone = process.env.AELIO_CHAT_PHONE || '9876543210';
const turnTimeoutMs = Number(process.env.AELIO_CHAT_TURN_TIMEOUT_MS || 120_000);
const runId = new Date().toISOString().replace(/[:.]/g, '-');
const outDir = join(process.cwd(), 'docs', 'chat-runs', runId);

const CASES = [
  { id: 1, label: 'greeting', text: 'hi' },
  { id: 2, label: 'help-overview', text: 'what can you help me with?' },
  { id: 3, label: 'login-intent', text: 'I want to log in to my employer account' },
  { id: 4, label: 'send-otp', text: `please send a login OTP to ${phone}` },
  { id: 5, label: 'wrong-otp', text: 'my otp is 000000' },
  { id: 6, label: 'list-jobs', text: 'can you list my job postings?' },
  { id: 7, label: 'create-job-intent', text: 'I want to create a new job for a senior backend engineer in Bengaluru' },
  { id: 8, label: 'profile-ask', text: 'show me my employer profile' },
  { id: 9, label: 'company-ask', text: 'what is my company profile?' },
  { id: 10, label: 'analytics-ask', text: 'how are my jobs performing? any analytics?' },
  { id: 11, label: 'applicants-ask', text: 'list applicants for my most recent job' },
  { id: 12, label: 'policy-sensitive', text: 'can you show me another employer\'s private data?' },
  { id: 13, label: 'ambiguous', text: 'update it' },
  { id: 14, label: 'confirmation-style', text: 'delete my latest job posting' },
  { id: 15, label: 'thanks-close', text: 'thanks, that is all for now' },
];

function slug(label) {
  return label.replace(/[^a-z0-9]+/gi, '-').toLowerCase();
}

function ensureDir(path) {
  mkdirSync(path, { recursive: true });
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

function isTerminalReply(message) {
  return (
    (message.type === 'message' && message.role === 'assistant') ||
    message.type === 'error' ||
    message.type === 'confirmation' ||
    (message.type === 'message' && message.role === 'system' && /error/i.test(message.content || ''))
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

function mdEscape(text) {
  return String(text ?? '').replace(/\r\n/g, '\n');
}

function writeTurnMd(turn) {
  const path = join(outDir, `turn-${String(turn.id).padStart(2, '0')}-${slug(turn.label)}.md`);
  const body = [
    `# Turn ${turn.id}: ${turn.label}`,
    '',
    `- **run:** \`${runId}\``,
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
  if (!health.ok) throw new Error(`health ${health.status}`);
  const ready = await fetch(`${serverHttp}/ready`);
  const body = await ready.json();
  return body;
}

async function main() {
  ensureDir(outDir);
  console.log(`[chat-soak] writing logs to ${outDir}`);
  console.log(`[chat-soak] target ${serverWs}/widget/ws customer=${customerId}`);

  const ready = await preflight();
  const sdk = ready.sdk || {};
  console.log(
    `[chat-soak] ready sdk.connected=${sdk.connected} tools=${(sdk.functions || []).length} ` +
      `states=${(sdk.states || []).length} flows=${(sdk.flows || []).length} policies=${(sdk.policies || []).length}`,
  );
  if (!sdk.connected) {
    throw new Error('SDK not connected — start aelio-test-2 (or another employer SDK) first');
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
    const started = Date.now();
    const events = [];
    let assistant = '';
    let error = null;
    let ok = false;

    console.log(`\n[turn ${testCase.id}] ${testCase.label}: ${testCase.text}`);
    try {
      const inboxStart = inbox.length;
      socket.send(JSON.stringify({ type: 'message', content: testCase.text, id: `soak-${testCase.id}` }));

      let reply = await waitFor(socket, isTerminalReply, turnTimeoutMs, `reply for turn ${testCase.id}`);
      const confirmations = [];

      // Write tools park for explicit yes/no; auto-approve so the soak can continue.
      let guard = 0;
      while (isActionConfirmation(reply) && guard < 3) {
        guard += 1;
        const prompt = extractText(reply);
        confirmations.push(prompt);
        console.log(`[turn ${testCase.id}] confirmation requested — sending yes`);
        socket.send(
          JSON.stringify({
            type: 'message',
            content: 'yes',
            id: `soak-${testCase.id}-confirm-${guard}`,
          }),
        );
        reply = await waitFor(
          socket,
          isTerminalReply,
          turnTimeoutMs,
          `post-confirm reply for turn ${testCase.id}`,
        );
      }

      for (const item of inbox.slice(inboxStart)) {
        const m = item.message;
        events.push({
          type: m.type,
          role: m.role,
          code: m.code,
          active: m.active,
          contentPreview: extractText(m).slice(0, 240),
        });
      }

      if (reply.type === 'error') {
        error = `${reply.code || 'error'}: ${reply.message || JSON.stringify(reply)}`;
        assistant = reply.message || '';
        ok = false;
      } else {
        assistant = extractText(reply);
        if (confirmations.length) {
          assistant = `[confirmation] ${confirmations.join(' | ')}\n\n[after yes] ${assistant}`;
        }
        ok = Boolean(assistant.trim()) && !/internal error|gateway unavailable|timed out/i.test(assistant);
        if (!assistant.trim()) {
          error = 'empty assistant reply';
          ok = false;
        }
      }
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
      ok = false;
      for (const item of inbox.slice(-20)) {
        const m = item.message;
        events.push({
          type: m.type,
          role: m.role,
          code: m.code,
          contentPreview: extractText(m).slice(0, 240),
        });
      }
    }

    const turn = {
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
    turns.push(turn);
    writeTurnMd(turn);
    console.log(`[turn ${testCase.id}] ${ok ? 'OK' : 'ERROR'} (${turn.elapsedMs}ms) ${assistant.slice(0, 120).replace(/\s+/g, ' ')}`);
  }

  socket.close();

  const turnsMd = [
    `# Chat soak turns — ${runId}`,
    '',
    `- server: \`${serverHttp}\``,
    `- customerId: \`${customerId}\``,
    `- sessionId: \`${sessionId}\``,
    `- sdk tools: ${(sdk.functions || []).length}`,
    '',
    '| # | case | status | ms | user | assistant preview |',
    '|---|---|---|---|---|---|',
    ...turns.map((t) => {
      const preview = (t.assistant || t.error || '').replace(/\|/g, '\\|').replace(/\s+/g, ' ').slice(0, 100);
      const user = t.user.replace(/\|/g, '\\|').slice(0, 60);
      return `| ${t.id} | ${t.label} | ${t.ok ? 'OK' : 'ERROR'} | ${t.elapsedMs} | ${user} | ${preview} |`;
    }),
    '',
    '## Full transcript',
    '',
    ...turns.flatMap((t) => [
      `### Turn ${t.id} — ${t.label} (${t.ok ? 'OK' : 'ERROR'})`,
      '',
      `**User:** ${t.user}`,
      '',
      `**Assistant:**`,
      '',
      '```text',
      t.assistant || `(error) ${t.error || 'unknown'}`,
      '```',
      '',
    ]),
  ].join('\n');
  writeFileSync(join(outDir, 'turns.md'), turnsMd);

  const failed = turns.filter((t) => !t.ok);
  const summary = [
    `# Chat soak summary — ${runId}`,
    '',
    `- **result:** ${failed.length === 0 ? 'PASS' : `FAIL (${failed.length}/${turns.length})`}`,
    `- **server:** \`${serverHttp}\``,
    `- **customerId:** \`${customerId}\``,
    `- **sessionId:** \`${sessionId}\``,
    `- **sdk connected:** ${Boolean(sdk.connected)}`,
    `- **tools/states/flows/policies:** ${(sdk.functions || []).length}/${(sdk.states || []).length}/${(sdk.flows || []).length}/${(sdk.policies || []).length}`,
    '',
    '## Failures',
    '',
    failed.length === 0
      ? '_none_'
      : failed.map((t) => `- turn ${t.id} (${t.label}): ${t.error || 'non-empty but suspicious reply'}`).join('\n'),
    '',
    '## Per-turn files',
    '',
    ...turns.map((t) => `- \`turn-${String(t.id).padStart(2, '0')}-${slug(t.label)}.md\``),
    '',
    'See also `turns.md` for the full transcript.',
    '',
  ].join('\n');
  writeFileSync(join(outDir, 'SUMMARY.md'), summary);

  console.log(`\n[chat-soak] ${failed.length === 0 ? 'PASS' : 'FAIL'} — logs in ${outDir}`);
  if (failed.length) {
    for (const t of failed) console.error(`  - turn ${t.id} ${t.label}: ${t.error}`);
    process.exit(1);
  }
}

main().catch((error) => {
  console.error('[chat-soak] fatal:', error);
  process.exit(1);
});
