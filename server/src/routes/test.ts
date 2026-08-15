import { getMockWhatsAppMessages } from '@aelio/channels';
import type { FastifyInstance } from 'fastify';
import type { ServerSdkBridge } from '../sdk-bridge.js';

export async function registerTestRoutes(app: FastifyInstance, sdkBridge: ServerSdkBridge) {
  if (process.env.AELIO_TEST_MODE !== '1') {
    return;
  }

  app.get('/__test__/whatsapp/outbox', async () => ({
    messages: getMockWhatsAppMessages(),
  }));

  app.get('/__test__/sdk/functions', async () => ({
    functions: sdkBridge.getFunctions().map((fn) => fn.name),
  }));

  /** Full registered catalog — used by e2e to assert tool-group expansion. */
  app.get('/__test__/sdk/catalog', async () => ({
    functions: sdkBridge.getFunctions(),
    states: sdkBridge.getStates(),
    policies: sdkBridge.getPolicies().map((policy) => ({ id: policy.id, severity: policy.severity })),
    flows: sdkBridge.getFlows().map((flow) => ({ id: flow.id, state: flow.state })),
    pipeline: sdkBridge.getPipelineManifest?.() ?? null,
  }));
}