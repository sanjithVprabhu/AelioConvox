import { enqueueJob } from '@aelio/core';
import { parseWhatsAppWebhook, verifyWhatsAppSignature } from '@aelio/channels';
import type { FastifyInstance } from 'fastify';
import type { RuntimeDeps } from '../runtime-deps.js';

export async function registerWhatsAppRoutes(app: FastifyInstance, deps: RuntimeDeps) {
  const wa = deps.config.channels.whatsapp;
  if (!wa.enabled) {
    return;
  }

  const webhookPath = wa.webhook_path ?? '/wa/webhook';

  app.get(webhookPath, async (request, reply) => {
    const query = request.query as Record<string, string>;
    const mode = query['hub.mode'];
    const token = query['hub.verify_token'];
    const challenge = query['hub.challenge'];

    if (mode === 'subscribe' && token === wa.verify_token) {
      return reply.status(200).send(challenge);
    }

    return reply.status(403).send('Forbidden');
  });

  app.post(webhookPath, async (request, reply) => {
    const rawBody = JSON.stringify(request.body ?? {});

    if (wa.app_secret) {
      const signature = request.headers['x-hub-signature-256'] as string | undefined;
      if (!verifyWhatsAppSignature(rawBody, signature, wa.app_secret)) {
        return reply.status(401).send('Invalid signature');
      }
    }

    const messages = parseWhatsAppWebhook(request.body);

    for (const message of messages) {
      await enqueueJob(deps.database, 'inbound', {
        channel: 'whatsapp',
        from: message.from,
        text: message.text,
        messageId: message.messageId,
        phoneNumberId: message.phoneNumberId,
      });
    }

    return reply.status(200).send({ success: true, queued: messages.length });
  });
}