export type ChatRole = 'user' | 'assistant' | 'system';

export type LLMToolCall = {
  id: string;
  name: string;
  args: Record<string, unknown>;
};

export type LLMToolResult = {
  toolUseId: string;
  content: string;
};

export type ChatMessage = {
  role: ChatRole;
  content: string;
  toolCalls?: LLMToolCall[];
  toolResults?: LLMToolResult[];
};

export type ToolDefinition = {
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
};

export type LLMTelemetryMeta = {
  purpose:
    | 'chat_completion'
    | 'tool_synthesis'
    | 'session_summary'
    | 'session_reflection';
  iteration?: number;
};

export type LLMCompleteOptions = {
  messages: ChatMessage[];
  tools: ToolDefinition[];
  system?: string;
  model: string;
  maxTokens?: number;
  temperature?: number;
  telemetry?: LLMTelemetryMeta;
};

export type LLMCompleteResult = {
  text: string;
  toolCalls: LLMToolCall[];
  stopReason: 'stop' | 'tool_use' | 'length' | 'error';
};

export interface LLMProvider {
  complete(opts: LLMCompleteOptions): Promise<LLMCompleteResult>;
}

export type LLMProviderConfig = {
  provider: 'anthropic' | 'openai' | 'gemini' | 'groq' | 'ollama' | 'mock';
  model: string;
  apiKey?: string;
  maxTokens?: number;
  baseUrl?: string;
};
