const baseUrl = process.env.AELIO_SERVER_URL ?? 'http://127.0.0.1:3000';
const phone = '919876543210';

const webhookPayload = {
  object: 'whatsapp_business_account',
  entry: [
    {
      changes: [
        {
          value: {
            metadata: { phone_number_id: '123456789' },
            messages: [
              {
                id: 'wamid.phase3.test',
                from: phone,
                timestamp: `${Math.floor(Date.now() / 1000)}`,
                type: 'text',
                text: { body: 'what is my order status?' },
              },
            ],
          },
        },
      ],
    },
  ],
};

async function sleep(ms) {
  await new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitForOutbox(predicate, timeoutMs = 15000) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const response = await fetch(`${baseUrl}/__test__/whatsapp/outbox`);
    if (!response.ok) {
      throw new Error(`Outbox endpoint failed: ${response.status}`);
    }
    const data = await response.json();
    const match = (data.messages ?? []).find(predicate);
    if (match) {
      return match;
    }
    await sleep(300);
  }
  throw new Error('Timed out waiting for WhatsApp outbound message');
}

try {
  const verify = await fetch(
    `${baseUrl}/wa/webhook?hub.mode=subscribe&hub.verify_token=test-verify-token&hub.challenge=phase3-challenge`,
  );
  const challenge = await verify.text();
  if (challenge !== 'phase3-challenge') {
    throw new Error(`Webhook verification failed: got "${challenge}"`);
  }
  console.log('[Phase 3] Webhook verification OK');

  const post = await fetch(`${baseUrl}/wa/webhook`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(webhookPayload),
  });
  const queued = await post.json();
  if (!post.ok || queued.queued !== 1) {
    throw new Error(`Webhook POST failed: ${post.status} ${JSON.stringify(queued)}`);
  }
  console.log('[Phase 3] Inbound webhook queued');

  const outbound = await waitForOutbox(
    (message) => message.to === phone && message.text.toLowerCase().includes('ship'),
  );

  console.log('[Phase 3] WhatsApp reply:', outbound.text);
  console.log('[Phase 3] PASSED — WhatsApp webhook → queue → SDK tool call → mock send');
  process.exit(0);
} catch (error) {
  console.error('[Phase 3] FAILED:', error);
  process.exit(1);
}