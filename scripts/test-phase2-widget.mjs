import WebSocket from 'ws';
import { aelioWsUrl } from './lib/aelio-port.mjs';

const serverUrl = (process.env.AELIO_WS_URL ?? aelioWsUrl()).replace(/^http/, 'ws');

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

const socket = new WebSocket(`${serverUrl.replace(/\/$/, '')}/widget/ws`);

socket.on('open', async () => {
  try {
    socket.send(JSON.stringify({ type: 'init', customerId: 'phase2-user' }));
    await waitForMessage(socket, (message) => message.type === 'ready');

    socket.send(JSON.stringify({ type: 'message', content: 'what is my order status?' }));
    await waitForMessage(socket, (message) => message.type === 'typing' && message.active === true);
    const reply = await waitForMessage(
      socket,
      (message) => message.type === 'message' && message.role === 'assistant',
    );

    console.log('[Phase 2] Assistant reply:', reply.content);
    if (!reply.content.toLowerCase().includes('ship')) {
      throw new Error('Expected shipping-related reply from SDK tool loop');
    }
    if (!reply.frame || reply.frame.frame !== 'render' || !Array.isArray(reply.frame.blocks)) {
      throw new Error('Expected Render Protocol frame on assistant message');
    }
    if (reply.frame.blocks[0]?.kind !== 'text@1' && !reply.frame.blocks[0]?.fallback) {
      throw new Error(`Unexpected first block kind: ${reply.frame.blocks[0]?.kind}`);
    }

    console.log('[Phase 2] PASSED — Web widget → LLM → SDK → RenderFrame reply');
    socket.close();
    process.exit(0);
  } catch (error) {
    console.error('[Phase 2] FAILED:', error);
    socket.close();
    process.exit(1);
  }
});

socket.on('error', (error) => {
  console.error('[Phase 2] WebSocket error:', error);
  process.exit(1);
});
