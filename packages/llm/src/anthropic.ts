import type { ChatMessage, LLMCompleteOptions, LLMCompleteResult, LLMProvider, LLMToolCall } from './types.js';

type AnthropicContentBlock =
  | { type: 'text'; text: string }
  | { type: 'tool_use'; id: string; name: string; input: Record<string, unknown> }
  | { type: 'tool_result'; tool_use_id: string; content: string };

type AnthropicResponse = {
  content: AnthropicContentBlock[];
  stop_reason: 'end_turn' | 'tool_use' | 'max_tokens' | string;
};

function toAnthropicMessages(messages: ChatMessage[]) {
  return messages
    .filter((message) => message.role !== 'system')
    .map((message) => {
      if (message.toolResults && message.toolResults.length > 0) {
        return {
          role: 'user' as const,
          content: message.toolResults.map((result) => ({
            type: 'tool_result' as const,
            tool_use_id: result.toolUseId,
            content: result.content,
          })),
        };
      }

      if (message.toolCalls && message.toolCalls.length > 0) {
        const content: AnthropicContentBlock[] = [];
        if (message.content.trim()) {
          content.push({ type: 'text', text: message.content });
        }
        for (const toolCall of message.toolCalls) {
          content.push({
            type: 'tool_use',
            id: toolCall.id,
            name: toolCall.name,
            input: toolCall.args,
          });
        }
        return { role: 'assistant' as const, content };
      }

      return {
        role: message.role as 'user' | 'assistant',
        content: message.content,
      };
    });
}

export class AnthropicProvider implements LLMProvider {
  constructor(
    private readonly apiKey: string,
    private readonly defaultMaxTokens = 4096,
  ) {}

  async complete(opts: LLMCompleteOptions): Promise<LLMCompleteResult> {
    const response = await fetch('https://api.anthropic.com/v1/messages', {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'x-api-key': this.apiKey,
        'anthropic-version': '2023-06-01',
      },
      body: JSON.stringify({
        model: opts.model,
        max_tokens: opts.maxTokens ?? this.defaultMaxTokens,
        temperature: opts.temperature ?? 0.2,
        system: opts.system,
        messages: toAnthropicMessages(opts.messages),
        tools: opts.tools.map((tool) => ({
          name: tool.name,
          description: tool.description,
          input_schema: tool.input_schema,
        })),
      }),
    });

    if (!response.ok) {
      const body = await response.text();
      throw new Error(`Anthropic API error (${response.status}): ${body}`);
    }

    const data = (await response.json()) as AnthropicResponse;
    const text = data.content
      .filter((block): block is Extract<AnthropicContentBlock, { type: 'text' }> => block.type === 'text')
      .map((block) => block.text)
      .join('\n')
      .trim();

    const toolCalls: LLMToolCall[] = data.content
      .filter(
        (block): block is Extract<AnthropicContentBlock, { type: 'tool_use' }> =>
          block.type === 'tool_use',
      )
      .map((block) => ({
        id: block.id,
        name: block.name,
        args: block.input,
      }));

    const stopReason =
      data.stop_reason === 'tool_use' || toolCalls.length > 0
        ? 'tool_use'
        : data.stop_reason === 'max_tokens'
          ? 'length'
          : 'stop';

    return { text, toolCalls, stopReason };
  }
}