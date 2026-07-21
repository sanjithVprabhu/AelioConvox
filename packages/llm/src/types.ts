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
    | 'immediate_context_compaction'
    | 'aspect_discovery'
    | 'plan'
    | 'replan'
    | 'bind'
    | 'synthesis'
    | 'recoil_extract';
  iteration?: number;
};

/**
 * How the model may use the provided tools.
 * - 'auto': model decides (default)
 * - 'none': tools visible but must not be called
 * - 'tool': the named tool MUST be called — this is the structured-output
 *   mechanism (e.g. the harness forces `emit_turn` to get a validated plan).
 */
export type LLMToolChoice =
  | { type: 'auto' }
  | { type: 'none' }
  | { type: 'tool'; name: string };

export type LLMCompleteOptions = {
  messages: ChatMessage[];
  tools: ToolDefinition[];
  system?: string;
  model: string;
  maxTokens?: number;
  temperature?: number;
  toolChoice?: LLMToolChoice;
  telemetry?: LLMTelemetryMeta;
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
