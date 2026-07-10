import {
  DEFAULT_SDK_PATH,
  HEARTBEAT_INTERVAL_MS,
  HEARTBEAT_TIMEOUT_MS,
  type Channel,
  type FlowDefinition,
  type FunctionDefinition,
  type IngestMessage,
  type InvocationContext,
  type InvokeMessage,
  type PingMessage,
  type PolicyDefinition,
  type RegisterMessage,
  type ResultMessage,
  type SafetyLevel,
  type SendInvokeMessage,
  type StateDefinition,
  SdkToServerMessageSchema,
  ServerToSdkMessageSchema,
} from '@aelio/protocol';
import WebSocket from 'ws';

export type { Channel, InvocationContext, SafetyLevel };

/** A reply Aelio is asking the SDK to deliver via the dev's own provider. */
export type OutboundDelivery = {
  channel: Channel;
  to: string;
  content: string;
  metadata?: Record<string, unknown>;
};

/** An inbound message the dev received on their own channel webhook. */
export type InboundIngest = {
  channel: Channel;
  from: string;
  text: string;
  messageId?: string;
  metadata?: Record<string, unknown>;
};

type SendHandler = (delivery: OutboundDelivery) => Promise<void>;

/** JSON-schema-ish primitive types a parameter can declare. */
export type ParamType = 'string' | 'number' | 'integer' | 'boolean' | 'array' | 'object';

/**
 * How a single parameter is declared. Three interchangeable forms:
 *   - `'string'`             → required string
 *   - `'string?'`            → optional string (trailing `?`)
 *   - `{ type, description?, optional?, enum?, format?, items? }` → full control
 */
export type ParamSpec =
  | ParamType
  | `${ParamType}?`
  | {
      type: ParamType;
      description?: string;
      optional?: boolean;
      enum?: Array<string | number>;
      format?: string;
      items?: ParamSpec;
      properties?: Record<string, ParamSpec>;
    };

export type FunctionSchema = {
  description: string;
  params: Record<string, ParamSpec>;
  safety: SafetyLevel;
  /**
   * Intent category this tool serves, in YOUR vocabulary (e.g. "order_inquiry").
   * Drives the runtime's conversation-intent tracking and smart tool selection.
   * Defaults to the function name.
   */
  intent?: string;
};

export type StateSchema = {
  description: string;
  allowedTools?: string[];
  blockedTools?: string[];
};

export type PolicySchema = {
  description: string;
  severity?: 'hard' | 'soft';
};

export type FlowStepSchema = {
  goal: string;
  tool?: string;
};

export type FlowSchema = {
  state: string;
  description: string;
  steps: Record<string, FlowStepSchema>;
};

type ExposedHandler = (args: Record<string, unknown>, ctx: InvocationContext) => Promise<unknown>;

type ListenOptions = {
  secret: string;
  url?: string;
  sdkVersion?: string;
};

export class Aelio {
  private readonly handlers = new Map<string, { handler: ExposedHandler; schema: FunctionSchema }>();
  private readonly states = new Map<string, StateSchema>();
  private readonly policies = new Map<string, PolicySchema>();
  private readonly flows = new Map<string, FlowSchema>();
  private sendHandler: SendHandler | null = null;
  private personaText: string | null = null;
  private ws: WebSocket | null = null;
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null;
  private lastPongAt = 0;
  private reconnectAttempt = 0;
  private listenOptions: ListenOptions | null = null;
  private shouldReconnect = false;
  private reconnecting = false;

  expose<T extends Record<string, unknown>, R>(
    name: string,
    handler: (args: T, ctx: InvocationContext) => Promise<R>,
    schema: FunctionSchema,
  ): void {
    this.handlers.set(name, {
      handler: handler as ExposedHandler,
      schema,
    });
  }

  /**
   * Set the assistant's persona/voice for your product (brand tone, naming,
   * standing instructions). Becomes the stable head of every system prompt.
   */
  persona(text: string): void {
    this.personaText = text.trim();
  }

  /**
   * Declare a customer lifecycle state. The SaaS backend sets the active state
   * per customer via setCustomerState(); Aelio keeps conversations within the
   * state's boundaries until the stage changes.
   */
  state(id: string, schema: StateSchema): void {
    this.states.set(id, schema);
  }

  /** Register a conversation policy enforced on every turn. */
  policy(id: string, schema: PolicySchema): void {
    this.policies.set(id, schema);
  }

  /**
   * Register a guided multi-step flow scoped to a lifecycle state.
   * steps is keyed by step id, e.g. { connect: { goal: '...' }, invite: { goal: '...' } }
   */
  flow(id: string, schema: FlowSchema): void {
    this.flows.set(id, schema);
  }

  /** Push the current lifecycle state for a customer (your DB is source of truth). */
  setCustomerState(customerId: string, stateId: string, reason?: string): void {
    this.send({
      type: 'set_state',
      customerId,
      stateId,
      ...(reason ? { reason } : {}),
    });
  }

  /** Update guided-flow progress for a customer. */
  setFlowProgress(
    customerId: string,
    flowId: string,
    stepIndex: number,
    completedSteps?: string[],
  ): void {
    this.send({
      type: 'set_flow_progress',
      customerId,
      flowId,
      stepIndex,
      ...(completedSteps ? { completedSteps } : {}),
    });
  }

  /**
   * Register a delivery handler so Aelio can send outbound messages through your
   * own messaging provider (bring-your-own WhatsApp/SMS/etc.). Aelio invokes this
   * automatically after each turn — it is NOT an LLM tool.
   */
  onSend(handler: SendHandler): void {
    this.sendHandler = handler;
  }

  /**
   * Hand Aelio an inbound message you received on your own channel webhook.
   * You own the webhook + signature verification + provider parsing; Aelio takes
   * over identity, memory, the LLM turn, safety, and the reply (via onSend).
   */
  ingest(message: InboundIngest): void {
    this.send({
      type: 'ingest',
      channel: message.channel,
      from: message.from,
      text: message.text,
      ...(message.messageId ? { messageId: message.messageId } : {}),
      ...(message.metadata ? { metadata: message.metadata } : {}),
    } satisfies IngestMessage);
  }

  async listen(opts: ListenOptions): Promise<void> {
    this.listenOptions = opts;
    this.shouldReconnect = true;
    await this.connect();
  }

  async disconnect(): Promise<void> {
    this.shouldReconnect = false;
    this.clearHeartbeat();
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
  }

  private async connect(): Promise<void> {
    if (!this.listenOptions) {
      throw new Error('listen() must be called before connecting');
    }

    const baseUrl = this.listenOptions.url ?? 'ws://127.0.0.1:3000';
    const url = new URL(DEFAULT_SDK_PATH, baseUrl);

    await new Promise<void>((resolve, reject) => {
      // The secret travels as an Authorization header, never in the URL —
      // query strings end up in server request logs and proxies.
      const ws = new WebSocket(url, {
        headers: { authorization: `Bearer ${this.listenOptions!.secret}` },
      });

      ws.once('open', () => {
        this.ws = ws;
        this.reconnectAttempt = 0;
        this.lastPongAt = Date.now();
        this.sendRegister();
        this.startHeartbeat();
        resolve();
      });

      ws.once('error', (error) => {
        reject(error);
      });

      ws.on('message', (raw) => {
        void this.handleMessage(raw.toString());
      });

      ws.on('close', () => {
        this.clearHeartbeat();
        this.ws = null;
        if (this.shouldReconnect) {
          void this.scheduleReconnect();
        }
      });
    });
  }

  private sendRegister(): void {
    const functions: FunctionDefinition[] = [...this.handlers.entries()].map(([name, entry]) => ({
      name,
      description: entry.schema.description,
      params: entry.schema.params,
      safety: entry.schema.safety,
      ...(entry.schema.intent ? { intent: entry.schema.intent } : {}),
    }));

    const states: StateDefinition[] = [...this.states.entries()].map(([id, entry]) => ({
      id,
      description: entry.description,
      ...(entry.allowedTools ? { allowedTools: entry.allowedTools } : {}),
      ...(entry.blockedTools ? { blockedTools: entry.blockedTools } : {}),
    }));

    const policies: PolicyDefinition[] = [...this.policies.entries()].map(([id, entry]) => ({
      id,
      description: entry.description,
      severity: entry.severity ?? 'soft',
    }));

    const flows: FlowDefinition[] = [...this.flows.entries()].map(([id, entry]) => ({
      id,
      state: entry.state,
      description: entry.description,
      steps: Object.entries(entry.steps).map(([stepId, step]) => ({
        id: stepId,
        goal: step.goal,
        ...(step.tool ? { tool: step.tool } : {}),
      })),
    }));

    const message: RegisterMessage = {
      type: 'register',
      sdkVersion: this.listenOptions?.sdkVersion ?? '0.1.0',
      language: 'node',
      functions,
      ...(states.length > 0 ? { states } : {}),
      ...(policies.length > 0 ? { policies } : {}),
      ...(flows.length > 0 ? { flows } : {}),
      ...(this.personaText ? { persona: this.personaText } : {}),
      canSend: this.sendHandler != null,
    };

    this.send(message);
  }

  private async handleMessage(raw: string): Promise<void> {
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch {
      return;
    }

    const result = ServerToSdkMessageSchema.safeParse(parsed);
    if (!result.success) {
      return;
    }

    const message = result.data;
    if (message.type === 'ping') {
      this.handlePing(message);
      return;
    }

    if (message.type === 'error') {
      console.error(
        `[aelio-sdk] server error${message.operation ? ` (${message.operation})` : ''}: ${message.message}`,
      );
      return;
    }

    if (message.type === 'ack') {
      if (!message.ok) {
        console.error(
          `[aelio-sdk] ${message.operation} failed: ${message.message ?? 'unknown error'}`,
        );
      }
      return;
    }

    if (message.type === 'invoke') {
      await this.handleInvoke(message);
      return;
    }

    if (message.type === 'send') {
      await this.handleSend(message);
    }
  }

  private handlePing(message: PingMessage): void {
    this.lastPongAt = Date.now();
    this.send({ type: 'pong', ts: message.ts });
  }

  private async handleSend(message: SendInvokeMessage): Promise<void> {
    const started = Date.now();

    if (!this.sendHandler) {
      this.send({
        type: 'result',
        id: message.id,
        ok: false,
        error: { code: 'NO_SEND_HANDLER', message: 'No onSend handler is registered' },
        durationMs: Date.now() - started,
      });
      return;
    }

    try {
      await this.sendHandler({
        channel: message.channel,
        to: message.to,
        content: message.content,
        ...(message.metadata ? { metadata: message.metadata } : {}),
      });
      this.send({ type: 'result', id: message.id, ok: true, durationMs: Date.now() - started });
    } catch (error) {
      this.send({
        type: 'result',
        id: message.id,
        ok: false,
        error: {
          code: 'SEND_FAILED',
          message: error instanceof Error ? error.message : 'Send handler failed',
          retryable: true,
        },
        durationMs: Date.now() - started,
      });
    }
  }

  private async handleInvoke(message: InvokeMessage): Promise<void> {
    const started = Date.now();
    const entry = this.handlers.get(message.function);

    if (!entry) {
      this.send({
        type: 'result',
        id: message.id,
        ok: false,
        error: {
          code: 'FUNCTION_NOT_FOUND',
          message: `Function "${message.function}" is not registered`,
        },
        durationMs: Date.now() - started,
      });
      return;
    }

    try {
      const data = await entry.handler(message.args, message.context);
      const response: ResultMessage = {
        type: 'result',
        id: message.id,
        ok: true,
        data,
        durationMs: Date.now() - started,
      };
      this.send(response);
    } catch (error) {
      const response: ResultMessage = {
        type: 'result',
        id: message.id,
        ok: false,
        error: {
          code: 'HANDLER_ERROR',
          message: error instanceof Error ? error.message : 'Unknown handler error',
          retryable: false,
        },
        durationMs: Date.now() - started,
      };
      this.send(response);
    }
  }

  private send(message: Parameters<typeof SdkToServerMessageSchema.parse>[0]): boolean {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      console.warn('[aelio-sdk] WebSocket not connected — message dropped');
      return false;
    }
    this.ws.send(JSON.stringify(message));
    return true;
  }

  private startHeartbeat(): void {
    this.clearHeartbeat();
    this.heartbeatTimer = setInterval(() => {
      if (Date.now() - this.lastPongAt > HEARTBEAT_TIMEOUT_MS) {
        this.ws?.terminate();
        return;
      }
    }, HEARTBEAT_INTERVAL_MS);
  }

  private clearHeartbeat(): void {
    if (this.heartbeatTimer) {
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }

  private async scheduleReconnect(): Promise<void> {
    if (this.reconnecting) {
      return;
    }
    this.reconnecting = true;
    const delay = Math.min(30_000, 1_000 * 2 ** this.reconnectAttempt);
    this.reconnectAttempt += 1;
    await new Promise((resolve) => setTimeout(resolve, delay));
    if (!this.shouldReconnect) {
      this.reconnecting = false;
      return;
    }
    try {
      await this.connect();
    } catch {
      this.reconnecting = false;
      void this.scheduleReconnect();
      return;
    }
    this.reconnecting = false;
  }
}

export const aelio = new Aelio();