import { z } from 'zod';

export const SafetyLevelSchema = z.enum(['read', 'write', 'destructive']);
export type SafetyLevel = z.infer<typeof SafetyLevelSchema>;

export const SdkLanguageSchema = z.enum(['node', 'python', 'go']);
export type SdkLanguage = z.infer<typeof SdkLanguageSchema>;

// A channel is a free-form identifier so devs can bring any transport
// (whatsapp, web, telegram, slack, sms, custom…). 'whatsapp' and 'web' are the
// built-in ones; anything else is delivered via the SDK's onSend handler.
export const ChannelSchema = z.string().min(1);
export type Channel = z.infer<typeof ChannelSchema>;
export type KnownChannel = 'whatsapp' | 'web';

export const OutputFieldDefinitionSchema = z.object({
  path: z.string().min(1).optional(),
  type: z.enum(['auto', 'string', 'number', 'boolean', 'array', 'object']).default('auto'),
  sensitivity: z.enum(['none', 'pii', 'secret']).default('none'),
  meaning: z.string().min(1),
}).strict();
export type OutputFieldDefinition = z.infer<typeof OutputFieldDefinitionSchema>;

export const FunctionDefinitionSchema = z.object({
  name: z.string().min(1),
  description: z.string().min(1),
  params: z.record(z.unknown()),
  safety: SafetyLevelSchema,
  // Client-declared executable intent (e.g. "order.status"). It must resolve to
  // exactly one tool; the core never hardcodes domain topics. Falls back to the
  // function name when omitted.
  intent: z.string().min(1).optional(),
  // Closed semantic allowlist for data that may become runtime evidence. Paths use
  // the Rust runtime's JSON-path subset (for example "$.status").
  output: z.record(OutputFieldDefinitionSchema).optional(),
  outputRole: z.enum(['data', 'effect_confirmation', 'continuation', 'error']).optional(),
}).strict();
export type FunctionDefinition = z.infer<typeof FunctionDefinitionSchema>;

// Guard conditions a customer must satisfy for a state (or transition) to apply.
// v1 guards are presence checks on named customer-profile fields; richer
// predicates can be added later without breaking the wire format.
export const StateGuardSchema = z.object({
  requires_fields: z.array(z.string().min(1)).optional(),
});
export type StateGuard = z.infer<typeof StateGuardSchema>;

// Declarative lifecycle transition: when the named tool succeeds (and the guard,
// if any, passes), the customer moves to state `to`. Applied deterministically by
// the server's harness executor; an SDK `set_state` push always overrides.
export const StateTransitionSchema = z.object({
  on_tool_success: z.string().min(1),
  to: z.string().min(1),
  guard: StateGuardSchema.optional(),
});
export type StateTransition = z.infer<typeof StateTransitionSchema>;

export const StateDefinitionSchema = z.object({
  id: z.string().min(1),
  description: z.string().min(1),
  allowedTools: z.array(z.string().min(1)).optional(),
  blockedTools: z.array(z.string().min(1)).optional(),
  guards: StateGuardSchema.optional(),
  transitions: z.array(StateTransitionSchema).optional(),
});
export type StateDefinition = z.infer<typeof StateDefinitionSchema>;

export const PolicySeveritySchema = z.enum(['hard', 'soft']);
export type PolicySeverity = z.infer<typeof PolicySeveritySchema>;

export type AelioJsonValue =
  | null
  | boolean
  | number
  | string
  | AelioJsonValue[]
  | { [key: string]: AelioJsonValue };

export const AelioJsonValueSchema: z.ZodType<AelioJsonValue> = z.lazy(() =>
  z.union([
    z.null(),
    z.boolean(),
    z.number().finite(),
    z.string(),
    z.array(AelioJsonValueSchema),
    z.record(AelioJsonValueSchema),
  ]),
);

export type AelioPredicate =
  | { op: 'true' | 'false' }
  | { op: 'eq' | 'ne' | 'gt' | 'gte' | 'lt' | 'lte'; path: string; value: AelioJsonValue }
  | { op: 'in'; path: string; values: AelioJsonValue[] }
  | { op: 'present' | 'absent'; path: string }
  | { op: 'and' | 'or'; of: AelioPredicate[] }
  | { op: 'not'; of: AelioPredicate };

export const AelioPredicateSchema: z.ZodType<AelioPredicate> = z.lazy(() =>
  z.discriminatedUnion('op', [
    z.object({ op: z.literal('true') }).strict(),
    z.object({ op: z.literal('false') }).strict(),
    ...(['eq', 'ne', 'gt', 'gte', 'lt', 'lte'] as const).map((op) =>
      z.object({ op: z.literal(op), path: z.string().min(1), value: AelioJsonValueSchema }).strict(),
    ),
    z.object({
      op: z.literal('in'),
      path: z.string().min(1),
      values: z.array(AelioJsonValueSchema).max(256),
    }).strict(),
    z.object({ op: z.literal('present'), path: z.string().min(1) }).strict(),
    z.object({ op: z.literal('absent'), path: z.string().min(1) }).strict(),
    z.object({ op: z.literal('and'), of: z.array(AelioPredicateSchema).min(1).max(64) }).strict(),
    z.object({ op: z.literal('or'), of: z.array(AelioPredicateSchema).min(1).max(64) }).strict(),
    z.object({ op: z.literal('not'), of: AelioPredicateSchema }).strict(),
  ]),
);

export const AelioPolicyArtifactSchema = z.object({
  effect: z.enum(['allow', 'deny']),
  subject: z.object({
    role: z.string().min(1).optional(),
    state: z.string().min(1).optional(),
    tenant: z.string().min(1).optional(),
    segment: z.string().min(1).optional(),
  }).strict().default({}),
  action: z.object({
    capability: z.string().min(1).optional(),
    tool_id: z.string().min(1).optional(),
    transition: z.string().min(1).optional(),
    flow_id: z.string().min(1).optional(),
  }).strict(),
  condition: AelioPredicateSchema.default({ op: 'true' }),
  reason_code: z.string().min(1),
  priority: z.number().int().min(-1_000_000).max(1_000_000).default(100),
}).strict();
export type AelioPolicyArtifact = z.infer<typeof AelioPolicyArtifactSchema>;

export const PolicyDefinitionSchema = z.object({
  id: z.string().min(1),
  description: z.string().min(1),
  severity: PolicySeveritySchema.default('soft'),
  /** Closed policy enforced by the authoritative Rust runtime. */
  aelio: AelioPolicyArtifactSchema.optional(),
});
export type PolicyDefinition = z.infer<typeof PolicyDefinitionSchema>;

export const FlowStepDefinitionSchema = z.object({
  id: z.string().min(1),
  goal: z.string().min(1),
  tool: z.string().min(1).optional(),
});
export type FlowStepDefinition = z.infer<typeof FlowStepDefinitionSchema>;

const FlowCapabilityBindingSchema = z.object({
  name: z.string().min(1).max(128),
  step_id: z.string().min(1).max(256),
  capability: z.string().min(1).max(256),
  deadline_ms: z.number().int().min(1).max(300_000),
}).strict();

const FlowFixtureSchema = z.object({
  binding: z.string().min(1).max(128),
  output: AelioJsonValueSchema,
  usage_tokens: z.number().int().nonnegative().optional(),
}).strict();

const FlowGateCaseSchema = z.object({
  input: AelioJsonValueSchema,
  wakes: z.array(AelioJsonValueSchema).max(64).optional(),
  expect_park: z.boolean().optional(),
  expected: AelioJsonValueSchema,
  fixtures: z.array(FlowFixtureSchema).max(256).optional(),
}).strict();

const FlowLoweringSchema = z.object({
  format: z.literal(1),
  /** Calls must use `$cap:<binding>`; Rust rejects every raw executable target id. */
  program: AelioJsonValueSchema,
  bindings: z.array(FlowCapabilityBindingSchema).max(256),
  cases: z.array(FlowGateCaseSchema).min(20).max(128),
}).strict();

const FlowEscapeSchema = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('fallback'), flow_id: z.string().min(1) }).strict(),
  z.object({ kind: z.literal('free_range') }).strict(),
  z.object({ kind: z.literal('escalate') }).strict(),
]);

const AelioSemanticFlowStepSchema = z.object({
  id: z.string().min(1).max(256),
  intent: z.string().min(1).max(2_048),
  postcondition: AelioPredicateSchema,
  admissible: z.array(z.string().min(1).max(256)).max(64),
  on_violation: z.enum(['repair', 'escape']),
  suspendable: z.boolean(),
}).strict();

/**
 * Closed authored-flow declaration. This is not a raw runtime FlowPush: executable calls remain
 * symbolic until the Rust catalog boundary resolves and gates exact immutable tool pins.
 */
export const AelioFlowArtifactSchema = z.object({
  version: z.string().regex(/^[1-9][0-9]*$/),
  activation: z.object({
    hard_preconditions: z.array(AelioPredicateSchema).max(64),
    trigger_surface: z.array(z.string().min(1).max(512)).min(1).max(256),
    margin_threshold: z.number().finite().min(0).max(1),
  }).strict(),
  learnable: z.boolean(),
  preemption: z.enum(['hold', 'suspend_yield', 'yield']),
  steps: z.array(AelioSemanticFlowStepSchema).min(1).max(256),
  escape: FlowEscapeSchema,
  terminal_states: z.array(z.string().min(1).max(256)).max(256),
  ttl_secs: z.number().int().min(1).max(31_536_000).nullable(),
  max_attempts: z.number().int().min(1).max(1_000),
  lowering: FlowLoweringSchema,
}).strict();
export type AelioFlowArtifact = z.infer<typeof AelioFlowArtifactSchema>;

export const FlowDefinitionSchema = z.object({
  id: z.string().min(1),
  state: z.string().min(1),
  description: z.string().min(1),
  steps: z.array(FlowStepDefinitionSchema).min(1),
  /** Closed, versioned program consumed only by the authoritative Rust runtime. */
  aelio: AelioFlowArtifactSchema.optional(),
});
export type FlowDefinition = z.infer<typeof FlowDefinitionSchema>;

export const InvocationContextSchema = z.object({
  customerId: z.string().min(1),
  sessionId: z.string().min(1),
  channel: ChannelSchema,
  channelAddress: z.string().min(1),
  locale: z.string().optional(),
  metadata: z.record(z.unknown()).optional(),
});
export type InvocationContext = z.infer<typeof InvocationContextSchema>;

export const RegisterMessageSchema = z.object({
  type: z.literal('register'),
  sdkVersion: z.string().min(1),
  language: SdkLanguageSchema,
  /** Human-readable employer/app name shown in Aelio connect logs. */
  application: z.string().min(1).max(128).optional(),
  functions: z.array(FunctionDefinitionSchema),
  states: z.array(StateDefinitionSchema).optional(),
  policies: z.array(PolicyDefinitionSchema).optional(),
  flows: z.array(FlowDefinitionSchema).optional(),
  // Client-supplied assistant persona/voice; becomes the stable head of the
  // system prompt (see runtime/prompt-composer).
  persona: z.string().optional(),
  // Client-written description of what the product does — grounds the harness
  // planner's capability taxonomy. Falls back to a registry-generated brief.
  productBrief: z.string().optional(),
  // True when the SDK has registered an onSend handler — i.e. it can deliver
  // outbound channel messages itself (bring-your-own WhatsApp/SMS provider).
  canSend: z.boolean().optional(),
});
export type RegisterMessage = z.infer<typeof RegisterMessageSchema>;

export const SetStateMessageSchema = z.object({
  type: z.literal('set_state'),
  customerId: z.string().min(1),
  stateId: z.string().min(1),
  reason: z.string().optional(),
});
export type SetStateMessage = z.infer<typeof SetStateMessageSchema>;

export const InvokeMessageSchema = z.object({
  type: z.literal('invoke'),
  id: z.string().min(1),
  function: z.string().min(1),
  args: z.record(z.unknown()),
  context: InvocationContextSchema,
});
export type InvokeMessage = z.infer<typeof InvokeMessageSchema>;

export const ResultErrorSchema = z.object({
  code: z.string().min(1),
  message: z.string().min(1),
  retryable: z.boolean().optional(),
});

export const ResultMessageSchema = z.object({
  type: z.literal('result'),
  id: z.string().min(1),
  ok: z.boolean(),
  data: z.unknown().optional(),
  error: ResultErrorSchema.optional(),
  durationMs: z.number().nonnegative(),
});
export type ResultMessage = z.infer<typeof ResultMessageSchema>;

// SDK → Server: an inbound message the SDK received on its own channel
// (the dev owns the webhook + provider parsing) and is handing to Aelio.
export const IngestMessageSchema = z.object({
  type: z.literal('ingest'),
  channel: ChannelSchema,
  from: z.string().min(1),
  text: z.string().min(1),
  messageId: z.string().optional(),
  metadata: z.record(z.unknown()).optional(),
});
export type IngestMessage = z.infer<typeof IngestMessageSchema>;

// Server → SDK: deliver this outbound message via the SDK's onSend handler.
// Distinct from `invoke` (which is the LLM calling a tool) — this is the
// transport layer asking the dev's provider to send a reply.
export const SendInvokeMessageSchema = z.object({
  type: z.literal('send'),
  id: z.string().min(1),
  channel: ChannelSchema,
  to: z.string().min(1),
  content: z.string(),
  metadata: z.record(z.unknown()).optional(),
});
export type SendInvokeMessage = z.infer<typeof SendInvokeMessageSchema>;

export const PingMessageSchema = z.object({
  type: z.literal('ping'),
  ts: z.number(),
});
export type PingMessage = z.infer<typeof PingMessageSchema>;

export const PongMessageSchema = z.object({
  type: z.literal('pong'),
  ts: z.number(),
});
export type PongMessage = z.infer<typeof PongMessageSchema>;

export const SdkToServerMessageSchema = z.discriminatedUnion('type', [
  RegisterMessageSchema,
  ResultMessageSchema,
  PongMessageSchema,
  IngestMessageSchema,
  SetStateMessageSchema,
]);
export type SdkToServerMessage = z.infer<typeof SdkToServerMessageSchema>;

// Server → SDK: a protocol-level error (e.g. a malformed frame the SDK sent).
// Diagnostic only — the SDK surfaces it so integrators aren't left guessing why
// a message had no effect.
export const ServerErrorMessageSchema = z.object({
  type: z.literal('error'),
  code: z.string().min(1),
  message: z.string().min(1),
});
export type ServerErrorMessage = z.infer<typeof ServerErrorMessageSchema>;

export const AckMessageSchema = z.object({
  type: z.literal('ack'),
  op: z.enum(['set_state', 'ingest']),
});
export type AckMessage = z.infer<typeof AckMessageSchema>;

/** Server confirms catalog admission — printed on both Aelio and employer SDK terminals. */
export const RegisteredMessageSchema = z.object({
  type: z.literal('registered'),
  application: z.string().min(1),
  tenant: z.string().min(1),
  connectionId: z.string().min(1),
  tools: z.array(z.string().min(1)),
  states: z.array(z.string().min(1)),
  flows: z.array(z.string().min(1)),
  policies: z.array(z.string().min(1)),
  /** First connect vs live catalog refresh after tools/states/flows/policies change. */
  reason: z.enum(['connect', 'catalog_update']).default('connect'),
  added: z
    .object({
      tools: z.array(z.string()).default([]),
      states: z.array(z.string()).default([]),
      flows: z.array(z.string()).default([]),
      policies: z.array(z.string()).default([]),
    })
    .optional(),
  removed: z
    .object({
      tools: z.array(z.string()).default([]),
      states: z.array(z.string()).default([]),
      flows: z.array(z.string()).default([]),
      policies: z.array(z.string()).default([]),
    })
    .optional(),
});
export type RegisteredMessage = z.infer<typeof RegisteredMessageSchema>;

export const ServerToSdkMessageSchema = z.discriminatedUnion('type', [
  InvokeMessageSchema,
  SendInvokeMessageSchema,
  PingMessageSchema,
  ServerErrorMessageSchema,
  AckMessageSchema,
  RegisteredMessageSchema,
]);
export type ServerToSdkMessage = z.infer<typeof ServerToSdkMessageSchema>;

export const DEFAULT_SDK_PATH = '/sdk';
export const HEARTBEAT_INTERVAL_MS = 30_000;
export const HEARTBEAT_TIMEOUT_MS = 60_000;
export const INVOKE_TIMEOUT_MS = 30_000;
export const MAX_WS_FRAME_BYTES = 64 * 1024;
export const MAX_SDK_CONNECTIONS = 32;
export const SDK_REGISTER_TIMEOUT_MS = 15_000;
