import {
  DEFAULT_SDK_PATH,
  HEARTBEAT_INTERVAL_MS,
  HEARTBEAT_TIMEOUT_MS,
  type Channel,
  type AelioFlowArtifact,
  type AelioPolicyArtifact,
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
   * Unique intent/capability this tool serves, in YOUR vocabulary (e.g. "order.status").
   * It drives deterministic tool resolution, so two tools may not claim the same label.
   * Defaults to the function name.
   */
  intent?: string;
  /**
   * Semantic allowlist for tool results. Only declared fields may become evidence,
   * enter memory, or be shown to an LLM. `path` defaults to the field name.
   */
  output?: Record<string, {
    path?: string;
    type?: 'auto' | 'string' | 'number' | 'boolean' | 'array' | 'object';
    sensitivity?: 'none' | 'pii' | 'secret';
    meaning: string;
  }>;
  outputRole?: 'data' | 'effect_confirmation' | 'continuation' | 'error';
};

export type StateGuardSchema = {
  requiresFields?: string[];
};

export type StateTransitionSchema = {
  /** Move to `to` when this tool succeeds (and the guard, if any, passes). */
  onToolSuccess: string;
  to: string;
  guard?: StateGuardSchema;
};

export type StateSchema = {
  description: string;
  allowedTools?: string[];
  blockedTools?: string[];
  /** Presence requirements for this state to apply (customer-profile fields). */
  guards?: StateGuardSchema;
  /** Declarative lifecycle transitions applied on tool success by the harness. */
  transitions?: StateTransitionSchema[];
};

export type PolicySchema = {
  description: string;
  severity?: 'hard' | 'soft';
  /** Closed rule enforced deterministically by the Rust runtime. */
  aelio?: AelioPolicyArtifact;
};

export type FlowStepSchema = {
  goal: string;
  tool?: string;
};

export type FlowSchema = {
  state: string;
  description: string;
  steps: Record<string, FlowStepSchema>;
  /** Closed program executed by the authoritative Rust Aelio runtime. */
  aelio?: AelioFlowArtifact;
};

type ExposedHandler = (args: Record<string, unknown>, ctx: InvocationContext) => Promise<unknown>;

type ListenOptions = {
  secret: string;
  url?: string;
  sdkVersion?: string;
};

export function registrationFingerprint(value: RegisterMessage): string {
  return JSON.stringify(canonicalJson(value));
}

function canonicalJson(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(canonicalJson);
  }
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, item]) => [key, canonicalJson(item)]),
    );
  }
  return value;
}

export class Aelio {
  private readonly handlers = new Map<string, { handler: ExposedHandler; schema: FunctionSchema }>();
  private readonly states = new Map<string, StateSchema>();
  private readonly policies = new Map<string, PolicySchema>();
  private readonly flows = new Map<string, FlowSchema>();
  private readonly invokeResults = new Map<string, ResultMessage>();
  private readonly inflightInvokeIds = new Set<string>();
  private sendHandler: SendHandler | null = null;
  private personaText: string | null = null;
  private personalitiesConfig: Array<{
    id: string;
    label?: string;
    voice: { register: string; verbosity: string; formality: string; emoji_policy: string };
    constraints?: string[];
    lexicon?: { preferred?: string[]; forbidden?: string[] };
  }> | null = null;
  private productBriefText: string | null = null;
  private applicationName: string | null = null;
  private catalogSyncTimer: ReturnType<typeof setTimeout> | null = null;
  private lastCatalogFingerprint: string | null = null;
  private sentCatalogFingerprint: string | null = null;
  private registrationInFlight = false;
  private ws: WebSocket | null = null;
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null;
  private lastPongAt = 0;
  private reconnectAttempt = 0;
  private listenOptions: ListenOptions | null = null;
  private shouldReconnect = false;
  /** Prevents overlapping connect()/reconnect storms that leave multiple live sockets. */
  private connectInFlight: Promise<void> | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;

  expose<T extends Record<string, unknown>, R>(
    name: string,
    handler: (args: T, ctx: InvocationContext) => Promise<R>,
    schema: FunctionSchema,
  ): void {
    this.handlers.set(name, {
      handler: handler as ExposedHandler,
      schema,
    });
    this.scheduleCatalogSync();
  }

  /**
   * Name shown in Aelio server connect logs (e.g. "Develup", "ShopCo").
   * Defaults to an unnamed SDK app if omitted.
   */
  application(name: string): void {
    this.applicationName = name.trim();
    this.scheduleCatalogSync();
  }

  /**
   * Set the assistant's persona/voice for your product (brand tone, naming,
   * standing instructions). Becomes the stable head of every system prompt.
   */
  persona(text: string): void {
    this.personaText = text.trim();
    this.scheduleCatalogSync();
  }

  /**
   * Register named personalities with audible voice traits. The first entry is
   * the default; tenants can switch by personality id. These must be visible in
   * how the assistant texts — register/formality/verbosity are enforced in prompt.
   */
  personalities(
    specs: Array<{
      id: string;
      label?: string;
      voice: { register: string; verbosity: string; formality: string; emoji_policy: string };
      constraints?: string[];
      lexicon?: { preferred?: string[]; forbidden?: string[] };
    }>,
  ): void {
    this.personalitiesConfig = specs
      .map((spec) => ({
        id: spec.id.trim(),
        ...(spec.label ? { label: spec.label.trim() } : {}),
        voice: {
          register: spec.voice.register.trim(),
          verbosity: spec.voice.verbosity.trim(),
          formality: spec.voice.formality.trim(),
          emoji_policy: spec.voice.emoji_policy.trim(),
        },
        ...(spec.constraints ? { constraints: spec.constraints.map((c) => c.trim()).filter(Boolean) } : {}),
        ...(spec.lexicon
          ? {
              lexicon: {
                preferred: (spec.lexicon.preferred ?? []).map((w) => w.trim()).filter(Boolean),
                forbidden: (spec.lexicon.forbidden ?? []).map((w) => w.trim()).filter(Boolean),
              },
            }
          : {}),
      }))
      .filter((spec) => spec.id.length > 0);
    this.scheduleCatalogSync();
  }

  /**
   * Describe what your product does, in your own words. Grounds the Aelio
   * harness planner: plans are drawn against this brief plus your registered
   * tools/flows, so a clear description improves both what the assistant
   * attempts and how gracefully it declines the impossible.
   */
  describe(text: string): void {
    this.productBriefText = text.trim();
    this.scheduleCatalogSync();
  }

  /**
   * Declare a customer lifecycle state. The SaaS backend sets the active state
   * per customer via setCustomerState(); Aelio keeps conversations within the
   * state's boundaries until the stage changes.
   */
  state(id: string, schema: StateSchema): void {
    this.states.set(id, schema);
    this.scheduleCatalogSync();
  }

  /** Register a conversation policy enforced on every turn. */
  policy(id: string, schema: PolicySchema): void {
    this.policies.set(id, schema);
    this.scheduleCatalogSync();
  }

  /**
   * Register a guided multi-step flow scoped to a lifecycle state.
   * steps is keyed by step id, e.g. { connect: { goal: '...' }, invite: { goal: '...' } }
   */
  flow(id: string, schema: FlowSchema): void {
    this.flows.set(id, schema);
    this.scheduleCatalogSync();
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

  /**
   * Optional flow-progress hint for multi-step journeys. Current Convox edge
   * treats lifecycle `set_state` as the authority for tool admission; this is
   * retained for shopping/testbed compatibility and is a no-op wire call until
   * `set_flow_progress` is re-admitted on the server protocol.
   */
  setFlowProgress(
    customerId: string,
    flowId: string,
    stepIndex: number,
    completedSteps: string[] = [],
  ): void {
    void customerId;
    void flowId;
    void stepIndex;
    void completedSteps;
  }

  /**
   * Register a delivery handler so Aelio can send outbound messages through your
   * own messaging provider (bring-your-own WhatsApp/SMS/etc.). Aelio invokes this
   * automatically after each turn — it is NOT an LLM tool.
   */
  onSend(handler: SendHandler): void {
    this.sendHandler = handler;
    this.scheduleCatalogSync();
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
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.clearHeartbeat();
    this.registrationInFlight = false;
    this.sentCatalogFingerprint = null;
    if (this.ws) {
      const previous = this.ws;
      this.ws = null;
      previous.close();
    }
  }

  private async connect(): Promise<void> {
    if (!this.listenOptions) {
      throw new Error('listen() must be called before connecting');
    }
    if (this.connectInFlight) {
      return this.connectInFlight;
    }

    this.connectInFlight = this.openSocket().finally(() => {
      this.connectInFlight = null;
    });
    return this.connectInFlight;
  }

  private async openSocket(): Promise<void> {
    const baseUrl =
      this.listenOptions!.url ??
      `ws://127.0.0.1:${process.env.AELIO_PORT ?? process.env.AELIO_SERVER_PORT ?? '3010'}`;
    const url = new URL(DEFAULT_SDK_PATH, baseUrl);

    // Tear down any prior socket before opening a new one. Otherwise stale
    // close handlers null `this.ws` while a newer connection is live, drop
    // invoke results, and schedule more reconnects (connection fan-out).
    if (this.ws) {
      const previous = this.ws;
      this.ws = null;
      this.clearHeartbeat();
      try {
        previous.close();
      } catch {
        /* ignore */
      }
    }

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
        void this.handleMessage(raw.toString(), ws);
      });

      ws.on('close', () => {
        // Only the active socket may clear state / reconnect. A superseded
        // socket's close must not clobber a newer connection.
        if (this.ws !== ws) {
          return;
        }
        this.clearHeartbeat();
        this.registrationInFlight = false;
        this.sentCatalogFingerprint = null;
        this.ws = null;
        if (this.shouldReconnect) {
          void this.scheduleReconnect();
        }
      });
    });
  }

  private sendRegister(): void {
    if (this.registrationInFlight) {
      return;
    }
    const message = this.buildRegisterMessage();
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      return;
    }
    this.registrationInFlight = true;
    this.sentCatalogFingerprint = registrationFingerprint(message);
    this.sendOn(this.ws, message);
  }

  private buildRegisterMessage(): RegisterMessage {
    const functions: FunctionDefinition[] = [...this.handlers.entries()].map(([name, entry]) => ({
      name,
      description: entry.schema.description,
      params: entry.schema.params,
      safety: entry.schema.safety,
      ...(entry.schema.intent ? { intent: entry.schema.intent } : {}),
      ...(entry.schema.output
        ? {
            output: Object.fromEntries(
              Object.entries(entry.schema.output).map(([field, spec]) => [
                field,
                {
                  ...spec,
                  type: spec.type ?? 'auto',
                  sensitivity: spec.sensitivity ?? 'none',
                },
              ]),
            ),
          }
        : {}),
      ...(entry.schema.outputRole ? { outputRole: entry.schema.outputRole } : {}),
    }));

    const states: StateDefinition[] = [...this.states.entries()].map(([id, entry]) => ({
      id,
      description: entry.description,
      ...(entry.allowedTools ? { allowedTools: entry.allowedTools } : {}),
      ...(entry.blockedTools ? { blockedTools: entry.blockedTools } : {}),
      ...(entry.guards?.requiresFields
        ? { guards: { requires_fields: entry.guards.requiresFields } }
        : {}),
      ...(entry.transitions
        ? {
            transitions: entry.transitions.map((transition) => ({
              on_tool_success: transition.onToolSuccess,
              to: transition.to,
              ...(transition.guard?.requiresFields
                ? { guard: { requires_fields: transition.guard.requiresFields } }
                : {}),
            })),
          }
        : {}),
    }));

    const policies: PolicyDefinition[] = [...this.policies.entries()].map(([id, entry]) => ({
      id,
      description: entry.description,
      severity: entry.severity ?? 'soft',
      ...(entry.aelio ? { aelio: entry.aelio } : {}),
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
      ...(entry.aelio ? { aelio: entry.aelio } : {}),
    }));

    const message: RegisterMessage = {
      type: 'register',
      sdkVersion: this.listenOptions?.sdkVersion ?? '0.1.0',
      language: 'node',
      ...(this.applicationName ? { application: this.applicationName } : {}),
      functions,
      ...(states.length > 0 ? { states } : {}),
      ...(policies.length > 0 ? { policies } : {}),
      ...(flows.length > 0 ? { flows } : {}),
      ...(this.personaText ? { persona: this.personaText } : {}),
      ...(this.personalitiesConfig && this.personalitiesConfig.length > 0
        ? { personalities: this.personalitiesConfig }
        : {}),
      ...(this.productBriefText ? { productBrief: this.productBriefText } : {}),
      canSend: this.sendHandler != null,
    };

    return message;
  }

  private async handleMessage(raw: string, socket: WebSocket): Promise<void> {
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
      this.handlePing(message, socket);
      const app = this.applicationName ?? 'unnamed SDK app';
      const ts = new Date().toISOString().slice(11, 19);
      console.log(
        `♥ [${ts}] heartbeat from Aelio — ${app} ` +
          `(tools=${this.handlers.size} states=${this.states.size} ` +
          `flows=${this.flows.size} policies=${this.policies.size})`,
      );
      return;
    }

    if (message.type === 'invoke') {
      await this.handleInvoke(message, socket);
      return;
    }

    if (message.type === 'send') {
      await this.handleSend(message, socket);
      return;
    }

    if (message.type === 'error') {
      // A protocol-level complaint from the server (e.g. a malformed frame we
      // sent). Surface it so integrators can see why a message had no effect.
      console.warn(`[aelio-sdk] server error [${message.code}]: ${message.message}`);
      this.registrationInFlight = false;
      return;
    }

    if (message.type === 'registered') {
      const border = '════════════════════════════════════════════════════════';
      const bullet = (label: string, items: string[]) =>
        items.length === 0
          ? [`  ${label} (0): (none)`]
          : [`  ${label} (${items.length}):`, ...items.map((item) => `    • ${item}`)];
      if (message.reason === 'catalog_update') {
        const change = (label: string, added?: string[], removed?: string[]) => {
          const lines: string[] = [];
          if (added?.length) lines.push(`  + ${label}: ${added.join(', ')}`);
          if (removed?.length) lines.push(`  − ${label}: ${removed.join(', ')}`);
          return lines;
        };
        console.log(
          [
            border,
            `  ↻ ${message.application} catalog updated on Aelio`,
            `  tenant:     ${message.tenant}`,
            '',
            ...change('tools', message.added?.tools, message.removed?.tools),
            ...change('states', message.added?.states, message.removed?.states),
            ...change('flows', message.added?.flows, message.removed?.flows),
            ...change('policies', message.added?.policies, message.removed?.policies),
            '',
            ...bullet('tools now', message.tools),
            ...bullet('states now', message.states),
            ...bullet('flows now', message.flows),
            ...bullet('policies now', message.policies),
            border,
          ].join('\n'),
        );
      } else {
        console.log(
          [
            border,
            `  ✓ ${message.application} successfully connected to Aelio`,
            `  tenant:     ${message.tenant}`,
            '',
            ...bullet('tools', message.tools),
            ...bullet('states', message.states),
            ...bullet('flows', message.flows),
            ...bullet('policies', message.policies),
            border,
          ].join('\n'),
        );
      }
      this.registrationInFlight = false;
      this.lastCatalogFingerprint = this.sentCatalogFingerprint;
      this.sentCatalogFingerprint = null;
      if (this.catalogFingerprint() !== this.lastCatalogFingerprint) {
        this.scheduleCatalogSync();
      }
    }
  }

  private catalogFingerprint(): string {
    return registrationFingerprint(this.buildRegisterMessage());
  }

  private scheduleCatalogSync(): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      return;
    }
    if (this.catalogSyncTimer) {
      clearTimeout(this.catalogSyncTimer);
    }
    this.catalogSyncTimer = setTimeout(() => {
      this.catalogSyncTimer = null;
      const next = this.catalogFingerprint();
      if (next === this.lastCatalogFingerprint || this.registrationInFlight) {
        return;
      }
      this.sendRegister();
    }, 250);
  }

  private handlePing(message: PingMessage, socket: WebSocket): void {
    this.lastPongAt = Date.now();
    this.sendOn(socket, { type: 'pong', ts: message.ts });
  }

  private async handleSend(message: SendInvokeMessage, socket: WebSocket): Promise<void> {
    const started = Date.now();

    if (!this.sendHandler) {
      this.sendOn(socket, {
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
      this.sendOn(socket, {
        type: 'result',
        id: message.id,
        ok: true,
        durationMs: Date.now() - started,
      });
    } catch (error) {
      this.sendOn(socket, {
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

  private async handleInvoke(message: InvokeMessage, socket: WebSocket): Promise<void> {
    const cached = this.invokeResults.get(message.id);
    if (cached) {
      this.sendOn(socket, cached);
      return;
    }
    if (this.inflightInvokeIds.has(message.id)) {
      return;
    }
    this.inflightInvokeIds.add(message.id);
    const started = Date.now();
    const entry = this.handlers.get(message.function);

    if (!entry) {
      this.finishInvoke(
        {
          type: 'result',
          id: message.id,
          ok: false,
          error: {
            code: 'FUNCTION_NOT_FOUND',
            message: `Function "${message.function}" is not registered`,
          },
          durationMs: Date.now() - started,
        },
        socket,
      );
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
      this.finishInvoke(response, socket);
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
      this.finishInvoke(response, socket);
    }
  }

  private finishInvoke(response: ResultMessage, socket: WebSocket): void {
    this.inflightInvokeIds.delete(response.id);
    if (this.invokeResults.size >= 10_000) {
      const oldest = this.invokeResults.keys().next().value;
      if (oldest) this.invokeResults.delete(oldest);
    }
    this.invokeResults.set(response.id, response);
    this.sendOn(socket, response);
  }

  private send(message: Parameters<typeof SdkToServerMessageSchema.parse>[0]): void {
    this.sendOn(this.ws, message);
  }

  private sendOn(
    socket: WebSocket | null,
    message: Parameters<typeof SdkToServerMessageSchema.parse>[0],
  ): void {
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      // Dropped rather than queued (the server is the source of truth and will
      // re-request on reconnect). Warn so a caller pushing state/ingest while
      // disconnected isn't left wondering why nothing happened. `result` and
      // `pong` are responses to server prompts — noisy and safe to drop quietly.
      const type = (message as { type?: string }).type;
      if (type !== 'result' && type !== 'pong') {
        console.warn(
          `[aelio-sdk] not connected — dropped "${type}" message. It was not delivered to the Aelio server.`,
        );
      }
      return;
    }
    socket.send(JSON.stringify(message));
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
    if (this.reconnectTimer || this.connectInFlight) {
      return;
    }
    const delay = Math.min(30_000, 1_000 * 2 ** this.reconnectAttempt);
    this.reconnectAttempt += 1;
    await new Promise<void>((resolve) => {
      this.reconnectTimer = setTimeout(() => {
        this.reconnectTimer = null;
        resolve();
      }, delay);
    });
    if (!this.shouldReconnect) {
      return;
    }
    try {
      await this.connect();
    } catch {
      void this.scheduleReconnect();
    }
  }
}

export const aelio = new Aelio();
