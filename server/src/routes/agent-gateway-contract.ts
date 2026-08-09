import { z } from 'zod';

export const AgentGatewayCapabilitiesSchema = z.object({
  protocol_version: z.literal(2),
  provider: z.string().min(1).max(128),
  model: z.string().min(1).max(256),
  native_tools: z.literal(true),
  prompt_caching: z.boolean(),
  streaming: z.boolean(),
  max_output_tokens: z.number().int().min(512).max(131_072),
}).strict();

const AgentToolCallSchema = z.object({
  id: z.string().min(1).max(256),
  name: z.string().min(1).max(128),
  arguments: z.record(z.unknown()),
}).strict();

const AgentUsageSchema = z.object({
  input_tokens: z.number().int().nonnegative(),
  cached_input_tokens: z.number().int().nonnegative(),
  output_tokens: z.number().int().nonnegative(),
}).strict();

export const AgentResponseSchema = z.object({
  id: z.string().min(1).max(256),
  content: z.array(z.discriminatedUnion('type', [
    z.object({ type: z.literal('text'), text: z.string().max(262_144) }).strict(),
    z.object({ type: z.literal('tool_call'), call: AgentToolCallSchema }).strict(),
  ])).max(128),
  stop_reason: z.enum(['tool_use', 'end_turn', 'max_output_tokens', 'other']),
  usage: AgentUsageSchema,
}).strict();

const AgentToolResultSchema = z.object({
  call_id: z.string().min(1).max(256),
  tool: z.string().min(1).max(128),
  data: z.unknown().nullable(),
  error: z.object({
    class: z.string().min(1).max(128),
    retryable: z.boolean(),
    detail: z.string().max(8_192),
  }).strict().nullable(),
}).strict();

const AgentMessageSchema = z.discriminatedUnion('role', [
  z.object({ role: z.literal('user'), text: z.string().max(262_144) }).strict(),
  z.object({ role: z.literal('assistant'), response: AgentResponseSchema }).strict(),
  z.object({ role: z.literal('tool_result'), result: AgentToolResultSchema }).strict(),
  z.object({
    role: z.literal('kernel_notice'),
    code: z.string().min(1).max(128),
    detail: z.string().max(8_192),
  }).strict(),
]);

export const AgentCompletionRequestSchema = z.object({
  protocol_version: z.literal(2),
  request_id: z.string().min(1).max(256),
  attempt_id: z.string().min(1).max(256),
  model: z.string().min(1).max(256),
  max_tokens: z.number().int().positive().max(131_072),
  temperature: z.number().min(0).max(2),
  system: z.string().min(1).max(262_144),
  bootstrap: z.string().min(1).max(262_144),
  messages: z.array(AgentMessageSchema).max(2_048),
  tools: z.array(z.object({
    name: z.string().min(1).max(128),
    version: z.string().min(1).max(128),
    description: z.string().min(1).max(2_048),
    input_schema: z.record(z.unknown()),
    effect_class: z.enum([
      'read',
      'write_reversible',
      'write_irreversible',
      'financial',
      'access_control',
    ]),
  }).strict()).min(1).max(5_000),
  tool_choice: z.object({ type: z.literal('auto') }).strict(),
  cache: z.object({
    stable_prefix_hash: z.string().regex(/^[a-f0-9]{64}$/),
  }).strict(),
}).strict();

export const AgentCompletionResponseSchema = z.object({
  protocol_version: z.literal(2),
  request_id: z.string().min(1).max(256),
  attempt_id: z.string().min(1).max(256),
  response: AgentResponseSchema,
}).strict();

type AgentCompletionRequest = z.infer<typeof AgentCompletionRequestSchema>;

export function agentRequestToLlmOptions(call: AgentCompletionRequest) {
  const messages = [
    { role: 'user' as const, content: call.bootstrap },
    ...call.messages.map((message) => {
      switch (message.role) {
        case 'user':
          return { role: 'user' as const, content: message.text };
        case 'assistant':
          return {
            role: 'assistant' as const,
            content: message.response.content
              .flatMap((block) => block.type === 'text' ? [block.text] : [])
              .join('\n'),
            toolCalls: message.response.content.flatMap((block) =>
              block.type === 'tool_call'
                ? [{ id: block.call.id, name: block.call.name, args: block.call.arguments }]
                : [],
            ),
          };
        case 'tool_result':
          return {
            role: 'user' as const,
            content: '',
            toolResults: [{
              toolUseId: message.result.call_id,
              content: JSON.stringify({
                tool: message.result.tool,
                data: message.result.data,
                error: message.result.error,
              }),
            }],
          };
        case 'kernel_notice':
          return {
            role: 'user' as const,
            content: JSON.stringify({
              kernel_notice: { code: message.code, detail: message.detail },
            }),
          };
      }
    }),
  ];
  return {
    messages,
    tools: call.tools.map((tool) => ({
      name: tool.name,
      description: tool.description,
      input_schema: tool.input_schema,
    })),
    system: call.system,
    model: call.model,
    maxTokens: call.max_tokens,
    temperature: call.temperature,
    toolChoice: { type: 'auto' as const },
    telemetry: { purpose: 'agent_loop' as const },
  };
}
