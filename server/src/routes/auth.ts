import { createMagicLink, createSessionToken, verifyMagicLink } from '@aelio/core';
import type { FastifyInstance } from 'fastify';
import { z } from 'zod';
import { requireSecret } from '../auth.js';
import type { RuntimeDeps } from '../runtime-deps.js';

const MagicLinkRequestSchema = z.object({
  email: z.string().email(),
  externalId: z.string().optional(),
});

function makeRateLimiter(maxHits: number, windowMs: number) {
  const hits = new Map<string, number[]>();
  return (key: string, now = Date.now()): boolean => {
    const recent = (hits.get(key) ?? []).filter((t) => now - t < windowMs);
    if (recent.length >= maxHits) {
      hits.set(key, recent);
      return false;
    }
    recent.push(now);
    hits.set(key, recent);
    if (hits.size > 10_000) {
      for (const [k, ts] of hits) {
        if (ts.every((t) => now - t >= windowMs)) hits.delete(k);
      }
    }
    return true;
  };
}

export async function registerAuthRoutes(app: FastifyInstance, deps: RuntimeDeps) {
  const magicLink = deps.config.channels.web.magic_link;
  if (!magicLink?.enabled) {
    return;
  }

  const perIp = makeRateLimiter(10, 60_000);
  const perEmail = makeRateLimiter(5, 60 * 60_000);
  const clientIp = (request: { ip: string; headers: Record<string, unknown> }): string =>
    process.env.AELIO_TRUST_PROXY === '1'
      ? (String(request.headers['x-forwarded-for'] ?? '').split(',')[0]?.trim() || request.ip)
      : request.ip;

  app.post('/auth/magic-link', async (request, reply) => {
    if (!requireSecret(request, reply, deps.config.secret)) {
      return reply;
    }

    const parsed = MagicLinkRequestSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.status(400).send({ error: 'Invalid request body' });
    }

    const ip = clientIp(request);
    if (!perIp(ip) || !perEmail(parsed.data.email.toLowerCase())) {
      app.log.warn({ ip, email: parsed.data.email }, 'Magic-link request rate-limited');
      return reply.status(429).send({ error: 'Too many requests — try again later.' });
    }

    const host = request.headers.host ?? `localhost:${deps.config.server.port}`;
    const protocol = request.protocol;
    const baseUrl = `${protocol}://${host}`;

    const result = await createMagicLink(
      {
        email: parsed.data.email,
        externalId: parsed.data.externalId,
        baseUrl,
        ttlMinutes: magicLink.ttl_minutes,
      },
      deps.customerStore,
      deps.magicLinkStore,
    );

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

    const verified = await verifyMagicLink(token, deps.magicLinkStore);
    if (!verified) {
      return reply.status(401).send({ error: 'Invalid or expired token' });
    }

    const sessionToken = createSessionToken(
      deps.config.secret,
      {
        customerId: verified.customerId,
        externalId: verified.externalId,
        email: verified.email,
      },
      magicLink.session_token_ttl_minutes ?? 60,
    );

    return {
      customerId: verified.customerId,
      externalId: verified.externalId,
      email: verified.email,
      sessionToken,
    };
  });
}
