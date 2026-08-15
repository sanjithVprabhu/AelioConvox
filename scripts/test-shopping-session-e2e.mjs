import WebSocket from 'ws';

const serverUrl = (process.env.AELIO_WS_URL ?? 'ws://127.0.0.1:3010').replace(/^http/, 'ws');
const customerId = process.env.AELIO_SHOPPING_CUSTOMER ?? `shopping-e2e-${Date.now()}`;
const email = process.env.AELIO_SHOPPING_EMAIL ?? `${customerId}@example.com`;

function waitForMessage(socket, predicate, timeoutMs = 120_000) {
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      socket.off('message', handler);
      reject(new Error('Timed out waiting for the shopping assistant'));
    }, timeoutMs);
    const handler = (raw) => {
      const message = JSON.parse(raw.toString());
      if (!predicate(message)) return;
      clearTimeout(timeout);
      socket.off('message', handler);
      resolve(message);
    };
    socket.on('message', handler);
  });
}

function assistantOrConfirmation(message) {
  return (
    (message.type === 'confirmation' && typeof message.prompt === 'string') ||
    (message.type === 'message' && message.role === 'assistant')
  );
}

function textOf(message) {
  return message.prompt ?? message.content ?? '';
}

async function ask(socket, content) {
  socket.send(JSON.stringify({ type: 'message', content }));
  return waitForMessage(socket, assistantOrConfirmation);
}

async function confirmIfRequested(socket, message) {
  const text = textOf(message);
  if (message.type !== 'confirmation' && !/\bconfirm\b/i.test(text)) return message;
  console.log(`confirmation: ${text}`);
  return ask(socket, 'yes');
}

function assertIncludes(text, patterns, label) {
  if (patterns.some((pattern) => pattern.test(text))) return;
  throw new Error(`${label}: unexpected assistant reply: ${text}`);
}

const socket = new WebSocket(`${serverUrl.replace(/\/$/, '')}/widget/ws`);

try {
  await new Promise((resolve, reject) => {
    socket.once('open', resolve);
    socket.once('error', reject);
  });

  socket.send(JSON.stringify({ type: 'init', customerId }));
  await waitForMessage(socket, (message) => message.type === 'ready');

  let reply = await ask(
    socket,
    `Start a shopping session for me. My email is ${email} and my name is Shopping Test.`,
  );
  reply = await confirmIfRequested(socket, reply);
  const sessionText = textOf(reply);
  console.log(`session: ${sessionText}`);
  assertIncludes(sessionText, [/session/i, /shop/i, /browse/i], 'shopping session did not start');
  if (/authorization|couldn['’]?t|unable|failed/i.test(sessionText)) {
    throw new Error(`shopping session reported failure: ${sessionText}`);
  }

  reply = await ask(socket, 'Find the Adjustable Desk Lamp and tell me its price and stock.');
  const productText = textOf(reply);
  console.log(`product: ${productText}`);
  assertIncludes(productText, [/Adjustable Desk Lamp/i], 'product search did not return the lamp');
  assertIncludes(productText, [/\$?76\b/, /14\s+(?:in stock|available)/i], 'product facts are missing');

  reply = await ask(socket, 'Add one Adjustable Desk Lamp to my cart.');
  reply = await confirmIfRequested(socket, reply);
  const addText = textOf(reply);
  console.log(`add: ${addText}`);
  assertIncludes(addText, [/added/i, /cart/i], 'lamp was not added to the cart');
  if (/authorization|couldn['’]?t|unable|failed/i.test(addText)) {
    throw new Error(`cart mutation reported failure: ${addText}`);
  }

  reply = await ask(socket, 'Show me my cart.');
  const cartText = textOf(reply);
  console.log(`cart: ${cartText}`);
  assertIncludes(cartText, [/Adjustable Desk Lamp/i], 'cart does not contain the lamp');

  console.log(`shopping-session E2E: PASS (${customerId})`);
} finally {
  socket.close();
}
