import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { processTurn, verifyWidgetSessionToken } from '@aelio/core';
import { MAX_WS_FRAME_BYTES } from '@aelio/protocol';
import type { FastifyInstance } from 'fastify';
import { resolvePublicDir } from '../paths.js';
import type { RuntimeDeps } from '../runtime-deps.js';
import { buildTurnInput } from '../turn-options.js';
import { z } from 'zod';

const WIDGET_WS_PATH = '/widget/ws';

function clientMessageSchema(maxMessageLength: number) {
  return z.discriminatedUnion('type', [
    z.object({
      type: z.literal('init'),
      customerId: z.string().min(1),
      email: z.string().email().optional(),
      /** Required when identity.allow_anonymous is false — issued by /auth/verify. */
      sessionToken: z.string().min(1).optional(),
    }),
    z.object({
      type: z.literal('message'),
      content: z.string().min(1).max(maxMessageLength),
    }),
  ]);
}

export async function registerWidgetRoutes(app: FastifyInstance, deps: RuntimeDeps) {
  if (!deps.config.channels.web.enabled) {
    return;
  }

  const maxMessageLength = deps.config.channels.web.max_message_length;
  const ClientMessageSchema = clientMessageSchema(maxMessageLength);

  app.get('/widget.js', async (_request, reply) => {
    const content = readFileSync(join(resolvePublicDir(), 'widget.js'), 'utf8');
    return reply.type('application/javascript').send(content);
  });

  app.get(WIDGET_WS_PATH, { websocket: true }, (socket, request) => {
    const origin = request.headers.origin;
    const allowed = deps.config.channels.web.allowed_origins;
    const strictOrigin = allowed.length > 0 && !allowed.includes('*');
    const remoteAddress = request.socket?.remoteAddress ?? '';
    const isLoopback = ['127.0.0.1', '::1', '::ffff:127.0.0.1'].includes(remoteAddress);

    if (strictOrigin) {
      if (origin) {
        if (!allowed.includes(origin)) {
          socket.close(1008, 'Origin not allowed');
          return;
        }
      } else if (
        (process.env.NODE_ENV === 'production' && process.env.AELIO_TEST_MODE !== '1') ||
        !isLoopback
      ) {
        socket.close(1008, 'Origin header required');
        return;
      }
    }

    let customerId = 'anonymous';
    let channelAddress = `web:${request.socket?.remoteAddress ?? 'unknown'}`;
    let initialized = false;
    // Serialize turns per socket so rapid messages cannot interleave.
    let turnChain: Promise<void> = Promise.resolve();

    const resetTurnChain = () => {
      turnChain = Promise.resolve();
    };

    socket.on('message', (raw) => {
      const frame = raw.toString();
      if (frame.length > MAX_WS_FRAME_BYTES) {
        socket.send(JSON.stringify({ type: 'error', message: 'Message frame too large' }));
        return;
      }

      turnChain = turnChain
        .then(async () => {
        let parsed: unknown;
        try {
          parsed = JSON.parse(frame);
        } catch {
          socket.send(JSON.stringify({ type: 'error', message: 'Invalid JSON message' }));
          return;
        }

        const result = ClientMessageSchema.safeParse(parsed);
        if (!result.success) {
          const tooLong = result.error.issues.some((issue) => issue.code === 'too_big');
          socket.send(
            JSON.stringify({
              type: 'error',
              message: tooLong
                ? `Message too long (max ${maxMessageLength} characters)`
                : 'Invalid message format',
            }),
          );
          return;
        }

        const message = result.data;
        if (message.type === 'init') {
          if (initialized) {
            socket.send(JSON.stringify({ type: 'error', message: 'Session already initialized' }));
            return;
          }

          if (!deps.config.identity.allow_anonymous) {
            if (!message.sessionToken) {
              socket.send(
                JSON.stringify({
                  type: 'error',
                  message: 'Authentication required. Use a magic link before starting chat.',
                }),
              );
              return;
            }

            const claims = verifyWidgetSessionToken(deps.config.secret, message.sessionToken);
            if (!claims) {
              socket.send(
                JSON.stringify({
                  type: 'error',
                  message: 'Invalid or expired session token. Verify your magic link again.',
                }),
              );
              return;
            }

            // Identity comes from the verified token — never trust client-supplied IDs.
            customerId = claims.externalId;
            channelAddress = `web:${claims.email}`;
            initialized = true;
            socket.send(JSON.stringify({ type: 'ready', customerId }));
            return;
          }

          if (message.customerId === 'anonymous') {
            customerId = 'anonymous';
            channelAddress = `web:${request.socket?.remoteAddress ?? 'unknown'}`;
          } else if (message.sessionToken) {
            const claims = verifyWidgetSessionToken(deps.config.secret, message.sessionToken);
            if (claims) {
              customerId = claims.externalId;
              channelAddress = `web:${claims.email}`;
            } else {
              customerId = message.customerId;
              channelAddress = message.email ? `web:${message.email}` : `web:${customerId}`;
            }
          } else {
            customerId = message.customerId;
            channelAddress = message.email ? `web:${message.email}` : `web:${customerId}`;
          }

          initialized = true;
          socket.send(JSON.stringify({ type: 'ready', customerId }));
          return;
        }

        if (!initialized) {
          socket.send(JSON.stringify({ type: 'error', message: 'Send init before messaging' }));
          return;
        }

        socket.send(JSON.stringify({ type: 'typing', active: true }));

        try {
          const { reply, turnId, pendingConfirmation } = await processTurn(
            buildTurnInput(deps, {
              customerExternalId: customerId,
              channel: 'web',
              channelAddress,
              message: message.content,
            }),
          );

          if (pendingConfirmation) {
            socket.send(
              JSON.stringify({
                type: 'confirmation',
                prompt: reply,
                functionName: pendingConfirmation.functionName,
                turnId,
              }),
            );
          } else {
            socket.send(
              JSON.stringify({
                type: 'message',
                role: 'assistant',
                content: reply,
                turnId,
              }),
            );
          }
        } catch (error) {
          app.log.error(error);
          socket.send(
            JSON.stringify({
              type: 'error',
              message: error instanceof Error ? error.message : 'Failed to process message',
            }),
          );
        } finally {
          socket.send(JSON.stringify({ type: 'typing', active: false }));
        }
      })
        .catch((error) => {
          app.log.error(error);
          resetTurnChain();
        });
    });
  });
}
