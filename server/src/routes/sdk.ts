import {
  DEFAULT_SDK_PATH,
  HEARTBEAT_INTERVAL_MS,
  MAX_SDK_CONNECTIONS,
  MAX_WS_FRAME_BYTES,
  SDK_REGISTER_TIMEOUT_MS,
  SdkToServerMessageSchema,
} from '@aelio/protocol';
import {
  enqueueJob,
  readSubjectState,
  resolveWhatsAppIdentity,
  upsertCustomerFlowProgress,
  upsertCustomerLifecycleState,
  type JsonValue,
} from '@aelio/core';
import type { WebSocket } from '@fastify/websocket';
import type { FastifyInstance } from 'fastify';
import { createHash, randomUUID } from 'node:crypto';
import { secretsMatch } from '../auth.js';
import type { RuntimeDeps } from '../runtime-deps.js';

/**
 * Merge one flow's progress into the durable subject snapshot without clobbering the others.
 * Read-modify-write is safe here: `patchSubjectState` does the merge under CAS with retry.
 */
async function patchRuntimeFlowProgress(
  deps: RuntimeDeps,
  customerId: string,
  flowId: string,
  stepIndex: number,
  completedSteps: string[] | undefined,
): Promise<void> {
  if (!deps.runtimeStore) return;
  const snapshot = await deps.runtimeStore.loadSnapshot(deps.config.name, customerId);
  const state = readSubjectState(snapshot?.state);
  await deps.runtimeStore.patchSubjectState(deps.config.name, customerId, {
    flowProgress: {
      ...state.flowProgress,
      [flowId]: { currentStepIndex: stepIndex, completedSteps: completedSteps ?? [] },
    } as unknown as JsonValue,
  });
}

function ingestDedupKey(channel: string, from: string, text: string, messageId?: string): string {
  if (messageId) {
    return `${channel}:${messageId}`;
  }
  const hash = createHash('sha256').update(`${channel}|${from}|${text}`).digest('hex').slice(0, 32);
  return `${channel}:hash:${hash}`;
}

export async function registerSdkRoutes(app: FastifyInstance, deps: RuntimeDeps) {
  const { config, sdkBridge, database } = deps;

  setInterval(() => {
    const now = Date.now();
    for (const connectionId of sdkBridge.getConnectionIds()) {
      const connection = sdkBridge.getConnection(connectionId);
      if (!connection) {
        continue;
      }

      if (!connection.registered && now - connection.connectedAt > SDK_REGISTER_TIMEOUT_MS) {
        connection.socket.close(1008, 'Register timeout');
        sdkBridge.unregister(connectionId);
        app.log.warn({ connectionId }, 'SDK connection closed — register not received in time');
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
    const header = request.headers.authorization;
    const bearer = header?.startsWith('Bearer ') ? header.slice('Bearer '.length) : undefined;
    const secret =
      bearer ?? new URL(request.url, 'http://localhost').searchParams.get('secret');
    if (!secret || !secretsMatch(secret, config.secret)) {
      app.log.warn(
        { route: DEFAULT_SDK_PATH, remoteAddress, usedAuthorizationHeader: Boolean(bearer) },
        'Rejected SDK websocket connection with invalid secret',
      );
      socket.close(1008, 'Invalid SDK secret');
      return;
    }
    if (!bearer) {
      app.log.warn('SDK connected with ?secret= in the URL — deprecated; upgrade the SDK to send an Authorization header');
    }

    if (sdkBridge.getConnectionIds().length >= MAX_SDK_CONNECTIONS) {
      const evicted = sdkBridge.evictOldestConnection();
      if (evicted) {
        app.log.warn({ evicted }, 'Evicted oldest SDK connection — at MAX_SDK_CONNECTIONS');
      }
    }

    const connectionId = randomUUID();
    sdkBridge.registerPending(connectionId, socket as WebSocket);

    app.log.info(
      { connectionId, route: DEFAULT_SDK_PATH, remoteAddress, usedAuthorizationHeader: Boolean(bearer) },
      'SDK websocket connection accepted',
    );

    socket.on('message', (raw) => {
      if (raw.toString().length > MAX_WS_FRAME_BYTES) {
        socket.send(
          JSON.stringify({ type: 'error', code: 'frame_too_large', message: 'Message exceeds frame size limit' }),
        );
        socket.close(1009, 'Frame too large');
        return;
      }

      let parsed: unknown;
      try {
        parsed = JSON.parse(raw.toString());
      } catch {
        socket.send(
          JSON.stringify({ type: 'error', code: 'invalid_json', message: 'Message was not valid JSON' }),
        );
        return;
      }

      const result = SdkToServerMessageSchema.safeParse(parsed);
      if (!result.success) {
        const detail = result.error.issues
          .map((issue) => `${issue.path.join('.') || '(root)'}: ${issue.message}`)
          .join('; ');
        socket.send(
          JSON.stringify({
            type: 'error',
            code: 'invalid_message',
            message: `Message failed validation: ${detail}`,
          }),
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
          },
          'Aelio SDK connected and registered catalog',
        );
        return;
      }

      if (!sdkBridge.isRegistered(connectionId)) {
        socket.send(
          JSON.stringify({
            type: 'error',
            code: 'not_registered',
            message: 'Send register before other messages',
          }),
        );
        return;
      }

      if (message.type === 'set_state') {
        // The tenant's backend is the authority on lifecycle, so the push has to land in BOTH
        // stores while they coexist: SQLite for the legacy turn, and the Aelio DB snapshot the
        // runtime conductor reads on the very next event.
        void Promise.all([
          upsertCustomerLifecycleState(database.db, message.customerId, message.stateId, message.reason),
          deps.runtimeStore?.patchSubjectState(config.name, message.customerId, {
            lifecycleState: message.stateId,
            lifecycleReason: message.reason ?? 'sdk push',
          }) ?? Promise.resolve(),
        ])
          .then(() => {
            socket.send(JSON.stringify({ type: 'ack', op: 'set_state' }));
          })
          .catch((error: unknown) => {
            socket.send(
              JSON.stringify({
                type: 'error',
                code: 'set_state_failed',
                message: error instanceof Error ? error.message : 'Failed to update state',
              }),
            );
          });
        return;
      }

      if (message.type === 'set_flow_progress') {
        void Promise.all([
          upsertCustomerFlowProgress(
            database.db,
            message.customerId,
            message.flowId,
            message.stepIndex,
            message.completedSteps,
          ),
          patchRuntimeFlowProgress(deps, message.customerId, message.flowId, message.stepIndex, message.completedSteps),
        ])
          .then(() => {
            socket.send(JSON.stringify({ type: 'ack', op: 'set_flow_progress' }));
          })
          .catch((error: unknown) => {
            socket.send(
              JSON.stringify({
                type: 'error',
                code: 'set_flow_progress_failed',
                message: error instanceof Error ? error.message : 'Failed to update flow progress',
              }),
            );
          });
        return;
      }

      if (message.type === 'pong') {
        sdkBridge.touchHeartbeat(connectionId);
        return;
      }

      if (message.type === 'result') {
        sdkBridge.handleResult(message);
        return;
      }

      if (message.type === 'ingest') {
        const dedupKey = ingestDedupKey(
          message.channel,
          message.from,
          message.text,
          message.messageId,
        );
        if (deps.runtimeStore) {
          // Aelio DB is the durable inbox. The schedule row is written before the ack, so a
          // crash after acking cannot lose the message, and the leased scheduler — not this
          // socket handler — owns the model/tool work.
          const identity = resolveWhatsAppIdentity(message.from);
          void deps.runtimeStore
            .scheduleEvent({
              scheduleId: `sdk:${dedupKey}`,
              tenantId: config.name,
              subjectId: message.channel === 'whatsapp' ? identity.externalId : message.from,
              dueAt: Date.now(),
              payload: {
                message: message.text,
                channel: 'sdk',
                delivery: { channel: message.channel, to: message.from },
              },
            })
            .then((scheduled) => {
              if (!scheduled) app.log.info({ connectionId, dedupKey }, 'Duplicate SDK ingest dropped');
              socket.send(JSON.stringify({ type: 'ack', op: 'ingest' }));
            })
            .catch((error: unknown) => {
              app.log.error({ err: error, connectionId, dedupKey }, 'SDK ingest could not be persisted');
              socket.send(
                JSON.stringify({ type: 'error', code: 'ingest_unavailable', message: 'Message was not accepted; retry.' }),
              );
            });
          return;
        }
        if (!database.claimInboundMessage(dedupKey)) {
          app.log.info({ connectionId, dedupKey }, 'Duplicate ingest dropped');
          socket.send(JSON.stringify({ type: 'ack', op: 'ingest' }));
          return;
        }
        void enqueueJob(database, 'inbound', {
          channel: message.channel,
          from: message.from,
          text: message.text,
          messageId: message.messageId,
          source: 'sdk-ingest',
        }).then(() => {
          socket.send(JSON.stringify({ type: 'ack', op: 'ingest' }));
        });
      }
    });

    socket.on('close', () => {
      sdkBridge.unregister(connectionId);
      app.log.info({ connectionId, remoteAddress }, 'Aelio SDK disconnected');
    });
  });
}
