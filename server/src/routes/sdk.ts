import {
  DEFAULT_SDK_PATH,
  HEARTBEAT_INTERVAL_MS,
  MAX_SDK_CONNECTIONS,
  MAX_WS_FRAME_BYTES,
  SDK_REGISTER_TIMEOUT_MS,
  SdkToServerMessageSchema,
} from '@aelio/protocol';
import { enqueueJob, upsertCustomerFlowProgress, upsertCustomerLifecycleState, syncToolEmbeddings } from '@aelio/core';
import type { WebSocket } from '@fastify/websocket';
import type { FastifyInstance } from 'fastify';
import { createHash, randomUUID } from 'node:crypto';
import type { RuntimeDeps } from '../runtime-deps.js';

function sendSdkError(
  socket: WebSocket,
  message: string,
  operation?: string,
): void {
  socket.send(JSON.stringify({ type: 'error', message, ...(operation ? { operation } : {}) }));
}

function inboundDedupKey(
  channel: string,
  from: string,
  text: string,
  messageId?: string,
): string {
  if (messageId) {
    return `${channel}:${messageId}`;
  }
  const hash = createHash('sha256').update(`${from}|${text}`).digest('hex').slice(0, 16);
  return `${channel}:fallback:${hash}`;
}

export async function registerSdkRoutes(
  app: FastifyInstance,
  deps: RuntimeDeps,
): Promise<{ stopHeartbeat: () => void }> {
  const { config, sdkBridge, database } = deps;
  const heartbeat = setInterval(() => {
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
  }, HEARTBEAT_INTERVAL_MS);
  heartbeat.unref();

  app.get(DEFAULT_SDK_PATH, { websocket: true }, (socket, request) => {
    // Preferred: Authorization header (never logged). Query-param `?secret=`
    // remains as a DEPRECATED fallback for older SDKs.
    const header = request.headers.authorization;
    const bearer = header?.startsWith('Bearer ') ? header.slice('Bearer '.length) : undefined;
    const secret =
      bearer ?? new URL(request.url, 'http://localhost').searchParams.get('secret');
    if (!secret || secret !== config.secret) {
      socket.close(1008, 'Invalid SDK secret');
      return;
    }
    if (!bearer) {
      app.log.warn(
        'SDK connected with ?secret= in the URL — deprecated; upgrade the SDK to send an Authorization header',
      );
    }

    if (sdkBridge.getConnectionCount() >= MAX_SDK_CONNECTIONS) {
      socket.close(1008, 'Too many SDK connections');
      return;
    }

    const connectionId = randomUUID();
    const ws = socket as WebSocket;
    let registered = false;

    const registerTimeout = setTimeout(() => {
      if (!registered) {
        socket.close(1008, 'Register timeout');
      }
    }, SDK_REGISTER_TIMEOUT_MS);
    registerTimeout.unref();

    const requireRegistered = (operation: string): boolean => {
      if (registered) {
        return true;
      }
      sendSdkError(ws, 'Send register before other SDK messages', operation);
      return false;
    };

    socket.on('message', (raw) => {
      const frame = raw.toString();
      if (frame.length > MAX_WS_FRAME_BYTES) {
        sendSdkError(ws, 'Message frame too large');
        return;
      }

      let parsed: unknown;
      try {
        parsed = JSON.parse(frame);
      } catch {
        sendSdkError(ws, 'Invalid JSON message');
        return;
      }

      const result = SdkToServerMessageSchema.safeParse(parsed);
      if (!result.success) {
        app.log.warn({ connectionId, issues: result.error.issues }, 'Invalid SDK message');
        sendSdkError(ws, 'Invalid message format');
        return;
      }

      const message = result.data;
      if (message.type === 'register') {
        sdkBridge.register({
          id: connectionId,
          socket: ws,
          functions: message.functions,
          states: message.states ?? [],
          policies: message.policies ?? [],
          flows: message.flows ?? [],
          persona: message.persona?.trim() || null,
          sdkVersion: message.sdkVersion,
          language: message.language,
          canSend: message.canSend ?? false,
          connectedAt: Date.now(),
          lastHeartbeatAt: Date.now(),
        });
        void syncToolEmbeddings(database.db, message.functions).catch((error) => {
          app.log.warn(
            { connectionId, err: error },
            'Failed to sync tool embeddings after SDK register',
          );
        });
        registered = true;
        clearTimeout(registerTimeout);

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
        if (!requireRegistered('set_state')) {
          return;
        }
        const connection = sdkBridge.getConnection(connectionId);
        const knownStates = connection?.states ?? [];
        if (!knownStates.some((state) => state.id === message.stateId)) {
          sendSdkError(
            ws,
            `Unknown stateId "${message.stateId}" — register it in the SDK catalog first`,
            'set_state',
          );
          return;
        }

        void upsertCustomerLifecycleState(
          database.db,
          message.customerId,
          message.stateId,
          message.reason,
        )
          .then(() => {
            app.log.info(
              {
                connectionId,
                customerId: message.customerId,
                stateId: message.stateId,
                reason: message.reason,
              },
              'Customer lifecycle state updated from SDK',
            );
            ws.send(
              JSON.stringify({ type: 'ack', operation: 'set_state', ok: true }),
            );
          })
          .catch((error: unknown) => {
            const msg = error instanceof Error ? error.message : 'Failed to update state';
            app.log.error({ connectionId, error: msg }, 'set_state failed');
            sendSdkError(ws, msg, 'set_state');
          });
        return;
      }

      if (message.type === 'set_flow_progress') {
        if (!requireRegistered('set_flow_progress')) {
          return;
        }
        const connection = sdkBridge.getConnection(connectionId);
        const knownFlows = connection?.flows ?? [];
        if (!knownFlows.some((flow) => flow.id === message.flowId)) {
          sendSdkError(
            ws,
            `Unknown flowId "${message.flowId}" — register it in the SDK catalog first`,
            'set_flow_progress',
          );
          return;
        }

        void upsertCustomerFlowProgress(
          database.db,
          message.customerId,
          message.flowId,
          message.stepIndex,
          message.completedSteps,
        )
          .then(() => {
            app.log.info(
              {
                connectionId,
                customerId: message.customerId,
                flowId: message.flowId,
                stepIndex: message.stepIndex,
              },
              'Customer flow progress updated from SDK',
            );
            ws.send(
              JSON.stringify({ type: 'ack', operation: 'set_flow_progress', ok: true }),
            );
          })
          .catch((error: unknown) => {
            const msg =
              error instanceof Error ? error.message : 'Failed to update flow progress';
            app.log.error({ connectionId, error: msg }, 'set_flow_progress failed');
            sendSdkError(ws, msg, 'set_flow_progress');
          });
        return;
      }

      if (message.type === 'pong') {
        if (registered) {
          sdkBridge.touchHeartbeat(connectionId);
        }
        return;
      }

      if (message.type === 'result') {
        if (!requireRegistered('result')) {
          return;
        }
        sdkBridge.handleResult(message);
        return;
      }

      if (message.type === 'ingest') {
        if (!requireRegistered('ingest')) {
          return;
        }
        const dedupKey = inboundDedupKey(
          message.channel,
          message.from,
          message.text,
          message.messageId,
        );
        if (!database.claimInboundMessage(dedupKey)) {
          app.log.info(
            { connectionId, messageId: message.messageId, dedupKey },
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
      clearTimeout(registerTimeout);
      sdkBridge.unregister(connectionId);
      app.log.info({ connectionId }, 'Aelio SDK disconnected');
    });
  });

  return { stopHeartbeat: () => clearInterval(heartbeat) };
}
