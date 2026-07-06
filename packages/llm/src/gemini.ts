import { randomUUID } from 'node:crypto';
import type {
  ChatMessage,
  LLMCompleteOptions,
  LLMCompleteResult,
  LLMProvider,
  LLMToolCall,
} from './types.js';

type GeminiPart =
  | { text: string }
  | { functionCall: { name: string; args: Record<string, unknown> } }
  | { functionResponse: { name: string; response: { content: string } } };

type GeminiContent = {
  role: 'user' | 'model';
  parts: GeminiPart[];
};

type GeminiResponse = {
  candidates?: Array<{
    content?: {
      parts?: Array<{
        text?: string;
        functionCall?: { name?: string; args?: Record<string, unknown> };
      }>;
    };
    finishReason?: string;
  }>;
  error?: { message?: string };
  usageMetadata?: {
    promptTokenCount?: number;
    candidatesTokenCount?: number;
  };
};

function toGeminiContents(messages: ChatMessage[]): GeminiContent[] {
  const output: GeminiContent[] = [];
  const toolIdToName = new Map<string, string>();

  for (const message of messages) {
    if (message.role === 'system') {
      continue;
    }

    if (message.toolCalls && message.toolCalls.length > 0) {
      for (const toolCall of message.toolCalls) {
        toolIdToName.set(toolCall.id, toolCall.name);
      }
    }

    if (message.toolResults && message.toolResults.length > 0) {
      output.push({
        role: 'user',
        parts: message.toolResults.map((result) => ({
          functionResponse: {
            name: toolIdToName.get(result.toolUseId) ?? result.toolUseId,
            response: { content: result.content },
          },
        })),
      });
      continue;
    }

    if (message.toolCalls && message.toolCalls.length > 0) {
      const parts: GeminiPart[] = [];
      if (message.content.trim()) {
        parts.push({ text: message.content });
      }
      for (const toolCall of message.toolCalls) {
        parts.push({
          functionCall: {
            name: toolCall.name,
            args: toolCall.args,
          },
        });
      }
      output.push({ role: 'model', parts });
      continue;
    }

    output.push({
      role: message.role === 'assistant' ? 'model' : 'user',
      parts: [{ text: message.content }],
    });
  }

  return output;
}

function toFunctionDeclarations(tools: LLMCompleteOptions['tools']) {
  return tools.map((tool) => ({
    name: tool.name,
    description: tool.description,
    parameters: tool.input_schema,
  }));
}

export class GeminiProvider implements LLMProvider {
  constructor(
    private readonly apiKey: string,
    private readonly defaultMaxTokens = 4096,
    private readonly baseUrl = 'https://generativelanguage.googleapis.com/v1beta',
  ) {}

  async complete(opts: LLMCompleteOptions): Promise<LLMCompleteResult> {
    const url = `${this.baseUrl}/models/${encodeURIComponent(opts.model)}:generateContent?key=${encodeURIComponent(this.apiKey)}`;

    const body: Record<string, unknown> = {
      contents: toGeminiContents(opts.messages),
      generationConfig: {
        maxOutputTokens: opts.maxTokens ?? this.defaultMaxTokens,
        temperature: opts.temperature ?? 0.2,
      },
    };

    if (opts.system) {
      body.systemInstruction = { parts: [{ text: opts.system }] };
    }

    if (opts.tools.length > 0) {
      body.tools = [{ functionDeclarations: toFunctionDeclarations(opts.tools) }];
      body.toolConfig = {
        functionCallingConfig: { mode: 'AUTO' },
      };
    }

    const response = await fetch(url, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    });

    if (!response.ok) {
      const errorBody = await response.text();
      throw new Error(`Gemini API error (${response.status}): ${errorBody}`);
    }

    const data = (await response.json()) as GeminiResponse;
    const parts = data.candidates?.[0]?.content?.parts ?? [];

    const text = parts
      .map((part) => part.text ?? '')
      .join('\n')
      .trim();

    const toolCalls: LLMToolCall[] = parts.flatMap((part) => {
      const call = part.functionCall;
      if (!call?.name) {
        return [];
      }
      return [
        {
          id: randomUUID(),
          name: call.name,
          args: call.args ?? {},
        },
      ];
    });

    const finishReason = data.candidates?.[0]?.finishReason ?? 'STOP';
    const stopReason =
      toolCalls.length > 0 || finishReason === 'TOOL_CALLS'
        ? 'tool_use'
        : finishReason === 'MAX_TOKENS'
          ? 'length'
          : 'stop';

    if (!text && toolCalls.length === 0) {
      throw new Error(data.error?.message ?? 'Gemini API returned no completion');
    }

    return {
      text,
      toolCalls,
      stopReason,
      ...(data.usageMetadata
        ? {
            usage: {
              inputTokens: data.usageMetadata.promptTokenCount ?? 0,
              outputTokens: data.usageMetadata.candidatesTokenCount ?? 0,
            },
          }
        : {}),
    };
  }
}