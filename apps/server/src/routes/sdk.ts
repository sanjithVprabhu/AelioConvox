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
    const secret = new URL(request.url, 'http://localhost').searchParams.get('secret');
    if (!secret || secret !== config.secret) {
      socket.close(1008, 'Invalid SDK secret');
      return;
    }

    const connectionId = randomUUID();

    socket.on('message', (raw) => {
      let parsed: unknown;
      try {
        parsed = JSON.parse(raw.toString());
      } catch {
        return;
      }

      const result = SdkToServerMessageSchema.safeParse(parsed);
      if (!result.success) {
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
        return;
      }

      if (message.type === 'result') {
        sdkBridge.handleResult(message);
        return;
      }

      if (message.type === 'ingest') {
        // The dev's own channel webhook handed us an inbound message. Queue it
        // for the inbound worker exactly like a built-in channel would.
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
      app.log.info({ connectionId }, 'Aelio SDK disconnected');
    });
  });
}