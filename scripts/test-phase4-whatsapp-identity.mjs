const baseUrl = process.env.AELIO_SERVER_URL ?? 'http://127.0.0.1:3000';
const phone = '919111222333';

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
                id: 'wamid.phase4.identity',
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
    const data = await response.json();
    const match = (data.messages ?? []).find(predicate);
    if (match) return match;
    await sleep(300);
  }
  throw new Error('Timed out waiting for WhatsApp outbound message');
}

try {
  const post = await fetch(`${baseUrl}/wa/webhook`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(webhookPayload),
  });
  const queued = await post.json();
  if (!post.ok || queued.queued !== 1) {
    throw new Error(`Webhook POST failed: ${post.status}`);
  }

  const outbound = await waitForOutbox(
    (message) => message.to === phone && message.text.toLowerCase().includes('ship'),
  );

  console.log('[Phase 4] WhatsApp identity reply for', phone, ':', outbound.text);

  const second = structuredClone(webhookPayload);
  second.entry[0].changes[0].value.messages[0].id = 'wamid.phase4.identity.2';
  second.entry[0].changes[0].value.messages[0].text.body = 'check my order again';

  await fetch(`${baseUrl}/wa/webhook`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(second),
  });

  const outbound2 = await waitForOutbox(
    (message) =>
      message.to === phone &&
      message.text.toLowerCase().includes('ship') &&
      message.sentAt > outbound.sentAt,
  );

  console.log('[Phase 4] Same phone second message resolved:', outbound2.text);
  console.log('[Phase 4] PASSED — phone number maps to persistent WhatsApp customer');
  process.exit(0);
} catch (error) {
  console.error('[Phase 4] WhatsApp identity FAILED:', error);
  process.exit(1);
}