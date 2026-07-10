import {
  DEFAULT_SDK_PATH,
  HEARTBEAT_INTERVAL_MS,
  SdkToServerMessageSchema,
} from '@aelio/protocol';
import { enqueueJob, upsertCustomerFlowProgress, upsertCustomerLifecycleState } from '@aelio/core';
import type { WebSocket } from '@fastify/websocket';
import type { FastifyInstance } from 'fastify';
import { randomUUID } from 'node:crypto';
import type { RuntimeDeps } from '../runtime-deps.js';

export async function registerSdkRoutes(app: FastifyInstance, deps: RuntimeDeps) {
  const { config, sdkBridge, database } = deps;
  setInterval(() => {
    const now = Date.now();
    for (const connectionId of sdkBridge.getConnectionIds()) {
      const connection = sdkBridge.getConnection(connectionId);
      if (!connection) {
        continue;
      }

      if (now - connection.lastHeartbeatAt > HEARTBEAT_INTERVAL_MS * 2) {
        connection.socket.close(1000, 'Heartbeat timeout');
        sdkBridge.unregister(connectionId);
        app.log.warn({ connectionId }, 'SDK connection timed out');
        continue;
      }

      connection.socket.send(JSON.stringify({ type: 'ping', ts: now }));
    }
  }, HEARTBEAT_INTERVAL_MS).unref();

  app.get(DEFAULT_SDK_PATH, { websocket: true }, (socket, request) => {
    const remoteAddress = request.socket.remoteAddress ?? 'unknown';
    // Preferred: Authorization header (never logged). Query-param `?secret=`
    // remains as a DEPRECATED fallback for older SDKs.
    const header = request.headers.authorization;
    const bearer = header?.startsWith('Bearer ') ? header.slice('Bearer '.length) : undefined;
    const secret =
      bearer ?? new URL(request.url, 'http://localhost').searchParams.get('secret');
    if (!secret || secret !== config.secret) {
      app.log.warn(
        {
          route: DEFAULT_SDK_PATH,
          remoteAddress,
          usedAuthorizationHeader: Boolean(bearer),
        },
        'Rejected SDK websocket connection with invalid secret',
      );
      socket.close(1008, 'Invalid SDK secret');
      return;
    }
    if (!bearer) {
      app.log.warn('SDK connected with ?secret= in the URL — deprecated; upgrade the SDK to send an Authorization header');
    }

    const connectionId = randomUUID();
    app.log.info(
      {
        connectionId,
        route: DEFAULT_SDK_PATH,
        remoteAddress,
        usedAuthorizationHeader: Boolean(bearer),
      },
      'SDK websocket connection accepted',
    );

    socket.on('message', (raw) => {
      let parsed: unknown;
      try {
        parsed = JSON.parse(raw.toString());
      } catch {
        app.log.warn(
          {
            connectionId,
            remoteAddress,
            payloadPreview: raw.toString().slice(0, 160),
          },
          'SDK sent invalid JSON payload',
        );
        return;
      }

      const result = SdkToServerMessageSchema.safeParse(parsed);
      if (!result.success) {
        app.log.warn(
          {
            connectionId,
            remoteAddress,
            issues: result.error.issues.map((issue) => ({
              path: issue.path.join('.'),
              message: issue.message,
            })),
          },
          'SDK payload failed schema validation',
        );
        return;
      }

      const message = result.data;
      if (message.type === 'register') {
        sdkBridge.register({
          id: connectionId,
          socket: socket as WebSocket,
          functions: message.functions,
          states: message.states ?? [],
          policies: message.policies ?? [],
          flows: message.flows ?? [],
          persona: message.persona?.trim() || null,
          productBrief: message.productBrief?.trim() || null,
          sdkVersion: message.sdkVersion,
          language: message.language,
          canSend: message.canSend ?? false,
          connectedAt: Date.now(),
          lastHeartbeatAt: Date.now(),
        });

        app.log.info(
          {
            connectionId,
            sdkVersion: message.sdkVersion,
            language: message.language,
            functions: message.functions.map((fn) => fn.name),
            states: (message.states ?? []).map((state) => state.id),
            policies: (message.policies ?? []).map((policy) => policy.id),
            flows: (message.flows ?? []).map((flow) => flow.id),
            canSend: message.canSend ?? false,
          },
          'Aelio SDK connected and registered catalog',
        );
        return;
      }

      if (message.type === 'set_state') {
        app.log.info(
          {
            connectionId,
            customerId: message.customerId,
            stateId: message.stateId,
            reason: message.reason ?? null,
          },
          'SDK requested customer lifecycle state update',
        );
        void upsertCustomerLifecycleState(
          database.db,
          message.customerId,
          message.stateId,
          message.reason,
        ).then(() => {
          app.log.info(
            {
              connectionId,
              customerId: message.customerId,
              stateId: message.stateId,
              reason: message.reason,
            },
            'Customer lifecycle state updated from SDK',
          );
        });
        return;
      }

      if (message.type === 'set_flow_progress') {
        app.log.info(
          {
            connectionId,
            customerId: message.customerId,
            flowId: message.flowId,
            stepIndex: message.stepIndex,
            completedSteps: message.completedSteps ?? [],
          },
          'SDK requested flow progress update',
        );
        void upsertCustomerFlowProgress(
          database.db,
          message.customerId,
          message.flowId,
          message.stepIndex,
          message.completedSteps,
        ).then(() => {
          app.log.info(
            {
              connectionId,
              customerId: message.customerId,
              flowId: message.flowId,
              stepIndex: message.stepIndex,
            },
            'Customer flow progress updated from SDK',
          );
        });
        return;
      }

      if (message.type === 'pong') {
        sdkBridge.touchHeartbeat(connectionId);
        app.log.debug({ connectionId, remoteAddress, ts: message.ts }, 'SDK heartbeat acknowledged');
        return;
      }

      if (message.type === 'result') {
        app.log.info(
          {
            connectionId,
            requestId: message.id,
            ok: message.ok,
            durationMs: message.durationMs,
            errorCode: message.error?.code ?? null,
          },
          'SDK function invocation completed',
        );
        sdkBridge.handleResult(message);
        return;
      }

      if (message.type === 'ingest') {
        app.log.info(
          {
            connectionId,
            channel: message.channel,
            from: message.from,
            messageId: message.messageId ?? null,
            textPreview: message.text.slice(0, 160),
            textLength: message.text.length,
          },
          'SDK submitted inbound channel message',
        );
        // The dev's own channel webhook handed us an inbound message. Queue it
        // for the inbound worker exactly like a built-in channel would.
        if (
          message.messageId &&
          !database.claimInboundMessage(`${message.channel}:${message.messageId}`)
        ) {
          app.log.info(
            { connectionId, messageId: message.messageId },
            'Duplicate ingest dropped',
          );
          return;
        }
        void enqueueJob(database, 'inbound', {
          channel: message.channel,
          from: message.from,
          text: message.text,
          messageId: message.messageId,
          source: 'sdk-ingest',
        });
        app.log.info(
          { connectionId, channel: message.channel, from: message.from },
          'Ingested inbound message from SDK channel',
        );
      }
    });

    socket.on('close', () => {
      sdkBridge.unregister(connectionId);
      app.log.info({ connectionId, remoteAddress }, 'Aelio SDK disconnected');
    });
  });
}
