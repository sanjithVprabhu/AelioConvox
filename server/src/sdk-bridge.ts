import {
  INVOKE_TIMEOUT_MS,
  MAX_SDK_CONNECTIONS,
  type Channel,
  type FlowDefinition,
  type FunctionDefinition,
  type InvocationContext,
  type InvokeMessage,
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
  persona: string | null;
  sdkVersion: string;
  language: string;
  canSend: boolean;
  connectedAt: number;
  lastHeartbeatAt: number;
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

  constructor(private readonly database: AelioDatabase) {
    this.persistConnectionStmt = this.database.sqlite.prepare(
      `INSERT OR REPLACE INTO sdk_connections
       (id, connection_token, sdk_version, language, connected_at, last_heartbeat_at, functions)
       VALUES (?, ?, ?, ?, ?, ?, ?)`,
    );
    this.deleteConnectionStmt = this.database.sqlite.prepare('DELETE FROM sdk_connections WHERE id = ?');
    // Keep connection metadata across restarts for ops visibility. Live sockets
    // are gone after a restart — SDKs must re-register — but we no longer wipe
    // the table. Stale rows (no heartbeat for 2 minutes) are pruned.
    const staleBefore = Date.now() - 2 * 60_000;
    this.database.sqlite
      .prepare('DELETE FROM sdk_connections WHERE last_heartbeat_at < ?')
      .run(staleBefore);
  }

  register(connection: ActiveConnection): void {
    if (this.connections.size >= MAX_SDK_CONNECTIONS) {
      connection.socket.close(1008, 'Too many SDK connections');
      return;
    }
    this.connections.set(connection.id, connection);
    this.persist(connection);
  }

  getConnectionCount(): number {
    return this.connections.size;
  }

  shutdown(): void {
    for (const [connectionId, connection] of this.connections) {
      connection.socket.close(1000, 'Server shutting down');
      this.unregister(connectionId);
    }
    for (const [id, pending] of this.pendingInvokes) {
      clearTimeout(pending.timeout);
      pending.reject(new Error('Server shutting down'));
      this.pendingInvokes.delete(id);
    }
  }

  private pickConnection(
    predicate: (entry: ActiveConnection) => boolean,
  ): ActiveConnection | undefined {
    return [...this.connections.values()]
      .filter(predicate)
      .sort((a, b) => b.lastHeartbeatAt - a.lastHeartbeatAt)[0];
  }

  unregister(connectionId: string): void {
    this.connections.delete(connectionId);
    this.deleteConnectionStmt.run(connectionId);
  }

  touchHeartbeat(connectionId: string): void {
    const connection = this.connections.get(connectionId);
    if (connection) {
      connection.lastHeartbeatAt = Date.now();
      this.persist(connection);
    }
  }

  updateFunctions(connectionId: string, functions: FunctionDefinition[]): void {
    const connection = this.connections.get(connectionId);
    if (connection) {
      connection.functions = functions;
      this.persist(connection);
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

  getConnection(connectionId: string): ActiveConnection | undefined {
    return this.connections.get(connectionId);
  }

  getFunctions(): FunctionDefinition[] {
    const seen = new Map<string, FunctionDefinition>();
    for (const connection of this.connections.values()) {
      for (const fn of connection.functions) {
        seen.set(fn.name, fn);
      }
    }
    return [...seen.values()];
  }

  getStates(): StateDefinition[] {
    const seen = new Map<string, StateDefinition>();
    for (const connection of this.connections.values()) {
      for (const state of connection.states) {
        seen.set(state.id, state);
      }
    }
    return [...seen.values()];
  }

  getPolicies(): PolicyDefinition[] {
    const seen = new Map<string, PolicyDefinition>();
    for (const connection of this.connections.values()) {
      for (const policy of connection.policies) {
        seen.set(policy.id, policy);
      }
    }
    return [...seen.values()];
  }

  getPersona(): string | null {
    for (const connection of this.connections.values()) {
      if (connection.persona) {
        return connection.persona;
      }
    }
    return null;
  }

  getFlows(): FlowDefinition[] {
    const seen = new Map<string, FlowDefinition>();
    for (const connection of this.connections.values()) {
      for (const flow of connection.flows) {
        seen.set(flow.id, flow);
      }
    }
    return [...seen.values()];
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

  /** True when some connected SDK can deliver outbound messages itself. */
  hasSendCapability(): boolean {
    return [...this.connections.values()].some((entry) => entry.canSend);
  }

  /**
   * Deliver an outbound message through a connected SDK's onSend handler
   * (bring-your-own provider). Correlated by id, same as invoke().
   */
  async sendViaChannel(
    channel: Channel,
    to: string,
    content: string,
    metadata?: Record<string, unknown>,
  ): Promise<SdkInvokeResult> {
    const connection = this.pickConnection((entry) => entry.canSend);
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
    const connection = this.pickConnection((entry) =>
      entry.functions.some((fn) => fn.name === functionName),
    );

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
