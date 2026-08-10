import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { processTurn, verifySessionToken, withSessionLock } from '@aelio/core';
import type { FastifyInstance } from 'fastify';
import { randomUUID } from 'node:crypto';
import { resolvePublicDir } from '../paths.js';
import type { RuntimeDeps } from '../runtime-deps.js';
import { buildTurnInput } from '../turn-options.js';
import { processAelioRuntimeMessage } from '../aelio-runtime.js';
import { z } from 'zod';
import { MAX_WS_FRAME_BYTES } from '@aelio/protocol';

const WIDGET_WS_PATH = '/widget/ws';
const MAX_MESSAGE_LENGTH = 4096;

// Tools/personas can ask a multiple-choice question by ending a reply with this marker —
// the widget renders it as tappable quick-reply buttons instead of the raw marker text.
// Kept out of @aelio/protocol since these are plain JSON fields on an already-unvalidated
// outbound frame, not new message types.
const QUICK_REPLIES_PATTERN = /\[\[quick_replies:\s*([^\]]+)\]\]\s*$/i;
// Same idea, but for a day/month/year date-of-birth picker — the widget submits the
// selection back as an ISO "YYYY-MM-DD" message once the shopper picks all three.
const DOB_PICKER_PATTERN = /\[\[dob_picker\]\]\s*$/i;

function extractWidgetMarkers(reply: string): { content: string; options?: string[]; datePicker?: boolean } {
  const dobMatch = reply.match(DOB_PICKER_PATTERN);
  if (dobMatch) {
    return { content: reply.slice(0, dobMatch.index).trim(), datePicker: true };
  }
  const quickReplyMatch = reply.match(QUICK_REPLIES_PATTERN);
  if (!quickReplyMatch || !quickReplyMatch[1]) {
    return { content: reply };
  }
  const options = quickReplyMatch[1]
    .split('|')
    .map((option) => option.trim())
    .filter(Boolean);
  const content = reply.slice(0, quickReplyMatch.index).trim();
  return options.length > 0 ? { content, options } : { content };
}

const ClientMessageSchema = z.discriminatedUnion('type', [
  z.object({
    type: z.literal('init'),
    customerId: z.string().min(1),
    email: z.string().email().optional(),
    authToken: z.string().min(1).optional(),
  }),
  z.object({
    type: z.literal('message'),
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
    let initialized = false;

    socket.on('message', (raw) => {
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
          socket.send(JSON.stringify({ type: 'ready', customerId }));
          return;
        }

        if (!initialized) {
          socket.send(JSON.stringify({ type: 'error', message: 'Send init before messaging' }));
          return;
        }

        socket.send(JSON.stringify({ type: 'typing', active: true }));

        try {
          const runtimeResult = deps.runtimeStore
            ? await processAelioRuntimeMessage(deps, {
                tenantId: deps.config.name,
                subjectId: customerId,
                idempotencyKey: `web:${randomUUID()}`,
                message: message.content,
                channel: 'web',
              })
            : null;
          if (runtimeResult && runtimeResult.status !== 'committed') {
            // A duplicate frame is already answered; a conflict means a concurrent turn for this
            // subject won the commit. Neither may be answered with a fabricated reply.
            socket.send(
              JSON.stringify({
                type: 'error',
                code: runtimeResult.status,
                message:
                  runtimeResult.status === 'duplicate'
                    ? 'That message was already processed.'
                    : 'Another message for this conversation is still being processed. Please resend.',
              }),
            );
            return;
          }
          const legacyResult = runtimeResult
            ? null
            : await processTurn(buildTurnInput(deps, {
                customerExternalId: customerId, channel: 'web', channelAddress, message: message.content,
              }));
          const reply = runtimeResult?.reply ?? legacyResult?.reply ?? 'Please try again.';
          const turnId = runtimeResult?.eventId ?? legacyResult?.turnId ?? randomUUID();
          const awaitingConfirmation =
            runtimeResult?.awaitingConfirmation ?? legacyResult?.awaitingConfirmation;

          const isConfirmation =
            awaitingConfirmation ||
            /reply \*\*yes\*\* to confirm/i.test(reply);

          if (isConfirmation) {
            socket.send(
              JSON.stringify({
                type: 'confirmation',
                prompt: reply,
                turnId,
              }),
            );
          } else {
            const { content, options, datePicker } = extractWidgetMarkers(reply);
            socket.send(
              JSON.stringify({
                type: 'message',
                role: 'assistant',
                content,
                turnId,
                ...(options ? { options } : {}),
                ...(datePicker ? { datePicker } : {}),
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
