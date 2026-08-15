import {
  DEFAULT_SDK_PATH,
  HEARTBEAT_INTERVAL_MS,
  HEARTBEAT_TIMEOUT_MS,
  expandToolAccess,
  type Channel,
  type AttributeDefinition,
  type FlowDefinition,
  type FunctionDefinition,
  type IngestMessage,
  type InvocationContext,
  type InvokeMessage,
  type PipelineManifest,
  type PingMessage,
  type PolicyDefinition,
  type RegisterMessage,
  type ResultMessage,
  type SafetyLevel,
  type SendInvokeMessage,
  type StateDefinition,
  type ToolGroupDefinition,
  SdkToServerMessageSchema,
  ServerToSdkMessageSchema,
} from '@aelio/protocol';
import WebSocket from 'ws';

export type { Channel, InvocationContext, SafetyLevel, ToolGroupDefinition };
export { expandToolAccess };

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

function pickDefinedAccess(
  expanded: ReturnType<typeof expandToolAccess>,
): {
  allowedTools?: string[];
  blockedTools?: string[];
  allowedIntents?: string[];
  blockedIntents?: string[];
  allowedSafety?: SafetyLevel[];
  blockedSafety?: SafetyLevel[];
} {
  return {
    ...(expanded.allowedTools ? { allowedTools: expanded.allowedTools } : {}),
    ...(expanded.blockedTools ? { blockedTools: expanded.blockedTools } : {}),
    ...(expanded.allowedIntents ? { allowedIntents: expanded.allowedIntents } : {}),
    ...(expanded.blockedIntents ? { blockedIntents: expanded.blockedIntents } : {}),
    ...(expanded.allowedSafety ? { allowedSafety: expanded.allowedSafety } : {}),
    ...(expanded.blockedSafety ? { blockedSafety: expanded.blockedSafety } : {}),
  };
}

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
  /** Admit tools by their schema `intent` category instead of enumerating names. */
  allowedIntents?: string[];
  blockedIntents?: string[];
  /** Admit or block whole safety classes. Block rules take precedence. */
  allowedSafety?: SafetyLevel[];
  blockedSafety?: SafetyLevel[];
  /** Reuse named buckets from `pipeline({ toolGroups })` / `toolGroups(...)`. */
  allowedGroups?: string[];
  blockedGroups?: string[];
  /** Presence requirements for this state to apply (customer-profile fields). */
  guards?: StateGuardSchema;
  /** Declarative lifecycle transitions applied on tool success by the harness. */
  transitions?: StateTransitionSchema[];
};

export type PolicySchema = {
  description: string;
  severity?: 'hard' | 'soft';
};

export type FlowStepSchema = {
  goal: string;
  tool?: string;
  type?: 'tool' | 'attribute' | 'content';
  attribute?: string;
  uiFormat?: {
    component: string;
    config?: Record<string, unknown>;
  };
  skipIfPresent?: string;
};

export type FlowSchema = {
  state: string;
  description: string;
  steps: Record<string, FlowStepSchema>;
};

export type AttributeSchema = {
  label: string;
  dataType?: 'string' | 'number' | 'boolean' | 'object' | 'array';
  sensitivityTier?: 'public' | 'pii' | 'sensitive_regulated';
  prompts?: string[];
  uiFormat?: {
    component: string;
    config?: Record<string, unknown>;
  };
  enumValues?: Array<string | number>;
};

export type PipelineStageSchema = {
  description: string;
  content?: {
    greeting?: string;
    cta?: string;
  };
  flow?: string;
  allowedTools?: string[];
  blockedTools?: string[];
  allowedIntents?: string[];
  blockedIntents?: string[];
  allowedSafety?: SafetyLevel[];
  blockedSafety?: SafetyLevel[];
  /** Reuse named buckets from pipeline `toolGroups`. Mix with individual tools freely. */
  allowedGroups?: string[];
  blockedGroups?: string[];
  guards?: StateGuardSchema;
  next?: string;
};

export type PipelineSchema = {
  initialStage?: string;
  /** Named reusable allow/block buckets referenced by stages via allowedGroups/blockedGroups. */
  toolGroups?: Record<string, ToolGroupDefinition>;
  stages: Record<string, PipelineStageSchema>;
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
  private pipelineManifest: PipelineSchema | null = null;
  private toolGroupCatalog: Record<string, ToolGroupDefinition> = {};
  private readonly attributes = new Map<string, AttributeSchema>();
  private sendHandler: SendHandler | null = null;
  private personaText: string | null = null;
  private productBriefText: string | null = null;
  private ws: WebSocket | null = null;
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null;
  private lastPongAt = 0;
  private reconnectAttempt = 0;
  private listenOptions: ListenOptions | null = null;
  private shouldReconnect = false;

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
   * Describe what your product does, in your own words. Grounds the Aelio
   * harness planner: plans are drawn against this brief plus your registered
   * tools/flows, so a clear description improves both what the assistant
   * attempts and how gracefully it declines the impossible.
   */
  describe(text: string): void {
    this.productBriefText = text.trim();
  }

  /**
   * Declare a customer lifecycle state. The SaaS backend sets the active state
   * per customer via setCustomerState(); Aelio keeps conversations within the
   * state's boundaries until the stage changes.
   * `allowedGroups` / `blockedGroups` expand against the catalog from
   * `toolGroups(...)` or `pipeline({ toolGroups })`.
   */
  state(id: string, schema: StateSchema): void {
    this.states.set(id, this.expandAccessSchema(schema));
  }

  /** Register reusable tool-access buckets for `allowedGroups` / `blockedGroups`. */
  toolGroups(groups: Record<string, ToolGroupDefinition>): void {
    this.toolGroupCatalog = { ...this.toolGroupCatalog, ...groups };
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

  /**
   * Declare the conversational pipeline: global stages, flows, and transitions.
   * Aelio runs this deterministically — the agent guides users through each step.
   * Optional `toolGroups` define reusable allow/block buckets referenced by
   * stage `allowedGroups` / `blockedGroups` (mixable with individual tools).
   */
  pipeline(schema: PipelineSchema): void {
    if (schema.toolGroups) {
      this.toolGroupCatalog = { ...this.toolGroupCatalog, ...schema.toolGroups };
    }
    const stages: Record<string, PipelineStageSchema> = {};
    for (const [id, stage] of Object.entries(schema.stages)) {
      const { allowedGroups: _ag, blockedGroups: _bg, ...rest } = stage;
      const expanded = expandToolAccess(this.toolGroupCatalog, stage);
      stages[id] = {
        ...rest,
        ...pickDefinedAccess(expanded),
      };
    }
    this.pipelineManifest = {
      initialStage: schema.initialStage,
      stages,
    };
  }

  private expandAccessSchema<T extends StateSchema | PipelineStageSchema>(schema: T): T {
    const expanded = expandToolAccess(this.toolGroupCatalog, schema);
    const { allowedGroups: _ag, blockedGroups: _bg, ...rest } = schema;
    return {
      ...rest,
      ...pickDefinedAccess(expanded),
    } as T;
  }

  /** Register a profile attribute the pipeline can collect during onboarding. */
  attribute(id: string, schema: AttributeSchema): void {
    this.attributes.set(id, schema);
  }

  /** Push the customer's global pipeline stage (verified, onboarding, active, …). */
  setGlobalStage(customerId: string, stage: string, reason?: string): void {
    this.send({
      type: 'set_global_stage',
      customerId,
      stage,
      ...(reason ? { reason } : {}),
    });
  }

  /** Push a collected profile attribute for a customer. */
  setAttribute(
    customerId: string,
    attributeId: string,
    value: unknown,
    options?: { source?: 'explicit_ask' | 'inferred' | 'third_party_auth'; verified?: boolean },
  ): void {
    this.send({
      type: 'set_attribute',
      customerId,
      attributeId,
      value,
      ...(options?.source ? { source: options.source } : {}),
      ...(options?.verified !== undefined ? { verified: options.verified } : {}),
    });
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

    const baseUrl =
      this.listenOptions.url ??
      `ws://127.0.0.1:${process.env.AELIO_PORT ?? process.env.AELIO_SERVER_PORT ?? '3010'}`;
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

    // Pipeline stages are lifecycle states by definition. Derive them automatically so a
    // YAML-backed pipeline does not need a second, duplicated lifecycle_states section.
    // An explicitly registered state with the same id remains authoritative.
    const stateEntries = new Map(this.states);
    for (const [id, stage] of Object.entries(this.pipelineManifest?.stages ?? {})) {
      if (!stateEntries.has(id)) {
        stateEntries.set(id, {
          description: stage.description,
          ...(stage.allowedTools ? { allowedTools: stage.allowedTools } : {}),
          ...(stage.blockedTools ? { blockedTools: stage.blockedTools } : {}),
          ...(stage.allowedIntents ? { allowedIntents: stage.allowedIntents } : {}),
          ...(stage.blockedIntents ? { blockedIntents: stage.blockedIntents } : {}),
          ...(stage.allowedSafety ? { allowedSafety: stage.allowedSafety } : {}),
          ...(stage.blockedSafety ? { blockedSafety: stage.blockedSafety } : {}),
          ...(stage.guards ? { guards: stage.guards } : {}),
        });
      }
    }

    const states: StateDefinition[] = [...stateEntries.entries()].map(([id, entry]) => ({
      id,
      description: entry.description,
      ...(entry.allowedTools ? { allowedTools: entry.allowedTools } : {}),
      ...(entry.blockedTools ? { blockedTools: entry.blockedTools } : {}),
      ...(entry.allowedIntents ? { allowedIntents: entry.allowedIntents } : {}),
      ...(entry.blockedIntents ? { blockedIntents: entry.blockedIntents } : {}),
      ...(entry.allowedSafety ? { allowedSafety: entry.allowedSafety } : {}),
      ...(entry.blockedSafety ? { blockedSafety: entry.blockedSafety } : {}),
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
    }));

    const flows: FlowDefinition[] = [...this.flows.entries()].map(([id, entry]) => ({
      id,
      state: entry.state,
      description: entry.description,
      steps: Object.entries(entry.steps).map(([stepId, step]) => ({
        id: stepId,
        goal: step.goal,
        ...(step.tool ? { tool: step.tool } : {}),
        ...(step.type ? { type: step.type } : {}),
        ...(step.attribute ? { attribute: step.attribute } : {}),
        ...(step.uiFormat ? { ui_format: step.uiFormat } : {}),
        ...(step.skipIfPresent ? { skip_if_present: step.skipIfPresent } : {}),
      })),
    }));

    const attributes: AttributeDefinition[] = [...this.attributes.entries()].map(
      ([id, entry]) => ({
        id,
        label: entry.label,
        data_type: entry.dataType ?? 'string',
        sensitivity_tier: entry.sensitivityTier ?? 'pii',
        ...(entry.prompts ? { prompts: entry.prompts } : {}),
        ...(entry.uiFormat ? { ui_format: entry.uiFormat } : {}),
        ...(entry.enumValues ? { enum_values: entry.enumValues } : {}),
      }),
    );

    const pipeline: PipelineManifest | undefined = this.pipelineManifest
      ? {
          initial_stage: this.pipelineManifest.initialStage ?? 'unverified',
          stages: Object.fromEntries(
            Object.entries(this.pipelineManifest.stages).map(([stageId, stage]) => [
              stageId,
              {
                id: stageId,
                description: stage.description,
                ...(stage.content ? { content: stage.content } : {}),
                ...(stage.flow ? { flow: stage.flow } : {}),
                ...(stage.allowedTools ? { allowedTools: stage.allowedTools } : {}),
                ...(stage.blockedTools ? { blockedTools: stage.blockedTools } : {}),
                ...(stage.allowedIntents ? { allowedIntents: stage.allowedIntents } : {}),
                ...(stage.blockedIntents ? { blockedIntents: stage.blockedIntents } : {}),
                ...(stage.allowedSafety ? { allowedSafety: stage.allowedSafety } : {}),
                ...(stage.blockedSafety ? { blockedSafety: stage.blockedSafety } : {}),
                ...(stage.guards?.requiresFields
                  ? { guards: { requires_fields: stage.guards.requiresFields } }
                  : {}),
                ...(stage.next ? { next: stage.next } : {}),
              },
            ]),
          ),
        }
      : undefined;

    const message: RegisterMessage = {
      type: 'register',
      sdkVersion: this.listenOptions?.sdkVersion ?? '0.1.0',
      language: 'node',
      functions,
      ...(states.length > 0 ? { states } : {}),
      ...(policies.length > 0 ? { policies } : {}),
      ...(flows.length > 0 ? { flows } : {}),
      ...(pipeline ? { pipeline } : {}),
      ...(attributes.length > 0 ? { attributes } : {}),
      ...(this.personaText ? { persona: this.personaText } : {}),
      ...(this.productBriefText ? { productBrief: this.productBriefText } : {}),
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

    if (message.type === 'invoke') {
      await this.handleInvoke(message);
      return;
    }

    if (message.type === 'send') {
      await this.handleSend(message);
      return;
    }

    if (message.type === 'error') {
      // A protocol-level complaint from the server (e.g. a malformed frame we
      // sent). Surface it so integrators can see why a message had no effect.
      console.warn(`[aelio-sdk] server error [${message.code}]: ${message.message}`);
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

  private send(message: Parameters<typeof SdkToServerMessageSchema.parse>[0]): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
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
    this.ws.send(JSON.stringify(message));
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
    const delay = Math.min(30_000, 1_000 * 2 ** this.reconnectAttempt);
    this.reconnectAttempt += 1;
    await new Promise((resolve) => setTimeout(resolve, delay));
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
