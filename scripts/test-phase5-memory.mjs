import { extractMemories, recallMemories, embed, cosineSimilarity } from '@aelio/core';
import { randomUUID } from 'node:crypto';
import WebSocket from 'ws';
import { aelioWsUrl } from './lib/aelio-port.mjs';

const serverUrl = (process.env.AELIO_WS_URL ?? aelioWsUrl()).replace(/^http/, 'ws');

/**
 * In-memory stand-in for ConvoxMemoryStore — same store/recall contract,
 * local cosine scoring. Production path is Aelio database VSS only.
 */
function createInMemoryMemoryStore() {
  /** @type {Array<{ memoryId: string, customerId: string, content: string, category: string | null, embedding: number[] }>} */
  const rows = [];

  return {
    async store(input) {
      const content = input.content.trim();
      if (!content) return null;
      if (
        input.dedupe !== false &&
        rows.some((row) => row.customerId === input.customerId && row.content === content)
      ) {
        return null;
      }
      const memoryId = randomUUID();
      rows.push({
        memoryId,
        customerId: input.customerId,
        content,
        category: input.category ?? null,
        embedding: input.embedding,
      });
      return { memoryId };
    },

    async recall(customerId, query, limit = 5, minScore = 0.05) {
      const queryEmbedding = await embed(query, { purpose: 'memory_recall' });
      return rows
        .filter((row) => row.customerId === customerId)
        .map((row) => ({
          id: row.memoryId,
          content: row.content,
          category: row.category,
          score: cosineSimilarity(queryEmbedding, row.embedding),
        }))
        .filter((entry) => entry.score >= minScore)
        .sort((a, b) => b.score - a.score)
        .slice(0, limit);
    },
  };
}

async function testAnalystUnit() {
  const memoryStore = createInMemoryMemoryStore();
  const customerId = randomUUID();
  const sessionId = randomUUID();

  const stored = await extractMemories({
    memoryStore,
    customerId,
    sessionId,
    userMessage: 'I prefer metric units for measurements',
    assistantReply: 'Got it, I will use metric units for you.',
  });

  if (!stored.some((fact) => fact.includes('metric'))) {
    throw new Error(`Expected metric preference fact, got: ${stored.join(', ')}`);
  }

  const recalled = await recallMemories(memoryStore, customerId, 'what units do I prefer?', 3);
  if (!recalled.some((entry) => entry.content.includes('metric'))) {
    throw new Error(`Recall missed metric preference: ${JSON.stringify(recalled)}`);
  }

  console.log('[Phase 5] Unit test — stored:', stored);
  console.log('[Phase 5] Unit test — recalled:', recalled.map((entry) => entry.content));
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
  const otherSocket = new WebSocket(`${serverUrl.replace(/\/$/, '')}/widget/ws`);

  await Promise.all([socket, otherSocket].map((candidate) => new Promise((resolve, reject) => {
    candidate.on('open', resolve);
    candidate.on('error', reject);
  })));

  const ready = waitForMessage(socket, (message) => message.type === 'ready');
  const otherReady = waitForMessage(otherSocket, (message) => message.type === 'ready');
  socket.send(JSON.stringify({ type: 'init', customerId: 'phase5-memory-user' }));
  otherSocket.send(JSON.stringify({ type: 'init', customerId: 'phase5-memory-other-user' }));
  await Promise.all([ready, otherReady]);

  const firstReply = waitForMessage(
    socket,
    (message) => message.type === 'message' && message.role === 'assistant',
  );
  const otherFirstReply = waitForMessage(
    otherSocket,
    (message) => message.type === 'message' && message.role === 'assistant',
  );
  socket.send(JSON.stringify({ type: 'message', content: 'I prefer metric units' }));
  otherSocket.send(JSON.stringify({ type: 'message', content: 'I prefer imperial units' }));
  await Promise.all([firstReply, otherFirstReply]);

  // Memory extraction runs as an async background job (spec §9, step 17), so the
  // stored fact is not guaranteed to be queryable on the very next turn. Poll the
  // recall-backed question until memory is reflected, rather than racing a fixed delay.
  let reply = null;
  let otherReply = null;
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
  for (let attempt = 0; attempt < 8; attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, 500));
    otherSocket.send(JSON.stringify({ type: 'message', content: 'what units do I prefer?' }));
    otherReply = await waitForMessage(
      otherSocket,
      (message) => message.type === 'message' && message.role === 'assistant',
    );
    if (otherReply.content.toLowerCase().includes('imperial')) {
      break;
    }
  }

  socket.close();
  otherSocket.close();

  if (
    !reply
    || !reply.content.toLowerCase().includes('metric')
    || reply.content.toLowerCase().includes('imperial')
  ) {
    throw new Error(`Expected memory-informed reply, got: ${reply ? reply.content : 'no reply'}`);
  }
  if (
    !otherReply
    || !otherReply.content.toLowerCase().includes('imperial')
    || otherReply.content.toLowerCase().includes('metric')
  ) {
    throw new Error(
      `Expected isolated second-user memory, got: ${otherReply ? otherReply.content : 'no reply'}`,
    );
  }

  console.log('[Phase 5] Integration reply:', reply.content);
  console.log('[Phase 5] Isolated second-user reply:', otherReply.content);
}

try {
  await testAnalystUnit();
  console.log('[Phase 5] Unit test PASSED — extract + recall (memory store contract)');

  await testMemoryIntegration();
  console.log('[Phase 5] Integration PASSED — scoped conversation memory influences reply');
  console.log('[Phase 5] PASSED');
  process.exit(0);
} catch (error) {
  console.error('[Phase 5] FAILED:', error);
  process.exit(1);
}
