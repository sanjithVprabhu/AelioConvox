import { createMagicLink, verifyMagicLink } from '@aelio/core';
import type { FastifyInstance } from 'fastify';
import { z } from 'zod';
import type { RuntimeDeps } from '../runtime-deps.js';

const MagicLinkRequestSchema = z.object({
  email: z.string().email(),
  externalId: z.string().optional(),
});

export async function registerAuthRoutes(app: FastifyInstance, deps: RuntimeDeps) {
  const magicLink = deps.config.channels.web.magic_link;
  if (!magicLink?.enabled) {
    return;
  }

  app.post('/auth/magic-link', async (request, reply) => {
    const parsed = MagicLinkRequestSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.status(400).send({ error: 'Invalid request body' });
    }

    const host = request.headers.host ?? `localhost:${deps.config.server.port}`;
    const protocol = request.protocol;
    const baseUrl = `${protocol}://${host}`;

    const result = await createMagicLink(deps.database, {
      email: parsed.data.email,
      externalId: parsed.data.externalId,
      baseUrl,
      ttlMinutes: magicLink.ttl_minutes,
    });

    return {
      email: result.email,
      expiresAt: result.expiresAt.toISOString(),
      url: result.url,
      ...(process.env.AELIO_TEST_MODE === '1' ? { token: result.token } : {}),
    };
  });

  app.get('/auth/verify', async (request, reply) => {
    const token = (request.query as Record<string, string>).token;
    if (!token) {
      return reply.status(400).send({ error: 'Missing token' });
    }

    const verified = await verifyMagicLink(deps.database, token);
    if (!verified) {
      return reply.status(401).send({ error: 'Invalid or expired token' });
    }

    return {
      customerId: verified.customerId,
      externalId: verified.externalId,
      email: verified.email,
    };
  });
}