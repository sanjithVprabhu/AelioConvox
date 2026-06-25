import { sendProactiveMessage, setProactiveOptIn } from '@aelio/core';
import type { FastifyInstance, FastifyRequest } from 'fastify';
import { z } from 'zod';
import type { RuntimeDeps } from '../runtime-deps.js';

const SendSchema = z.object({
  customerExternalId: z.string().min(1),
  channel: z.string().min(1),
  content: z.string().min(1),
  templateName: z.string().optional(),
  dedupKey: z.string().optional(),
});

const OptInSchema = z.object({
  customerExternalId: z.string().min(1),
  optIn: z.boolean(),
});

// The dev's backend triggers proactive messages on its own domain events, so
// these endpoints are authenticated with the same SDK secret.
function authorized(request: FastifyRequest, secret: string): boolean {
  const header = request.headers.authorization;
  const bearer = header?.startsWith('Bearer ') ? header.slice('Bearer '.length) : undefined;
  const provided = bearer ?? (request.headers['x-aelio-secret'] as string | undefined);
  return provided === secret;
}

export async function registerProactiveRoutes(app: FastifyInstance, deps: RuntimeDeps) {
  if (!deps.config.proactive.enabled) {
    return;
  }

  const proactiveConfig = {
    enabled: deps.config.proactive.enabled,
    requireOptIn: deps.config.proactive.require_opt_in,
    maxPerCustomerPerDay: deps.config.proactive.max_per_customer_per_day,
    windowHours: deps.config.proactive.window_hours,
  };

  app.post('/proactive/opt-in', async (request, reply) => {
    if (!authorized(request, deps.config.secret)) {
      return reply.status(401).send({ error: 'Unauthorized' });
    }
    const parsed = OptInSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.status(400).send({ error: 'Invalid request body' });
    }
    const ok = await setProactiveOptIn(deps.database, parsed.data.customerExternalId, parsed.data.optIn);
    if (!ok) {
      return reply.status(404).send({ error: 'Unknown customer' });
    }
    return { ok: true, customerExternalId: parsed.data.customerExternalId, optIn: parsed.data.optIn };
  });

  app.post('/proactive', async (request, reply) => {
    if (!authorized(request, deps.config.secret)) {
      return reply.status(401).send({ error: 'Unauthorized' });
    }
    const parsed = SendSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.status(400).send({ error: 'Invalid request body' });
    }

    const result = await sendProactiveMessage({
      database: deps.database,
      config: proactiveConfig,
      customerExternalId: parsed.data.customerExternalId,
      channel: parsed.data.channel,
      content: parsed.data.content,
      templateName: parsed.data.templateName,
      dedupKey: parsed.data.dedupKey,
    });

    return reply.status(result.ok ? 202 : 200).send(result);
  });
}
