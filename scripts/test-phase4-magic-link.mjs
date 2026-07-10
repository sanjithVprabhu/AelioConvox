import WebSocket from 'ws';

const baseUrl = process.env.AELIO_SERVER_URL ?? 'http://127.0.0.1:3000';
const secret = process.env.AELIO_SDK_SECRET ?? 'change-me-in-production';

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

try {
  const unauthorized = await fetch(`${baseUrl}/auth/magic-link`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ email: 'phase4-user@example.com', externalId: 'user_phase4_123' }),
  });
  if (unauthorized.status !== 401) {
    throw new Error(`Expected 401 without SDK secret, got ${unauthorized.status}`);
  }

  const issue = await fetch(`${baseUrl}/auth/magic-link`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      authorization: `Bearer ${secret}`,
    },
    body: JSON.stringify({ email: 'phase4-user@example.com', externalId: 'user_phase4_123' }),
  });
  const issued = await issue.json();
  if (!issue.ok || !issued.token) {
    throw new Error(`Magic link issue failed: ${issue.status} ${JSON.stringify(issued)}`);
  }
  console.log('[Phase 4] Magic link issued for', issued.email);

  const verify = await fetch(`${baseUrl}/auth/verify?token=${issued.token}`);
  const verified = await verify.json();
  if (!verify.ok || verified.externalId !== 'user_phase4_123' || !verified.sessionToken) {
    throw new Error(`Magic link verify failed: ${verify.status} ${JSON.stringify(verified)}`);
  }
  console.log('[Phase 4] Magic link verified:', verified.externalId);

  const socket = new WebSocket(`${baseUrl.replace(/^http/, 'ws')}/widget/ws`);
  await new Promise((resolve, reject) => {
    socket.on('open', resolve);
    socket.on('error', reject);
  });

  // Spoof attempt: different customerId without session token should still work
  // only when allow_anonymous is true (local config). With sessionToken, identity
  // is bound to the verified externalId.
  socket.send(
    JSON.stringify({
      type: 'init',
      customerId: 'spoofed-id',
      email: verified.email,
      sessionToken: verified.sessionToken,
    }),
  );
  const ready = await waitForMessage(socket, (message) => message.type === 'ready');
  if (ready.customerId !== verified.externalId) {
    throw new Error(
      `Widget init did not bind to verified customer id (got ${ready.customerId})`,
    );
  }

  socket.close();
  console.log('[Phase 4] PASSED — magic link auth → verified widget session');
  process.exit(0);
} catch (error) {
  console.error('[Phase 4] Magic link FAILED:', error);
  process.exit(1);
}
