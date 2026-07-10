import { createHash } from 'node:crypto';
import { Readable } from 'node:stream';
import { enqueueJob } from '@aelio/core';
import { parseWhatsAppWebhook, verifyWhatsAppSignature } from '@aelio/channels';
import type { FastifyInstance, FastifyRequest } from 'fastify';
import type { RuntimeDeps } from '../runtime-deps.js';

declare module 'fastify' {
  interface FastifyRequest {
    rawBody?: string;
  }
}

function inboundDedupKey(
  channel: string,
  from: string,
  text: string,
  messageId?: string,
): string {
  if (messageId) {
    return `${channel}:${messageId}`;
  }
  const hash = createHash('sha256').update(`${from}|${text}`).digest('hex').slice(0, 16);
  return `${channel}:fallback:${hash}`;
}

async function captureRawBody(
  request: FastifyRequest,
  _reply: unknown,
  payload: NodeJS.ReadableStream,
): Promise<Readable> {
  const chunks: Buffer[] = [];
  for await (const chunk of payload) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk as string));
  }
  const raw = Buffer.concat(chunks);
  request.rawBody = raw.toString('utf8');
  return Readable.from(raw);
}

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

  app.post(
    webhookPath,
    {
      // Capture Meta's exact request bytes for HMAC — re-serializing
      // JSON.stringify(request.body) can change key order / whitespace.
      preParsing: captureRawBody,
    },
    async (request, reply) => {
      const rawBody = request.rawBody ?? JSON.stringify(request.body ?? {});

      if (wa.app_secret) {
        const signature = request.headers['x-hub-signature-256'] as string | undefined;
        if (!verifyWhatsAppSignature(rawBody, signature, wa.app_secret)) {
          return reply.status(401).send('Invalid signature');
        }
      }

      const messages = parseWhatsAppWebhook(request.body);

      let queued = 0;
      for (const message of messages) {
        const dedupKey = inboundDedupKey('wa', message.from, message.text, message.messageId);
        if (!deps.database.claimInboundMessage(dedupKey)) {
          app.log.info(
            { messageId: message.messageId, dedupKey },
            'Duplicate WhatsApp webhook dropped',
          );
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
    },
  );
}
