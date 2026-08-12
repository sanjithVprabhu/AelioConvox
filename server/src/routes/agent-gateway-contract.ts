import { z } from 'zod';

export const AgentGatewayCapabilitiesSchema = z.object({
  protocol_version: z.literal(2),
  provider: z.string().min(1).max(128),
  model: z.string().min(1).max(256),
  /** Provider-native function calling is disabled on the agent path. */
  native_tools: z.literal(false),
  /** Agent-loop provider transport: strict ReAct JSON text. */
  tool_transport: z.literal('react_json'),
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

export type AgentCompletionRequest = z.infer<typeof AgentCompletionRequestSchema>;

const ReactActionSchema = z.object({
  name: z.string().min(1).max(128),
  arguments: z.record(z.unknown()).optional().default({}),
}).strict();

const ReactReplySchema = z.object({
  thought: z.string().max(16_384).optional().default(''),
  actions: z.array(ReactActionSchema).min(1).max(32),
}).strict();

/** Build a text tool catalog for the system prompt (not provider function declarations). */
export function formatReactToolCatalog(
  tools: AgentCompletionRequest['tools'],
): string {
  const lines = [
    'Available tools (call via JSON actions; do not invent names):',
    ...tools.map((tool) => {
      const schema = JSON.stringify(tool.input_schema);
      return [
        `- ${tool.name} [v${tool.version}] (${tool.effect_class})`,
        `  ${tool.description}`,
        `  input_schema: ${schema}`,
      ].join('\n');
    }),
    '',
    'Reply with ONLY a JSON object of the form:',
    '{"thought":"...","actions":[{"name":"tool_name","arguments":{...}}]}',
    'Do not write markdown fences or prose outside the JSON object.',
    'finish must be the only action when completing the turn.',
    'For complex multi-step work use write_todos / update_todos; spawn_task for isolated sub-goals; await_tasks or cancel_tasks before finish if children are pending.',
  ];
  return lines.join('\n');
}

function assistantBlocksToReactText(
  content: z.infer<typeof AgentResponseSchema>['content'],
): string {
  const textParts = content
    .flatMap((block) => (block.type === 'text' ? [block.text] : []));
  const actions = content.flatMap((block) =>
    block.type === 'tool_call'
      ? [{ name: block.call.name, arguments: block.call.arguments }]
      : [],
  );
  if (actions.length === 0) {
    return textParts.join('\n').trim();
  }
  const payload = {
    thought: textParts.join('\n').trim(),
    actions,
  };
  return `\`\`\`json\n${JSON.stringify(payload, null, 2)}\n\`\`\``;
}

/**
 * Encode a structured agent-loop request as plain text for any LLM provider.
 * Never attaches tools / toolCalls / toolResults / toolChoice.
 */
export function reactRequestToLlmOptions(call: AgentCompletionRequest) {
  const catalog = formatReactToolCatalog(call.tools);
  const system = `${call.system.trim()}\n\n${catalog}`;
  const messages: Array<{ role: 'user' | 'assistant'; content: string }> = [
    { role: 'user', content: call.bootstrap },
  ];

  for (const message of call.messages) {
    switch (message.role) {
      case 'user':
        messages.push({ role: 'user', content: message.text });
        break;
      case 'assistant':
        messages.push({
          role: 'assistant',
          content: assistantBlocksToReactText(message.response.content),
        });
        break;
      case 'tool_result':
        messages.push({
          role: 'user',
          content: `Observation:\n${JSON.stringify({
            call_id: message.result.call_id,
            tool: message.result.tool,
            data: message.result.data,
            error: message.result.error,
          })}`,
        });
        break;
      case 'kernel_notice':
        messages.push({
          role: 'user',
          content: `Kernel notice (${message.code}): ${message.detail}`,
        });
        break;
    }
  }

  return {
    messages,
    tools: [] as Array<{ name: string; description: string; input_schema: Record<string, unknown> }>,
    system,
    model: call.model,
    maxTokens: call.max_tokens,
    temperature: call.temperature,
    responseFormat: { type: 'json_object' as const },
    telemetry: { purpose: 'agent_loop' as const },
  };
}

/** Extract a JSON object from raw model text (raw or fenced). */
export function extractJsonObject(text: string): unknown | null {
  const trimmed = text.trim();
  if (!trimmed) return null;

  const fence = trimmed.match(/```(?:json)?\s*([\s\S]*?)```/i);
  const candidate = (fence?.[1] ?? trimmed).trim();

  try {
    return JSON.parse(candidate);
  } catch {
    // Fall through to brace scan.
  }

  const start = candidate.indexOf('{');
  const end = candidate.lastIndexOf('}');
  if (start < 0 || end <= start) return null;
  try {
    return JSON.parse(candidate.slice(start, end + 1));
  } catch {
    return null;
  }
}

export type ParsedReactAssistant = {
  content: Array<
    | { type: 'text'; text: string }
    | { type: 'tool_call'; call: { id: string; name: string; arguments: Record<string, unknown> } }
  >;
  stop_reason: 'tool_use' | 'end_turn';
};

/**
 * Parse ReAct JSON text into structured agent gateway content blocks.
 * Invalid / empty actions → text-only content (kernel will nudge must_call_finish).
 */
export function parseReactAssistantText(
  text: string,
  requestId: string,
): ParsedReactAssistant {
  const parsed = extractJsonObject(text);
  const checked = ReactReplySchema.safeParse(parsed);
  if (!checked.success) {
    const fallback = text.trim();
    return {
      content: fallback ? [{ type: 'text', text: fallback }] : [],
      stop_reason: 'end_turn',
    };
  }

  const thought = checked.data.thought.trim();
  const content: ParsedReactAssistant['content'] = [];
  if (thought) {
    content.push({ type: 'text', text: thought });
  }
  checked.data.actions.forEach((action, index) => {
    content.push({
      type: 'tool_call',
      call: {
        id: `react-${requestId}-${index + 1}`.slice(0, 256),
        name: action.name,
        arguments: action.arguments ?? {},
      },
    });
  });
  return {
    content,
    stop_reason: 'tool_use',
  };
}

/** @deprecated Prefer reactRequestToLlmOptions — kept for transitional fixtures only. */
export function agentRequestToLlmOptions(call: AgentCompletionRequest) {
  return reactRequestToLlmOptions(call);
}
