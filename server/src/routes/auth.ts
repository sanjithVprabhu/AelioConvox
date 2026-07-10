import {
  createMagicLink,
  issueWidgetSessionToken,
  verifyMagicLink,
} from '@aelio/core';
import type { FastifyInstance } from 'fastify';
import { z } from 'zod';
import { authorized } from '../auth.js';
import { checkRateLimit } from '../rate-limit-http.js';
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
    if (magicLink.require_sdk_auth && !authorized(request, deps.config.secret)) {
      return reply.status(401).send({ error: 'Unauthorized' });
    }

    const parsed = MagicLinkRequestSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.status(400).send({ error: 'Invalid request body' });
    }

    const ip = request.ip || 'unknown';
    const emailKey = parsed.data.email.toLowerCase();
    const limit = magicLink.rate_limit_per_minute;

    const ipLimit = checkRateLimit(`magic-link:ip:${ip}`, limit, 60_000);
    if (!ipLimit.allowed) {
      return reply
        .status(429)
        .header('retry-after', String(Math.ceil((ipLimit.retryAfterMs ?? 60_000) / 1000)))
        .send({ error: 'Rate limit exceeded. Try again later.' });
    }

    const emailLimit = checkRateLimit(`magic-link:email:${emailKey}`, limit, 60_000);
    if (!emailLimit.allowed) {
      return reply
        .status(429)
        .header('retry-after', String(Math.ceil((emailLimit.retryAfterMs ?? 60_000) / 1000)))
        .send({ error: 'Rate limit exceeded for this email. Try again later.' });
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

    const sessionToken = issueWidgetSessionToken(
      deps.config.secret,
      {
        customerId: verified.customerId,
        externalId: verified.externalId,
        email: verified.email,
      },
      magicLink.session_token_ttl_minutes,
    );

    return {
      customerId: verified.customerId,
      externalId: verified.externalId,
      email: verified.email,
      sessionToken,
    };
  });
}
