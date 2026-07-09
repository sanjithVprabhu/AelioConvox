import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { processTurn } from '@aelio/core';
import type { FastifyInstance } from 'fastify';
import { resolvePublicDir } from '../paths.js';
import type { RuntimeDeps } from '../runtime-deps.js';
import { buildTurnInput } from '../turn-options.js';
import { z } from 'zod';

const WIDGET_WS_PATH = '/widget/ws';

const ClientMessageSchema = z.discriminatedUnion('type', [
  z.object({
    type: z.literal('init'),
    customerId: z.string().min(1),
    email: z.string().email().optional(),
    authToken: z.string().min(1).optional(),
  }),
  z.object({
    type: z.literal('message'),
    content: z.string().min(1),
  }),
]);

export async function registerWidgetRoutes(app: FastifyInstance, deps: RuntimeDeps) {
  if (!deps.config.channels.web.enabled) {
    return;
  }

  app.get('/widget.js', async (_request, reply) => {
    const content = readFileSync(join(resolvePublicDir(), 'widget.js'), 'utf8');
    return reply.type('application/javascript').send(content);
  });

  app.get(WIDGET_WS_PATH, { websocket: true }, (socket, request) => {
    const remoteAddress = request.socket.remoteAddress ?? 'unknown';
    const origin = request.headers.origin;
    const allowed = deps.config.channels.web.allowed_origins;
    app.log.info(
      {
        route: WIDGET_WS_PATH,
        remoteAddress,
        origin: origin ?? null,
      },
      'Widget websocket connection opened',
    );
    if (
      origin &&
      allowed.length > 0 &&
      !allowed.includes(origin) &&
      !allowed.includes('*')
    ) {
      app.log.warn(
        {
          route: WIDGET_WS_PATH,
          remoteAddress,
          origin,
          allowedOrigins: allowed,
        },
        'Widget websocket rejected by origin policy',
      );
      // Send a diagnosable reason before closing so the widget can show *why*
      // rather than sitting on "Connecting…". ws buffers this before the close.
      socket.send(
        JSON.stringify({
          type: 'error',
          code: 'origin_not_allowed',
          message: `Origin ${origin} is not in channels.web.allowed_origins`,
        }),
      );
      socket.close(1008, 'Origin not allowed');
      return;
    }

    let customerId = 'anonymous';
    let channelAddress = `web:${remoteAddress}`;
    let initialized = false;

    socket.on('message', (raw) => {
      void (async () => {
        let parsed: unknown;
        try {
          parsed = JSON.parse(raw.toString());
        } catch {
          socket.send(JSON.stringify({ type: 'error', message: 'Invalid JSON message' }));
          return;
        }

        const result = ClientMessageSchema.safeParse(parsed);
        if (!result.success) {
          socket.send(JSON.stringify({ type: 'error', message: 'Invalid message format' }));
          return;
        }

        const message = result.data;
        if (message.type === 'init') {
          if (!deps.config.identity.allow_anonymous && message.customerId === 'anonymous') {
            app.log.warn(
              {
                route: WIDGET_WS_PATH,
                remoteAddress,
                customerId: message.customerId,
              },
              'Widget init rejected because anonymous access is disabled',
            );
            socket.send(
              JSON.stringify({
                type: 'error',
                message: 'Authentication required. Use a magic link before starting chat.',
              }),
            );
            return;
          }

          customerId = message.customerId;
          channelAddress = message.email ? `web:${message.email}` : `web:${customerId}`;
          initialized = true;
          app.log.info(
            {
              route: WIDGET_WS_PATH,
              remoteAddress,
              customerId,
              channelAddress,
              email: message.email ?? null,
              hasAuthToken: Boolean(message.authToken),
            },
            'Widget client initialized',
          );
          socket.send(JSON.stringify({ type: 'ready', customerId }));
          return;
        }

        if (!initialized) {
          app.log.warn(
            {
              route: WIDGET_WS_PATH,
              remoteAddress,
            },
            'Widget message received before init',
          );
          socket.send(JSON.stringify({ type: 'error', message: 'Send init before messaging' }));
          return;
        }

        app.log.info(
          {
            route: WIDGET_WS_PATH,
            remoteAddress,
            customerId,
            channelAddress,
            contentPreview: message.content.slice(0, 160),
            contentLength: message.content.length,
          },
          'Widget message received',
        );
        socket.send(JSON.stringify({ type: 'typing', active: true }));

        try {
          const { reply, turnId } = await processTurn(
            buildTurnInput(deps, {
              customerExternalId: customerId,
              channel: 'web',
              channelAddress,
              message: message.content,
            }),
          );

          socket.send(
            JSON.stringify({
              type: 'message',
              role: 'assistant',
              content: reply,
              turnId,
            }),
          );
          app.log.info(
            {
              route: WIDGET_WS_PATH,
              remoteAddress,
              customerId,
              turnId,
              replyPreview: reply.slice(0, 160),
              replyLength: reply.length,
            },
            'Widget turn completed',
          );
        } catch (error) {
          app.log.error(
            {
              err: error,
              route: WIDGET_WS_PATH,
              remoteAddress,
              customerId,
            },
            'Widget turn failed',
          );
          socket.send(
            JSON.stringify({
              type: 'error',
              message: error instanceof Error ? error.message : 'Failed to process message',
            }),
          );
        } finally {
          socket.send(JSON.stringify({ type: 'typing', active: false }));
        }
      })();
    });

    socket.on('close', () => {
      app.log.info(
        {
          route: WIDGET_WS_PATH,
          remoteAddress,
          customerId,
          initialized,
        },
        'Widget websocket disconnected',
      );
    });
  });
}
