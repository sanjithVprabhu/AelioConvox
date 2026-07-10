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
    | 'session_reflection'
    | 'harness_router'
    | 'harness_planner'
    | 'harness_replan'
    | 'harness_synthesis';
  iteration?: number;
};

export type LLMToolChoice =
  | 'auto'
  | 'none'
  | { mode: 'any'; allowedFunctionNames?: string[] }
  | { mode: 'required'; name: string };

export type LLMCompleteOptions = {
  messages: ChatMessage[];
  tools: ToolDefinition[];
  system?: string;
  model: string;
  maxTokens?: number;
  temperature?: number;
  telemetry?: LLMTelemetryMeta;
  /** When set, providers that support it return JSON-only text (no markdown). */
  responseFormat?: 'json';
  toolChoice?: LLMToolChoice;
};

export type LLMUsage = {
  inputTokens: number;
  outputTokens: number;
};

export type LLMCompleteResult = {
  text: string;
  toolCalls: LLMToolCall[];
  stopReason: 'stop' | 'tool_use' | 'length' | 'error';
  /** Provider-reported token usage, when available. */
  usage?: LLMUsage;
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
