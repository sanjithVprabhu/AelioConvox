import {
  INVOKE_TIMEOUT_MS,
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
import type { ConvoxSdkConnectionStore, SdkBridge, SdkInvokeResult } from '@aelio/core/edge';
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
  private readonly registryListeners = new Set<() => void>();
  private readonly correlatedInvokes = new Map<string, Promise<SdkInvokeResult>>();
  private readonly completedInvokes = new Map<string, { at: number; result: SdkInvokeResult }>();

  constructor(private readonly sdkConnectionStore: ConvoxSdkConnectionStore) {
    // Prune stale rows from prior boots instead of wiping the whole table.
    void this.sdkConnectionStore.pruneStale(Date.now() - 2 * 60_000).catch((error) => {
      console.error('[aelio] AelioDb sdk_connections prune failed:', error);
    });
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
    void this.sdkConnectionStore.remove(connectionId).catch((error) => {
      console.error('[aelio] AelioDb sdk_connections remove failed:', error);
    });
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
    void this.sdkConnectionStore
      .upsert({
        id: connection.id,
        sdkVersion: connection.sdkVersion,
        language: connection.language,
        connectedAt: connection.connectedAt,
        lastHeartbeatAt: connection.lastHeartbeatAt,
        functions: connection.functions as unknown as Array<Record<string, unknown>>,
      })
      .catch((error) => {
        console.error('[aelio] AelioDb sdk_connections upsert failed:', error);
      });
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
    return this.invokeCorrelated(functionName, args, context, randomUUID());
  }

  /**
   * Invoke with the kernel-issued logical call identity. Concurrent/retried deliveries join the
   * same promise and completed results are replayed from a bounded cache.
   */
  async invokeCorrelated(
    functionName: string,
    args: Record<string, unknown>,
    context: InvocationContext,
    correlationId: string,
  ): Promise<SdkInvokeResult> {
    const now = Date.now();
    for (const [id, entry] of this.completedInvokes) {
      if (now - entry.at > 10 * 60_000) this.completedInvokes.delete(id);
    }
    const completed = this.completedInvokes.get(correlationId);
    if (completed) return completed.result;
    const active = this.correlatedInvokes.get(correlationId);
    if (active) return active;

    const invocation = this.invokeOnce(functionName, args, context, correlationId);
    this.correlatedInvokes.set(correlationId, invocation);
    try {
      const result = await invocation;
      if (this.completedInvokes.size >= 10_000) {
        const oldest = this.completedInvokes.keys().next().value;
        if (oldest) this.completedInvokes.delete(oldest);
      }
      this.completedInvokes.set(correlationId, { at: Date.now(), result });
      return result;
    } finally {
      this.correlatedInvokes.delete(correlationId);
    }
  }

  private async invokeOnce(
    functionName: string,
    args: Record<string, unknown>,
    context: InvocationContext,
    id: string,
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
