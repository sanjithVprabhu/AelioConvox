import WebSocket from 'ws';

const serverUrl = process.env.AELIO_SERVER_URL ?? 'ws://127.0.0.1:3000';

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
    socket.send(JSON.stringify({ type: 'init', customerId: 'e2e-user' }));
    await waitForMessage(socket, (message) => message.type === 'ready');

    socket.send(JSON.stringify({ type: 'message', content: 'what is my order status?' }));
    await waitForMessage(socket, (message) => message.type === 'typing' && message.active === true);
    const reply = await waitForMessage(
      socket,
      (message) => message.type === 'message' && message.role === 'assistant',
    );

    console.log('Assistant reply:', reply.content);
    if (!reply.content.toLowerCase().includes('ship')) {
      throw new Error('Expected shipping-related reply from tool loop');
    }

    console.log('E2E widget test passed');
    socket.close();
    process.exit(0);
  } catch (error) {
    console.error('E2E widget test failed:', error);
    socket.close();
    process.exit(1);
  }
});

socket.on('error', (error) => {
  console.error('WebSocket error:', error);
  process.exit(1);
});