import { createDatabase } from '@aelio/db';
import { extractMemories, recallMemories } from '@aelio/core';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import WebSocket from 'ws';

const migrationsFolder = join(
  dirname(fileURLToPath(import.meta.url)),
  '../packages/db/drizzle',
);

const serverUrl = (process.env.AELIO_WS_URL ?? process.env.AELIO_SERVER_URL ?? 'ws://127.0.0.1:3000')
  .replace(/^http/, 'ws');

async function testAnalystUnit() {
  const dir = mkdtempSync(join(tmpdir(), 'aelio-memory-'));
  const dbPath = join(dir, 'test.db');

  try {
    const database = createDatabase(dbPath);
    database.migrate(migrationsFolder);
    const customerId = randomUUID();
    const sessionId = randomUUID();

    const { customers, sessions } = await import('@aelio/db');
    const now = new Date();
    await database.db.insert(customers).values({
      id: customerId,
      externalId: 'memory-test-user',
      displayName: 'Memory Test User',
      createdAt: now,
      updatedAt: now,
    });
    await database.db.insert(sessions).values({
      id: sessionId,
      customerId,
      channel: 'web',
      status: 'active',
      startedAt: now,
      lastActivityAt: now,
    });

    const stored = await extractMemories({
      database,
      customerId,
      sessionId,
      userMessage: 'I prefer metric units for measurements',
      assistantReply: 'Got it, I will use metric units for you.',
    });

    if (!stored.some((fact) => fact.includes('metric'))) {
      throw new Error(`Expected metric preference fact, got: ${stored.join(', ')}`);
    }

    const recalled = await recallMemories(database, customerId, 'what units do I prefer?', 3);
    if (!recalled.some((entry) => entry.content.includes('metric'))) {
      throw new Error(`Recall missed metric preference: ${JSON.stringify(recalled)}`);
    }

    console.log('[Phase 5] Unit test — stored:', stored);
    console.log('[Phase 5] Unit test — recalled:', recalled.map((entry) => entry.content));
    database.close();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function waitForMessage(socket, predicate, timeoutMs = 15000) {
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error('Timed out waiting for message')), timeoutMs);
    const handler = (raw) => {
      const message = JSON.parse(raw.toString());
      if (predicate(message)) {
        clearTimeout(timeout);
        socket.off('message', handler);
        resolve(message);
      }
    };
    socket.on('message', handler);
  });
}

async function testMemoryIntegration() {
  const socket = new WebSocket(`${serverUrl.replace(/\/$/, '')}/widget/ws`);

  await new Promise((resolve, reject) => {
    socket.on('open', resolve);
    socket.on('error', reject);
  });

  socket.send(JSON.stringify({ type: 'init', customerId: 'phase5-memory-user' }));
  await waitForMessage(socket, (message) => message.type === 'ready');

  socket.send(JSON.stringify({ type: 'message', content: 'I prefer metric units' }));
  await waitForMessage(socket, (message) => message.type === 'message' && message.role === 'assistant');

  // Memory extraction runs as an async background job (spec §9, step 17), so the
  // stored fact is not guaranteed to be queryable on the very next turn. Poll the
  // recall-backed question until memory is reflected, rather than racing a fixed delay.
  let reply = null;
  for (let attempt = 0; attempt < 8; attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, 500));
    socket.send(JSON.stringify({ type: 'message', content: 'what units do I prefer?' }));
    reply = await waitForMessage(
      socket,
      (message) => message.type === 'message' && message.role === 'assistant',
    );
    if (reply.content.toLowerCase().includes('metric')) {
      break;
    }
  }

  socket.close();

  if (!reply || !reply.content.toLowerCase().includes('metric')) {
    throw new Error(`Expected memory-informed reply, got: ${reply ? reply.content : 'no reply'}`);
  }

  console.log('[Phase 5] Integration reply:', reply.content);
}

try {
  await testAnalystUnit();
  console.log('[Phase 5] Unit test PASSED — extract + vector recall');

  await testMemoryIntegration();
  console.log('[Phase 5] Integration PASSED — conversation memory influences reply');
  console.log('[Phase 5] PASSED');
  process.exit(0);
} catch (error) {
  console.error('[Phase 5] FAILED:', error);
  process.exit(1);
}
