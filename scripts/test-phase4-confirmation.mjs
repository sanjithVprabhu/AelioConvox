import WebSocket from 'ws';

const serverUrl = (process.env.AELIO_WS_URL ?? process.env.AELIO_SERVER_URL ?? 'ws://127.0.0.1:3000')
  .replace(/^http/, 'ws');

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
    socket.send(JSON.stringify({ type: 'init', customerId: 'phase4-confirm-user' }));
    await waitForMessage(socket, (message) => message.type === 'ready');

    socket.send(JSON.stringify({ type: 'message', content: 'please cancel my order' }));
    const prompt = await waitForMessage(
      socket,
      (message) =>
        (message.type === 'confirmation' && message.prompt) ||
        (message.type === 'message' && message.role === 'assistant'),
    );

    const promptText = prompt.prompt ?? prompt.content;
    console.log('[Phase 4] Confirmation prompt:', promptText);
    if (!promptText.toLowerCase().includes('confirm') || !promptText.includes('cancelOrder')) {
      throw new Error('Expected confirmation prompt for cancelOrder');
    }

    socket.send(JSON.stringify({ type: 'message', content: 'yes' }));
    const result = await waitForMessage(
      socket,
      (message) => message.type === 'message' && message.role === 'assistant',
    );

    console.log('[Phase 4] After confirmation:', result.content);
    if (!result.content.toLowerCase().includes('cancel')) {
      throw new Error('Expected cancellation success message');
    }

    console.log('[Phase 4] PASSED — write action requires confirmation before SDK invoke');
    socket.close();
    process.exit(0);
  } catch (error) {
    console.error('[Phase 4] FAILED:', error);
    socket.close();
    process.exit(1);
  }
});

socket.on('error', (error) => {
  console.error('[Phase 4] WebSocket error:', error);
  process.exit(1);
});
