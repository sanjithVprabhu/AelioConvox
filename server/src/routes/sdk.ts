import {
  DEFAULT_SDK_PATH,
  HEARTBEAT_INTERVAL_MS,
  MAX_SDK_CONNECTIONS,
  MAX_WS_FRAME_BYTES,
  SDK_REGISTER_TIMEOUT_MS,
  SdkToServerMessageSchema,
} from '@aelio/protocol';
import { enqueueJob, upsertCustomerLifecycleState } from '@aelio/core/edge';
import type { WebSocket } from '@fastify/websocket';
import type { FastifyInstance } from 'fastify';
import { createHash, randomUUID } from 'node:crypto';
import { buildAgentCatalog } from '../aelio-agent-catalog.js';
import { stableAgentUserId } from '../conversation-turn.js';
import { secretsMatch } from '../auth.js';
import type { RuntimeDeps } from '../runtime-deps.js';

function ingestDedupKey(channel: string, from: string, text: string, messageId?: string): string {
  if (messageId) {
    return `${channel}:${messageId}`;
  }
  const hash = createHash('sha256').update(`${channel}|${from}|${text}`).digest('hex').slice(0, 32);
  return `${channel}:hash:${hash}`;
}

export async function registerSdkRoutes(app: FastifyInstance, deps: RuntimeDeps) {
  const { config, sdkBridge } = deps;

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
    let registrationStarted = false;
    sdkBridge.registerPending(connectionId, socket as WebSocket);

    app.log.info(
      { connectionId, route: DEFAULT_SDK_PATH, remoteAddress, usedAuthorizationHeader: Boolean(bearer) },
      'SDK websocket connection accepted',
    );

    socket.on('message', async (raw: Buffer | ArrayBuffer | Buffer[]) => {
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
        if (registrationStarted) {
          socket.send(
            JSON.stringify({
              type: 'error',
              code: 'duplicate_register',
              message: 'This SDK connection already started catalog registration',
            }),
          );
          socket.close(1008, 'Duplicate registration');
          return;
        }
        registrationStarted = true;
        if (deps.aelioRuntime) {
          try {
            await deps.aelioRuntime.pushAgentCatalog(
              buildAgentCatalog(
                config.name,
                message,
                { memoryEnabled: config.memory.enabled },
              ),
            );
          } catch (error: unknown) {
            app.log.error(
              { err: error, connectionId },
              'SDK registration contained a flow rejected by the Rust runtime',
            );
            socket.send(
              JSON.stringify({
                type: 'error',
                code: 'aelio_flow_rejected',
                message: error instanceof Error ? error.message : 'Rust runtime rejected flow',
              }),
            );
            socket.close(1008, 'Aelio flow rejected');
            return;
          }
        }
        // Switch the callable host snapshot only after Rust has atomically admitted the exact
        // tools and compiled flow pins. A rejected upgrade therefore leaves the previous SDK
        // connection authoritative instead of creating a version-drift window.
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
        const commandId = `sdk-state:${createHash('sha256')
          .update(`${message.customerId}\u001f${message.stateId}\u001f${message.reason ?? ''}`)
          .digest('hex')}`;
        void Promise.all([
          deps.aelioRuntime.setAgentUserState({
            command_id: commandId,
            user_id: stableAgentUserId(message.customerId),
            state_id: message.stateId,
            ...(message.reason ? { reason: message.reason } : {}),
          }),
          // Keep the edge read model synchronized for admin/identity views. It is not an
          // execution authority; Rust commits the lifecycle command above.
          upsertCustomerLifecycleState(
            message.customerId,
            message.stateId,
            message.reason,
            deps.customerStore,
          ),
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
        void (async () => {
          const claimed = await deps.inboundDedupStore.claim(dedupKey);
          if (!claimed) {
            app.log.info({ connectionId, dedupKey }, 'Duplicate ingest dropped');
            socket.send(JSON.stringify({ type: 'ack', op: 'ingest' }));
            return;
          }
          await enqueueJob(
            'inbound',
            {
              channel: message.channel,
              from: message.from,
              text: message.text,
              messageId: message.messageId,
              source: 'sdk-ingest',
            },
            deps.jobStore,
          );
          socket.send(JSON.stringify({ type: 'ack', op: 'ingest' }));
        })();
      }
    });

    socket.on('close', () => {
      sdkBridge.unregister(connectionId);
      app.log.info({ connectionId, remoteAddress }, 'Aelio SDK disconnected');
    });
  });
}
