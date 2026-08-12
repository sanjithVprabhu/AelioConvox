import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { verifySessionToken, withSessionLock } from '@aelio/core/edge';
import type { FastifyInstance } from 'fastify';
import { resolvePublicDir } from '../paths.js';
import type { RuntimeDeps } from '../runtime-deps.js';
import { executeConversationTurn } from '../conversation-turn.js';
import { ensureChatSession, loadChatHistory } from '../chat-memory.js';
import { z } from 'zod';
import { CORE_KINDS, makeHello, intersectKinds } from '@aelio/chat-sdk';
import { MAX_WS_FRAME_BYTES } from '@aelio/protocol';

const WIDGET_WS_PATH = '/widget/ws';
const MAX_MESSAGE_LENGTH = 4096;

const ClientMessageSchema = z.discriminatedUnion('type', [
  z.object({
    type: z.literal('init'),
    customerId: z.string().min(1),
    email: z.string().email().optional(),
    authToken: z.string().min(1).optional(),
    sessionId: z.string().uuid().optional(),
    hello: z
      .object({
        protocol: z.literal('aelio-render@v1'),
        kinds: z.array(z.string()).min(1),
        limits: z
          .object({
            max_blocks: z.number().int().positive().optional(),
            max_nesting: z.number().int().positive().optional(),
          })
          .optional(),
      })
      .optional(),
  }),
  z.object({
    type: z.literal('message'),
    id: z.string().min(1).max(256).optional(),
    content: z.string().min(1).max(MAX_MESSAGE_LENGTH),
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
    const socketLockKey = `widget-socket:${randomUUID()}`;

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
    let internalCustomerId: string | null = null;
    let sessionId: string | null = null;
    let initialized = false;

    socket.on('message', (raw: Buffer | ArrayBuffer | Buffer[]) => {
      if (raw.toString().length > MAX_WS_FRAME_BYTES) {
        socket.send(JSON.stringify({ type: 'error', message: 'Message too large' }));
        socket.close(1009, 'Frame too large');
        return;
      }

      void withSessionLock(socketLockKey, async () => {
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
          if (initialized) {
            socket.send(
              JSON.stringify({
                type: 'error',
                code: 'already_initialized',
                message: 'This connection is already initialized.',
              }),
            );
            return;
          }

          const requireAuth = !deps.config.identity.allow_anonymous;
          const claims = message.authToken
            ? verifySessionToken(deps.config.secret, message.authToken)
            : null;

          if (requireAuth && !claims) {
            app.log.warn(
              { route: WIDGET_WS_PATH, remoteAddress, customerId: message.customerId },
              'Widget init rejected — valid session token required',
            );
            socket.send(
              JSON.stringify({
                type: 'error',
                code: 'auth_required',
                message: 'Authentication required. Verify a magic link and pass the session token.',
              }),
            );
            return;
          }

          if (claims) {
            if (message.customerId !== claims.externalId) {
              socket.send(
                JSON.stringify({
                  type: 'error',
                  code: 'identity_mismatch',
                  message: 'customerId does not match the verified session.',
                }),
              );
              return;
            }
            customerId = claims.externalId;
            channelAddress = `web:${claims.email}`;
          } else {
            if (message.customerId === 'anonymous') {
              socket.send(
                JSON.stringify({
                  type: 'error',
                  message: 'Anonymous access is disabled. Use a magic link before starting chat.',
                }),
              );
              return;
            }
            customerId = message.customerId;
            channelAddress = message.email ? `web:${message.email}` : `web:${customerId}`;
          }

          initialized = true;
          app.log.info(
            {
              route: WIDGET_WS_PATH,
              remoteAddress,
              customerId,
              channelAddress,
              email: message.email ?? claims?.email ?? null,
              authenticated: Boolean(claims),
            },
            'Widget client initialized',
          );

          try {
            const chatSession = await ensureChatSession(deps, {
              customerExternalId: customerId,
              channel: 'web',
              channelAddress,
              resumeSessionId: message.sessionId,
            });
            internalCustomerId = chatSession.customerId;
            sessionId = chatSession.sessionId;

            const history = await loadChatHistory(deps, sessionId);
            if (history.length > 0) {
              socket.send(
                JSON.stringify({
                  type: 'history',
                  messages: history.filter(
                    (row) => row.role === 'user' || row.role === 'assistant',
                  ),
                }),
              );
            }
          } catch (error) {
            app.log.error(
              { err: error, route: WIDGET_WS_PATH, remoteAddress, customerId },
              'Widget session bootstrap failed',
            );
            socket.send(
              JSON.stringify({
                type: 'error',
                code: 'session_bootstrap_failed',
                message: 'Could not restore your chat session. Please refresh and try again.',
              }),
            );
            socket.close(1011, 'Session bootstrap failed');
            return;
          }

          const hello = message.hello ?? makeHello();
          const accepted = intersectKinds(hello.kinds, CORE_KINDS);
          socket.send(
            JSON.stringify({
              type: 'ready',
              customerId,
              welcome: {
                session_id: sessionId,
                accepted_kinds: accepted.length ? accepted : ['text@1'],
                server_limits: { max_blocks: 64, max_nesting: 4 },
                protocol: 'aelio-render@v1',
              },
            }),
          );
          return;
        }

        if (!initialized || !internalCustomerId || !sessionId) {
          socket.send(JSON.stringify({ type: 'error', message: 'Send init before messaging' }));
          return;
        }

        socket.send(JSON.stringify({ type: 'typing', active: true }));

        try {
          const { reply, frame, turnId, awaitingConfirmation } = await executeConversationTurn(
            deps,
            {
              customerExternalId: customerId,
              channel: 'web',
              channelAddress,
              message: message.content,
              sourceTurnId: message.id,
              customerId: internalCustomerId,
              sessionId,
            },
          );

          const isConfirmation =
            /confirm this exact action|reply \*\*yes\*\* to confirm|yes to approve this exact action/i.test(
              reply,
            );

          if (isConfirmation || (awaitingConfirmation && /yes to (approve|confirm)/i.test(reply))) {
            socket.send(
              JSON.stringify({
                type: 'confirmation',
                prompt: reply,
                frame,
                turnId,
              }),
            );
          } else {
            socket.send(
              JSON.stringify({
                type: 'message',
                role: 'assistant',
                content: reply,
                frame,
                turnId,
              }),
            );
          }
        } catch (error) {
          app.log.error(
            { err: error, route: WIDGET_WS_PATH, remoteAddress, customerId },
            'Widget turn failed',
          );
          socket.send(
            JSON.stringify({
              type: 'error',
              message: 'Something went wrong processing your message. Please try again.',
            }),
          );
        } finally {
          socket.send(JSON.stringify({ type: 'typing', active: false }));
        }
      }).catch((error) => {
        app.log.error(
          { err: error, route: WIDGET_WS_PATH, remoteAddress, customerId },
          'Widget message handler failed',
        );
        try {
          socket.send(
            JSON.stringify({
              type: 'error',
              message: 'Something went wrong processing your message. Please try again.',
            }),
          );
        } catch {
          // Socket may already be closed.
        }
      });
    });

    socket.on('close', () => {
      app.log.info(
        { route: WIDGET_WS_PATH, remoteAddress, customerId, initialized },
        'Widget websocket disconnected',
      );
    });
  });
}
