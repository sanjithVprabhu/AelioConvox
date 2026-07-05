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
});
export type FunctionDefinition = z.infer<typeof FunctionDefinitionSchema>;

export const StateDefinitionSchema = z.object({
  id: z.string().min(1),
  description: z.string().min(1),
  allowedTools: z.array(z.string().min(1)).optional(),
  blockedTools: z.array(z.string().min(1)).optional(),
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
  tool: z.string().min(1).optional(),
});
export type FlowStepDefinition = z.infer<typeof FlowStepDefinitionSchema>;

export const FlowDefinitionSchema = z.object({
  id: z.string().min(1),
  state: z.string().min(1),
  description: z.string().min(1),
  steps: z.array(FlowStepDefinitionSchema).min(1),
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
  functions: z.array(FunctionDefinitionSchema),
  states: z.array(StateDefinitionSchema).optional(),
  policies: z.array(PolicyDefinitionSchema).optional(),
  flows: z.array(FlowDefinitionSchema).optional(),
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
]);
export type SdkToServerMessage = z.infer<typeof SdkToServerMessageSchema>;

export const ServerToSdkMessageSchema = z.discriminatedUnion('type', [
  InvokeMessageSchema,
  SendInvokeMessageSchema,
  PingMessageSchema,
]);
export type ServerToSdkMessage = z.infer<typeof ServerToSdkMessageSchema>;

export const DEFAULT_SDK_PATH = '/sdk';
export const HEARTBEAT_INTERVAL_MS = 30_000;
export const HEARTBEAT_TIMEOUT_MS = 60_000;
export const INVOKE_TIMEOUT_MS = 30_000;