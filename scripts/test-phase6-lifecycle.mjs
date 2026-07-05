/**
 * Phase 6 — SDK lifecycle catalog: states, policies, flows + setCustomerState.
 * Requires Aelio server + an SDK with lifecycle definitions running.
 */
import WebSocket from 'ws';

const baseUrl = process.env.AELIO_SERVER_URL ?? 'http://127.0.0.1:3000';
const secret = process.env.AELIO_SDK_SECRET ?? 'change-me-in-production';
const wsUrl = new URL('/sdk', baseUrl.replace(/^http/, 'ws'));
wsUrl.searchParams.set('secret', secret);

const socket = new WebSocket(wsUrl);

socket.on('open', () => {
  socket.send(
    JSON.stringify({
      type: 'register',
      sdkVersion: 'test',
      language: 'node',
      functions: [
        {
          name: 'getOrderStatus',
          description: 'Get order status',
          params: { orderId: 'string' },
          safety: 'read',
        },
        {
          name: 'upgradePlan',
          description: 'Upgrade plan',
          params: { plan: 'string' },
          safety: 'write',
        },
      ],
      states: [
        {
          id: 'onboarding',
          description: 'Setup only. No upgrades.',
          allowedTools: ['getOrderStatus'],
          blockedTools: ['upgradePlan'],
        },
      ],
      policies: [
        {
          id: 'stay-in-lifecycle',
          description: 'Stay within lifecycle boundaries.',
          severity: 'hard',
        },
      ],
      flows: [
        {
          id: 'onboarding_setup',
          state: 'onboarding',
          description: 'Setup flow',
          steps: [{ id: 'check_order', goal: 'Check an order', tool: 'getOrderStatus' }],
        },
      ],
    }),
  );

  socket.send(
    JSON.stringify({
      type: 'set_state',
      customerId: 'lifecycle-test-user',
      stateId: 'onboarding',
      reason: 'phase6_test',
    }),
  );
});

setTimeout(async () => {
  try {
    const health = await fetch(`${baseUrl}/diagnostics`);
    const body = await health.json();
    const fnNames = body.sdk?.functions?.map((f) => f.name) ?? [];
    if (!fnNames.includes('getOrderStatus')) {
      throw new Error('SDK functions not visible in diagnostics');
    }

    const widgetUrl = new URL('/widget/ws', baseUrl.replace(/^http/, 'ws'));
    const widget = new WebSocket(widgetUrl);

    widget.on('open', () => {
      widget.send(JSON.stringify({ type: 'init', customerId: 'lifecycle-test-user' }));
    });

    widget.on('message', (raw) => {
      const msg = JSON.parse(raw.toString());
      if (msg.type === 'ready') {
        widget.send(JSON.stringify({ type: 'message', content: 'upgrade me to pro please' }));
        return;
      }
      if (msg.type === 'message' && msg.role === 'assistant') {
        console.log('[Phase 6] Assistant reply:', msg.content);
        widget.close();
        socket.close();
        console.log('[Phase 6] PASSED — lifecycle catalog registered and state-scoped turn completed');
        process.exit(0);
      }
    });

    widget.on('error', (error) => {
      console.error('[Phase 6] Widget error:', error);
      process.exit(1);
    });
  } catch (error) {
    console.error('[Phase 6] FAILED:', error);
    process.exit(1);
  }
}, 800);

socket.on('error', (error) => {
  console.error('[Phase 6] SDK socket error:', error);
  process.exit(1);
});

setTimeout(() => {
  console.error('[Phase 6] Timed out');
  process.exit(1);
}, 30_000);