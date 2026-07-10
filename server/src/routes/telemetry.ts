import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { listConversationTelemetry, listTurnApiCalls, summarizeTurnApiCalls } from '@aelio/core';
import type { FastifyInstance } from 'fastify';
import { resolvePublicDir } from '../paths.js';
import type { RuntimeDeps } from '../runtime-deps.js';
import { requireSecret } from '../auth.js';

export async function registerTelemetryRoutes(app: FastifyInstance, deps: RuntimeDeps) {
  // Telemetry exposes conversation content and per-turn call data — gate every
  // route behind the SDK secret (Authorization: Bearer <secret>). SEC-002.
  const secret = deps.config.secret;

  // The dashboard shell is static HTML with no data (it fetches the gated JSON
  // APIs below via JS, supplying the secret), so it stays public — a browser
  // can't send an Authorization header on navigation anyway.
  app.get('/telemetry', async (_request, reply) => {
    const html = readFileSync(join(resolvePublicDir(), 'telemetry.html'), 'utf8');
    return reply.type('text/html').send(html);
  });

  app.get('/api/v1/telemetry/events', async (request, reply) => {
    if (!requireSecret(request, reply, secret)) {
      return reply;
    }
    const query = request.query as {
      limit?: string;
      since?: string;
      session_id?: string;
      customer_id?: string;
    };

    if (!deps.config.sunjet.enabled || !deps.sunjetClient) {
      return reply.status(503).send({
        enabled: false,
        events: [],
        message: 'Sunjet is not enabled — enable sunjet in config to stream conversation telemetry.',
      });
    }

    const limit = query.limit ? Number.parseInt(query.limit, 10) : 100;
    const since = query.since ? Number.parseInt(query.since, 10) : undefined;

    const events = await listConversationTelemetry(
      deps.sunjetClient,
      deps.config.sunjet.tables.conversations,
      {
        limit: Number.isFinite(limit) ? limit : 100,
        since: Number.isFinite(since) ? since : undefined,
        sessionId: query.session_id,
        customerExternalId: query.customer_id,
      },
    );

    return {
      enabled: true,
      table: deps.config.sunjet.tables.conversations,
      count: events.length,
      events,
    };
  });

  app.get('/api/v1/telemetry/turn-calls', async (request, reply) => {
    if (!requireSecret(request, reply, secret)) {
      return reply;
    }
    const query = request.query as {
      turn_id?: string;
      session_id?: string;
      limit?: string;
    };

    const limit = query.limit ? Number.parseInt(query.limit, 10) : 200;

    if (query.turn_id) {
      const summary = await summarizeTurnApiCalls(deps.database, query.turn_id);
      return {
        ...summary,
        source: 'sqlite',
      };
    }

    const calls = await listTurnApiCalls(deps.database, {
      sessionId: query.session_id,
      limit: Number.isFinite(limit) ? limit : 200,
    });

    return {
      source: 'sqlite',
      count: calls.length,
      calls,
    };
  });
}