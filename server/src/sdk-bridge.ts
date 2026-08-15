import {
  INVOKE_TIMEOUT_MS,
  type AttributeDefinition,
  type Channel,
  type FlowDefinition,
  type FunctionDefinition,
  type InvocationContext,
  type InvokeMessage,
  type PipelineManifest,
  type PolicyDefinition,
  type ResultMessage,
  type SendInvokeMessage,
  type StateDefinition,
} from '@aelio/protocol';
import type { SdkBridge, SdkInvokeResult } from '@aelio/core';
import type { AelioDatabase } from '@aelio/db';
import type { WebSocket } from '@fastify/websocket';
import { randomUUID } from 'node:crypto';

type ActiveConnection = {
  id: string;
  socket: WebSocket;
  functions: FunctionDefinition[];
  states: StateDefinition[];
  policies: PolicyDefinition[];
  flows: FlowDefinition[];
  pipeline: PipelineManifest | null;
  attributes: AttributeDefinition[];
  persona: string | null;
  productBrief: string | null;
  sdkVersion: string;
  language: string;
  canSend: boolean;
  connectedAt: number;
  lastHeartbeatAt: number;
  registered: boolean;
};

type PendingInvoke = {
  resolve: (result: ResultMessage) => void;
  reject: (error: Error) => void;
  timeout: ReturnType<typeof setTimeout>;
};

export class ServerSdkBridge implements SdkBridge {
  private readonly connections = new Map<string, ActiveConnection>();
  private readonly pendingInvokes = new Map<string, PendingInvoke>();
  private readonly persistConnectionStmt;
  private readonly deleteConnectionStmt;
  private readonly registryListeners = new Set<() => void>();

  constructor(private readonly database: AelioDatabase) {
    this.persistConnectionStmt = this.database.sqlite.prepare(
      `INSERT OR REPLACE INTO sdk_connections
       (id, connection_token, sdk_version, language, connected_at, last_heartbeat_at, functions)
       VALUES (?, ?, ?, ?, ?, ?, ?)`,
    );
    this.deleteConnectionStmt = this.database.sqlite.prepare('DELETE FROM sdk_connections WHERE id = ?');
    // Prune stale rows from prior boots instead of wiping the whole table.
    const staleCutoff = Date.now() - 2 * 60_000;
    this.database.sqlite
      .prepare('DELETE FROM sdk_connections WHERE last_heartbeat_at < ?')
      .run(staleCutoff);
  }

  onRegistryChange(listener: () => void): () => void {
    this.registryListeners.add(listener);
    return () => this.registryListeners.delete(listener);
  }

  private notifyRegistryChange(): void {
    for (const listener of this.registryListeners) {
      try {
        listener();
      } catch (error) {
        console.error('[aelio] registry-change listener failed:', error);
      }
    }
  }

  /** Accept a socket before `register` — tracked for register-timeout enforcement. */
  registerPending(connectionId: string, socket: WebSocket): void {
    const now = Date.now();
    this.connections.set(connectionId, {
      id: connectionId,
      socket,
      functions: [],
      states: [],
      policies: [],
      flows: [],
      pipeline: null,
      attributes: [],
      persona: null,
      productBrief: null,
      sdkVersion: '',
      language: 'node',
      canSend: false,
      connectedAt: now,
      lastHeartbeatAt: now,
      registered: false,
    });
  }

  isRegistered(connectionId: string): boolean {
    return this.connections.get(connectionId)?.registered ?? false;
  }

  register(connection: Omit<ActiveConnection, 'registered'>): void {
    const full: ActiveConnection = { ...connection, registered: true };
    this.connections.set(connection.id, full);
    this.persist(full);
    this.notifyRegistryChange();
  }

  unregister(connectionId: string): void {
    this.connections.delete(connectionId);
    this.deleteConnectionStmt.run(connectionId);
    this.notifyRegistryChange();
  }

  /** Evict the connection with the oldest heartbeat (SDK-008). */
  evictOldestConnection(): string | null {
    let oldest: ActiveConnection | null = null;
    for (const connection of this.connections.values()) {
      if (!oldest || connection.lastHeartbeatAt < oldest.lastHeartbeatAt) {
        oldest = connection;
      }
    }
    if (!oldest) {
      return null;
    }
    oldest.socket.close(1008, 'Connection limit reached');
    this.unregister(oldest.id);
    return oldest.id;
  }

  shutdown(): void {
    for (const pending of this.pendingInvokes.values()) {
      clearTimeout(pending.timeout);
      pending.reject(new Error('Server shutting down'));
    }
    this.pendingInvokes.clear();
    for (const connection of this.connections.values()) {
      connection.socket.close(1001, 'Server shutting down');
    }
    this.connections.clear();
  }

  touchHeartbeat(connectionId: string): void {
    const connection = this.connections.get(connectionId);
    if (connection) {
      connection.lastHeartbeatAt = Date.now();
      if (connection.registered) {
        this.persist(connection);
      }
    }
  }

  updateFunctions(connectionId: string, functions: FunctionDefinition[]): void {
    const connection = this.connections.get(connectionId);
    if (connection) {
      connection.functions = functions;
      this.persist(connection);
      this.notifyRegistryChange();
    }
  }

  private persist(connection: ActiveConnection): void {
    this.persistConnectionStmt.run(
      connection.id,
      connection.id,
      connection.sdkVersion,
      connection.language,
      connection.connectedAt,
      connection.lastHeartbeatAt,
      JSON.stringify(connection.functions),
    );
  }

  getConnectionIds(): string[] {
    return [...this.connections.keys()];
  }

  getConnection(connectionId: string): (ActiveConnection & { registered: boolean }) | undefined {
    return this.connections.get(connectionId);
  }

  getFunctions(): FunctionDefinition[] {
    const seen = new Map<string, FunctionDefinition>();
    for (const connection of this.registeredConnections()) {
      for (const fn of connection.functions) {
        seen.set(fn.name, fn);
      }
    }
    return [...seen.values()];
  }

  getStates(): StateDefinition[] {
    const seen = new Map<string, StateDefinition>();
    for (const connection of this.registeredConnections()) {
      for (const state of connection.states) {
        seen.set(state.id, state);
      }
    }
    return [...seen.values()];
  }

  getPolicies(): PolicyDefinition[] {
    const seen = new Map<string, PolicyDefinition>();
    for (const connection of this.registeredConnections()) {
      for (const policy of connection.policies) {
        seen.set(policy.id, policy);
      }
    }
    return [...seen.values()];
  }

  getPersona(): string | null {
    for (const connection of this.registeredConnections()) {
      if (connection.persona) {
        return connection.persona;
      }
    }
    return null;
  }

  getProductBrief(): string | null {
    for (const connection of this.registeredConnections()) {
      if (connection.productBrief) {
        return connection.productBrief;
      }
    }
    return null;
  }

  getFlows(): FlowDefinition[] {
    const seen = new Map<string, FlowDefinition>();
    for (const connection of this.registeredConnections()) {
      for (const flow of connection.flows) {
        seen.set(flow.id, flow);
      }
    }
    return [...seen.values()];
  }

  getPipelineManifest(): PipelineManifest | null {
    for (const connection of this.registeredConnections()) {
      if (connection.pipeline) {
        return connection.pipeline;
      }
    }
    return null;
  }

  getAttributes(): AttributeDefinition[] {
    const seen = new Map<string, AttributeDefinition>();
    for (const connection of this.registeredConnections()) {
      for (const attribute of connection.attributes) {
        seen.set(attribute.id, attribute);
      }
    }
    return [...seen.values()];
  }

  private registeredConnections(): ActiveConnection[] {
    return [...this.connections.values()].filter((entry) => entry.registered);
  }

  handleResult(message: ResultMessage): void {
    const pending = this.pendingInvokes.get(message.id);
    if (!pending) {
      return;
    }
    clearTimeout(pending.timeout);
    this.pendingInvokes.delete(message.id);
    pending.resolve(message);
  }

  hasSendCapability(): boolean {
    return this.registeredConnections().some((entry) => entry.canSend);
  }

  async sendViaChannel(
    channel: Channel,
    to: string,
    content: string,
    metadata?: Record<string, unknown>,
  ): Promise<SdkInvokeResult> {
    const connection = [...this.registeredConnections()]
      .filter((entry) => entry.canSend)
      .sort((a, b) => b.lastHeartbeatAt - a.lastHeartbeatAt)[0];
    if (!connection) {
      return { ok: false, error: 'No connected SDK can deliver outbound messages', durationMs: 0 };
    }

    const id = randomUUID();
    const sendMessage: SendInvokeMessage = {
      type: 'send',
      id,
      channel,
      to,
      content,
      ...(metadata ? { metadata } : {}),
    };
    const started = Date.now();

    try {
      const result = await new Promise<ResultMessage>((resolve, reject) => {
        const timeout = setTimeout(() => {
          this.pendingInvokes.delete(id);
          reject(new Error(`SDK send timed out after ${INVOKE_TIMEOUT_MS}ms`));
        }, INVOKE_TIMEOUT_MS);
        this.pendingInvokes.set(id, { resolve, reject, timeout });
        connection.socket.send(JSON.stringify(sendMessage));
      });

      return {
        ok: result.ok,
        data: result.data,
        error: result.error?.message,
        durationMs: result.durationMs || Date.now() - started,
      };
    } catch (error) {
      return {
        ok: false,
        error: error instanceof Error ? error.message : 'SDK send failed',
        durationMs: Date.now() - started,
      };
    }
  }

  async invoke(
    functionName: string,
    args: Record<string, unknown>,
    context: InvocationContext,
  ): Promise<SdkInvokeResult> {
    const connection = [...this.registeredConnections()]
      .filter((entry) => entry.functions.some((fn) => fn.name === functionName))
      .sort((a, b) => b.lastHeartbeatAt - a.lastHeartbeatAt)[0];

    if (!connection) {
      return {
        ok: false,
        error: `No connected SDK exposes function "${functionName}"`,
        durationMs: 0,
      };
    }

    const id = randomUUID();
    const invokeMessage: InvokeMessage = {
      type: 'invoke',
      id,
      function: functionName,
      args,
      context,
    };

    const started = Date.now();

    try {
      const result = await new Promise<ResultMessage>((resolve, reject) => {
        const timeout = setTimeout(() => {
          this.pendingInvokes.delete(id);
          reject(new Error(`SDK invoke timed out after ${INVOKE_TIMEOUT_MS}ms`));
        }, INVOKE_TIMEOUT_MS);

        this.pendingInvokes.set(id, { resolve, reject, timeout });
        connection.socket.send(JSON.stringify(invokeMessage));
      });

      return {
        ok: result.ok,
        data: result.data,
        error: result.error?.message,
        durationMs: result.durationMs || Date.now() - started,
      };
    } catch (error) {
      return {
        ok: false,
        error: error instanceof Error ? error.message : 'SDK invoke failed',
        durationMs: Date.now() - started,
      };
    }
  }
}
