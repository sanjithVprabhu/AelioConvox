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

    // Require verify_token to be configured — otherwise `undefined === undefined`
    // would let anyone subscribe the webhook (SEC-003).
    if (!wa.verify_token) {
      app.log.error('WhatsApp webhook verification attempted but channels.whatsapp.verify_token is not set');
      return reply.status(403).send('Forbidden');
    }
    if (mode === 'subscribe' && token === wa.verify_token) {
      return reply.status(200).send(challenge);
    }

    return reply.status(403).send('Forbidden');
  });

  // A live Meta integration MUST verify signatures. Only skip when explicitly in
  // mock mode or using the bring-your-own-provider path (no Meta creds here).
  const requiresSignature = !wa.mock_mode && wa.provider === 'meta';
  if (requiresSignature && !wa.app_secret) {
    app.log.error(
      'WhatsApp provider is "meta" but channels.whatsapp.app_secret is not set — inbound webhooks cannot be verified (SEC-005).',
    );
  }

  app.post(webhookPath, async (request, reply) => {
    // Sign the RAW bytes Meta sent (captured by the content-type parser), not a
    // re-serialized copy (SEC-004).
    const rawBody = (request as unknown as { rawBody?: string }).rawBody ?? JSON.stringify(request.body ?? {});
    const signature = request.headers['x-hub-signature-256'] as string | undefined;

    if (wa.app_secret) {
      if (!verifyWhatsAppSignature(rawBody, signature, wa.app_secret)) {
        return reply.status(401).send('Invalid signature');
      }
    } else if (requiresSignature) {
      // Live Meta channel with no secret configured → reject unsigned inbound
      // rather than trusting forged messages (SEC-005).
      return reply.status(401).send('Signature verification not configured');
    }

    const messages = parseWhatsAppWebhook(request.body);

    let queued = 0;
    for (const message of messages) {
      // Meta redelivers webhooks on slow ACKs — drop ids we already claimed.
      if (message.messageId && !deps.database.claimInboundMessage(`wa:${message.messageId}`)) {
        app.log.info({ messageId: message.messageId }, 'Duplicate WhatsApp webhook dropped');
        continue;
      }
      await enqueueJob(deps.database, 'inbound', {
        channel: 'whatsapp',
        from: message.from,
        text: message.text,
        messageId: message.messageId,
        phoneNumberId: message.phoneNumberId,
      });
      queued += 1;
    }

    return reply.status(200).send({ success: true, queued });
  });
}