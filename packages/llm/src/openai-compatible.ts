import { randomUUID } from 'node:crypto';
import type {
  ChatMessage,
  LLMCompleteOptions,
  LLMCompleteResult,
  LLMProvider,
  LLMToolCall,
  LLMToolChoice,
} from './types.js';

function toOpenAIToolChoice(choice: LLMToolChoice | undefined, toolCount: number) {
  if (!choice) {
    return toolCount > 0 ? 'auto' : 'none';
  }
  if (choice.type === 'tool') {
    return { type: 'function' as const, function: { name: choice.name } };
  }
  return choice.type;
}

type OpenAIMessage =
  | {
      role: 'system' | 'user' | 'assistant';
      content: string | null;
      tool_calls?: Array<{
        id: string;
        type: 'function';
        function: {
          name: string;
          arguments: string;
        };
      }>;
    }
  | {
      role: 'tool';
      content: string;
      tool_call_id: string;
    };

type OpenAIResponse = {
  choices?: Array<{
    finish_reason?: string | null;
    message?: {
      content?: string | null;
      tool_calls?: Array<{
        id?: string;
        type?: 'function';
        function?: {
          name?: string;
          arguments?: string;
        };
      }>;
    };
  }>;
  error?: {
    message?: string;
  };
  usage?: {
    prompt_tokens?: number;
    completion_tokens?: number;
  };
};

type OpenAIToolCall = NonNullable<
  NonNullable<NonNullable<OpenAIResponse['choices']>[number]['message']>['tool_calls']
>[number];

function toOpenAIMessages(messages: ChatMessage[]): OpenAIMessage[] {
  const output: OpenAIMessage[] = [];

  for (const message of messages) {
    if (message.role === 'system') {
      output.push({ role: 'system', content: message.content });
      continue;
    }

    if (message.toolCalls && message.toolCalls.length > 0) {
      output.push({
        role: 'assistant',
        content: message.content || null,
        tool_calls: message.toolCalls.map((toolCall) => ({
          id: toolCall.id,
          type: 'function',
          function: {
            name: toolCall.name,
            arguments: JSON.stringify(toolCall.args),
          },
        })),
      });
      continue;
    }

    if (message.toolResults && message.toolResults.length > 0) {
      for (const toolResult of message.toolResults) {
        output.push({
          role: 'tool',
          content: toolResult.content,
          tool_call_id: toolResult.toolUseId,
        });
      }
      continue;
    }

    output.push({
      role: message.role,
      content: message.content,
    });
  }

  return output;
}

function parseToolCalls(toolCalls: OpenAIToolCall[] | undefined): LLMToolCall[] {
  return (toolCalls ?? []).flatMap((toolCall) => {
    const name = toolCall.function?.name;
    if (!name) {
      return [];
    }

    let args: Record<string, unknown> = {};
    const rawArgs = toolCall.function?.arguments ?? '{}';
    try {
      const parsed = JSON.parse(rawArgs) as unknown;
      if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
        args = parsed as Record<string, unknown>;
      }
    } catch {
      args = { raw: rawArgs };
    }

    return [
      {
        id: toolCall.id ?? randomUUID(),
        name,
        args,
      },
    ];
  });
}

export class OpenAICompatibleProvider implements LLMProvider {
  constructor(
    private readonly baseUrl: string,
    private readonly defaultHeaders: Record<string, string>,
    private readonly defaultMaxTokens = 4096,
  ) {}

  async complete(opts: LLMCompleteOptions): Promise<LLMCompleteResult> {
    const response = await fetch(this.baseUrl, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        ...this.defaultHeaders,
      },
      body: JSON.stringify({
        model: opts.model,
        // `max_tokens` is the broadly-compatible field (OpenAI gpt-4o family, Groq,
        // Ollama). OpenAI rejects sending it together with `max_completion_tokens`.
        max_tokens: opts.maxTokens ?? this.defaultMaxTokens,
        temperature: opts.temperature ?? 0.2,
        messages: [
          ...(opts.system ? [{ role: 'system' as const, content: opts.system }] : []),
          ...toOpenAIMessages(opts.messages),
        ],
        tools: opts.tools.map((tool) => ({
          type: 'function',
          function: {
            name: tool.name,
            description: tool.description,
            parameters: tool.input_schema,
          },
        })),
        tool_choice: toOpenAIToolChoice(opts.toolChoice, opts.tools.length),
      }),
    });

    if (!response.ok) {
      const body = (await response.text()).trim();
      throw new Error(`LLM API error (${response.status}): ${body}`);
    }

    const data = (await response.json()) as OpenAIResponse;
    const choice = data.choices?.[0];
    if (!choice?.message) {
      throw new Error(data.error?.message ?? 'LLM API returned no completion choices');
    }

    const toolCalls = parseToolCalls(choice.message.tool_calls);
    const text = (choice.message.content ?? '').trim();
    const finishReason = choice.finish_reason ?? 'stop';

    return {
      text,
      toolCalls,
      stopReason:
        toolCalls.length > 0 || finishReason === 'tool_calls'
          ? 'tool_use'
          : finishReason === 'length'
            ? 'length'
            : 'stop',
      ...(data.usage
        ? {
            usage: {
              inputTokens: data.usage.prompt_tokens ?? 0,
              outputTokens: data.usage.completion_tokens ?? 0,
            },
          }
        : {}),
    };
  }
}
