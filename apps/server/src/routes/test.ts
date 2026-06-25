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
}