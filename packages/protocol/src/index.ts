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

export const FunctionDefinitionSchema = z.object({
  name: z.string().min(1),
  description: z.string().min(1),
  params: z.record(z.unknown()),
  safety: SafetyLevelSchema,
  // Client-declared intent category (e.g. "order_inquiry"). Drives the runtime's
  // intent stack and tool retrieval using the TENANT's vocabulary — the core never
  // hardcodes domain topics. Falls back to the function name when omitted.
  intent: z.string().min(1).optional(),
});
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
  /** Compact access rules for large registries. Intent values come from FunctionDefinition.intent. */
  allowedIntents: z.array(z.string().min(1)).optional(),
  blockedIntents: z.array(z.string().min(1)).optional(),
  /** Safety-class selectors compose with intent/name selectors; block rules always win. */
  allowedSafety: z.array(SafetyLevelSchema).optional(),
  blockedSafety: z.array(SafetyLevelSchema).optional(),
  guards: StateGuardSchema.optional(),
  transitions: z.array(StateTransitionSchema).optional(),
});
export type StateDefinition = z.infer<typeof StateDefinitionSchema>;

export const PolicySeveritySchema = z.enum(['hard', 'soft']);
export type PolicySeverity = z.infer<typeof PolicySeveritySchema>;

export const PolicyDefinitionSchema = z.object({
  id: z.string().min(1),
  description: z.string().min(1),
  severity: PolicySeveritySchema.default('soft'),
});
export type PolicyDefinition = z.infer<typeof PolicyDefinitionSchema>;

export const FlowStepDefinitionSchema = z.object({
  id: z.string().min(1),
  goal: z.string().min(1),
  /** @deprecated use `type: 'tool'` — kept for backward compatibility */
  tool: z.string().min(1).optional(),
  type: z.enum(['tool', 'attribute', 'content']).optional(),
  attribute: z.string().min(1).optional(),
  ui_format: z
    .object({
      component: z.string().min(1),
      config: z.record(z.unknown()).optional(),
    })
    .optional(),
  skip_if_present: z.string().min(1).optional(),
  on_complete: z
    .object({
      advance_flow: z.boolean().default(true),
      transition_to: z.string().min(1).optional(),
    })
    .optional(),
});
export type FlowStepDefinition = z.infer<typeof FlowStepDefinitionSchema>;

export const FlowDefinitionSchema = z.object({
  id: z.string().min(1),
  state: z.string().min(1),
  description: z.string().min(1),
  steps: z.array(FlowStepDefinitionSchema).min(1),
});
export type FlowDefinition = z.infer<typeof FlowDefinitionSchema>;

/** Canonical global product lifecycle stages. */
export const GlobalStageSchema = z.enum(['unverified', 'verified', 'onboarding', 'active']);
export type GlobalStage = z.infer<typeof GlobalStageSchema>;

/** Per-feature lifecycle stages (discover → activate → retain). */
export const FeatureStageSchema = z.enum([
  'not_started',
  'feature_discovery',
  'activated',
  'power_usage',
]);
export type FeatureStage = z.infer<typeof FeatureStageSchema>;

export const EntryMethodSchema = z.enum(['system_triggered', 'user_invoked']);
export type EntryMethod = z.infer<typeof EntryMethodSchema>;

export const UiFormatSchema = z.object({
  component: z.string().min(1),
  config: z.record(z.unknown()).optional(),
});
export type UiFormat = z.infer<typeof UiFormatSchema>;

export const AttributeDefinitionSchema = z.object({
  id: z.string().min(1),
  label: z.string().min(1),
  data_type: z.enum(['string', 'number', 'boolean', 'object', 'array']).default('string'),
  sensitivity_tier: z.enum(['public', 'pii', 'sensitive_regulated']).default('pii'),
  prompts: z.array(z.string().min(1)).optional(),
  ui_format: UiFormatSchema.optional(),
  enum_values: z.array(z.union([z.string(), z.number()])).optional(),
});
export type AttributeDefinition = z.infer<typeof AttributeDefinitionSchema>;

export const PipelineStageDefinitionSchema = z.object({
  id: z.string().min(1),
  description: z.string().min(1),
  content: z
    .object({
      greeting: z.string().optional(),
      cta: z.string().optional(),
    })
    .optional(),
  flow: z.string().min(1).optional(),
  allowedTools: z.array(z.string().min(1)).optional(),
  blockedTools: z.array(z.string().min(1)).optional(),
  allowedIntents: z.array(z.string().min(1)).optional(),
  blockedIntents: z.array(z.string().min(1)).optional(),
  allowedSafety: z.array(SafetyLevelSchema).optional(),
  blockedSafety: z.array(SafetyLevelSchema).optional(),
  guards: StateGuardSchema.optional(),
  next: z.string().min(1).optional(),
});
export type PipelineStageDefinition = z.infer<typeof PipelineStageDefinitionSchema>;

export const FeatureTriggerSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('immediate') }),
  z.object({
    type: z.literal('stage_reached'),
    depends_on_feature: z.string().min(1),
    min_stage: FeatureStageSchema,
    dependency_type: z.enum(['soft', 'hard']).default('soft'),
  }),
  z.object({
    type: z.literal('event'),
    event_name: z.string().min(1),
  }),
  z.object({ type: z.literal('user_invoked') }),
]);
export type FeatureTrigger = z.infer<typeof FeatureTriggerSchema>;

export const FeaturePipelineDefinitionSchema = z.object({
  id: z.string().min(1),
  description: z.string().optional(),
  trigger: FeatureTriggerSchema.optional(),
  activation_action: z.string().min(1).optional(),
});
export type FeaturePipelineDefinition = z.infer<typeof FeaturePipelineDefinitionSchema>;

export const PipelineManifestSchema = z.object({
  initial_stage: z.string().min(1).default('unverified'),
  stages: z.record(PipelineStageDefinitionSchema),
  features: z.array(FeaturePipelineDefinitionSchema).optional(),
});
export type PipelineManifest = z.infer<typeof PipelineManifestSchema>;

/** Widget UI directive emitted by the pipeline engine. */
export const UiDirectiveSchema = z.object({
  component: z.string().min(1),
  config: z.record(z.unknown()).optional(),
  attribute_id: z.string().optional(),
  step_id: z.string().optional(),
});
export type UiDirective = z.infer<typeof UiDirectiveSchema>;

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
  functions: z.array(FunctionDefinitionSchema),
  states: z.array(StateDefinitionSchema).optional(),
  policies: z.array(PolicyDefinitionSchema).optional(),
  flows: z.array(FlowDefinitionSchema).optional(),
  pipeline: PipelineManifestSchema.optional(),
  attributes: z.array(AttributeDefinitionSchema).optional(),
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

export const SetFlowProgressMessageSchema = z.object({
  type: z.literal('set_flow_progress'),
  customerId: z.string().min(1),
  flowId: z.string().min(1),
  stepIndex: z.number().int().nonnegative(),
  completedSteps: z.array(z.string().min(1)).optional(),
});
export type SetFlowProgressMessage = z.infer<typeof SetFlowProgressMessageSchema>;

export const SetGlobalStageMessageSchema = z.object({
  type: z.literal('set_global_stage'),
  customerId: z.string().min(1),
  stage: z.string().min(1),
  entryMethod: EntryMethodSchema.optional(),
  reason: z.string().optional(),
});
export type SetGlobalStageMessage = z.infer<typeof SetGlobalStageMessageSchema>;

export const SetAttributeMessageSchema = z.object({
  type: z.literal('set_attribute'),
  customerId: z.string().min(1),
  attributeId: z.string().min(1),
  value: z.unknown(),
  source: z.enum(['explicit_ask', 'inferred', 'third_party_auth']).optional(),
  verified: z.boolean().optional(),
});
export type SetAttributeMessage = z.infer<typeof SetAttributeMessageSchema>;

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
  SetFlowProgressMessageSchema,
  SetGlobalStageMessageSchema,
  SetAttributeMessageSchema,
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
  op: z.enum(['set_state', 'set_flow_progress', 'set_global_stage', 'set_attribute', 'ingest']),
});
export type AckMessage = z.infer<typeof AckMessageSchema>;

export const ServerToSdkMessageSchema = z.discriminatedUnion('type', [
  InvokeMessageSchema,
  SendInvokeMessageSchema,
  PingMessageSchema,
  ServerErrorMessageSchema,
  AckMessageSchema,
]);
export type ServerToSdkMessage = z.infer<typeof ServerToSdkMessageSchema>;

export const DEFAULT_SDK_PATH = '/sdk';
export const HEARTBEAT_INTERVAL_MS = 30_000;
export const HEARTBEAT_TIMEOUT_MS = 60_000;
export const INVOKE_TIMEOUT_MS = 30_000;
export const MAX_WS_FRAME_BYTES = 64 * 1024;
export const MAX_SDK_CONNECTIONS = 32;
export const SDK_REGISTER_TIMEOUT_MS = 15_000;

export {
  expandToolAccess,
  type ToolAccessSelectors,
  type ToolGroupDefinition,
} from './tool-groups.js';
