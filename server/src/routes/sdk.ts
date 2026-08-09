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
import {
  logSdkCatalogPersist,
  logSdkCatalogUpdate,
  logSdkDisconnected,
  logSdkHeartbeat,
  logSdkRegistrationSuccess,
  diffCatalogNames,
} from '../sdk-registration-log.js';
import { refreshCatalogBagAfterSync } from '../catalog-bag.js';

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
        const isUpdate = registrationStarted && sdkBridge.isRegistered(connectionId);
        if (registrationStarted && !isUpdate) {
          // Register in flight / failed mid-way — don't accept a second concurrent attempt.
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
        if (!registrationStarted) {
          registrationStarted = true;
        }

        const prior = isUpdate ? sdkBridge.getConnection(connectionId) : undefined;
        const beforeNames = {
          tools: (prior?.functions ?? []).map((fn) => fn.name),
          states: (prior?.states ?? []).map((s) => s.id),
          flows: (prior?.flows ?? []).map((f) => f.id),
          policies: (prior?.policies ?? []).map((p) => p.id),
        };
        const afterNames = {
          tools: message.functions.map((fn) => fn.name),
          states: (message.states ?? []).map((s) => s.id),
          flows: (message.flows ?? []).map((f) => f.id),
          policies: (message.policies ?? []).map((p) => p.id),
        };
        const { added, removed, changed } = diffCatalogNames(beforeNames, afterNames);
        if (isUpdate && !changed) {
          // No capability delta — still ack so the SDK clears its sync timer.
          socket.send(
            JSON.stringify({
              type: 'registered',
              application: prior?.application ?? config.name,
              tenant: config.name,
              connectionId,
              tools: afterNames.tools,
              states: afterNames.states,
              flows: afterNames.flows,
              policies: afterNames.policies,
              reason: 'catalog_update',
              added,
              removed,
            }),
          );
          return;
        }

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
              { err: error, connectionId, isUpdate },
              'SDK registration contained a flow rejected by the Rust runtime',
            );
            socket.send(
              JSON.stringify({
                type: 'error',
                code: 'aelio_flow_rejected',
                message: error instanceof Error ? error.message : 'Rust runtime rejected flow',
              }),
            );
            if (!isUpdate) {
              socket.close(1008, 'Aelio flow rejected');
            }
            return;
          }
        }
        // Switch the callable host snapshot only after Rust has atomically admitted the exact
        // tools and compiled flow pins. A rejected upgrade therefore leaves the previous SDK
        // connection authoritative instead of creating a version-drift window.
        const application =
          message.application?.trim()
          || message.productBrief?.split('\n').find((line) => line.trim())?.trim()?.slice(0, 128)
          || prior?.application
          || config.name
          || 'unnamed SDK app';
        sdkBridge.register({
          id: connectionId,
          socket: socket as WebSocket,
          application,
          functions: message.functions,
          states: message.states ?? [],
          policies: message.policies ?? [],
          flows: message.flows ?? [],
          persona: message.persona?.trim() || null,
          productBrief: message.productBrief?.trim() || null,
          sdkVersion: message.sdkVersion,
          language: message.language,
          canSend: message.canSend ?? false,
          connectedAt: prior?.connectedAt ?? Date.now(),
          lastHeartbeatAt: Date.now(),
        });

        // Durable soft-delete catalog + hot bag: present → active=true; missing → active=false.
        try {
          const syncResult = await deps.catalogEntityStore.syncSnapshot({
            tenant: config.name,
            application,
            connectionId,
            tools: message.functions.map((fn) => ({ ...fn, name: fn.name })),
            states: (message.states ?? []).map((s) => ({ ...s, id: s.id })),
            policies: (message.policies ?? []).map((p) => ({ ...p, id: p.id })),
            flows: (message.flows ?? []).map((f) => ({ ...f, id: f.id })),
          });
          await refreshCatalogBagAfterSync(deps.catalogEntityStore, config.name, application);
          logSdkCatalogPersist({
            application,
            tenant: config.name,
            activated: syncResult.activated,
            deactivated: syncResult.deactivated,
          });
        } catch (error: unknown) {
          app.log.error(
            { err: error, connectionId, application },
            'SDK catalog soft-delete persist failed',
          );
        }

        if (isUpdate) {
          logSdkCatalogUpdate({
            application,
            tenant: config.name,
            connectionId,
            after: afterNames,
            added,
            removed,
          });
        } else {
          logSdkRegistrationSuccess({
            application,
            tenant: config.name,
            connectionId,
            remoteAddress,
            sdkVersion: message.sdkVersion,
            language: message.language,
            tools: message.functions.map((fn) => ({
              name: fn.name,
              description: fn.description,
            })),
            states: (message.states ?? []).map((state) => ({
              id: state.id,
              description: state.description,
            })),
            flows: (message.flows ?? []).map((flow) => ({
              id: flow.id,
              description: flow.description,
              state: flow.state,
            })),
            policies: (message.policies ?? []).map((policy) => ({
              id: policy.id,
              description: policy.description,
            })),
          });
        }
        socket.send(
          JSON.stringify({
            type: 'registered',
            application,
            tenant: config.name,
            connectionId,
            tools: afterNames.tools,
            states: afterNames.states,
            flows: afterNames.flows,
            policies: afterNames.policies,
            reason: isUpdate ? 'catalog_update' : 'connect',
            ...(isUpdate ? { added, removed } : {}),
          }),
        );
        app.log.info(
          {
            connectionId,
            application,
            reason: isUpdate ? 'catalog_update' : 'connect',
            sdkVersion: message.sdkVersion,
            language: message.language,
            functions: afterNames.tools,
            states: afterNames.states,
            flows: afterNames.flows,
            policies: afterNames.policies,
          },
          isUpdate
            ? 'Aelio SDK catalog updated'
            : 'Aelio SDK connected and registered catalog',
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
        const connection = sdkBridge.getConnection(connectionId);
        if (connection?.registered) {
          logSdkHeartbeat({
            application: connection.application,
            connectionId,
            tools: connection.functions.length,
            states: connection.states.length,
            flows: connection.flows.length,
            policies: connection.policies.length,
          });
        }
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
      const prior = sdkBridge.getConnection(connectionId);
      const application = prior?.application ?? '(unknown app)';
      sdkBridge.unregister(connectionId);
      if (prior?.registered) {
        logSdkDisconnected(application, connectionId);
      }
      app.log.info({ connectionId, application, remoteAddress }, 'Aelio SDK disconnected');
    });
  });
}
