import type { ChatMessage, LLMProvider } from '@aelio/llm';
import type { LedgerEntry } from './schema.js';

const MAX_RESULT_CHARS = 4000;

function capResult(payload: string): string {
  if (payload.length <= MAX_RESULT_CHARS) {
    return payload;
  }
  return `${payload.slice(0, MAX_RESULT_CHARS)}…[truncated]`;
}

/**
 * The final reply call. Executed tool results are presented as native
 * tool-call/tool-result turns (providers ground better on their own format,
 * and the mock provider's result-synthesis branch keys on it). Soft policies
 * are re-injected via the system prompt the caller composed — synthesis is
 * where tone/limits language actually lands in front of the user.
 */
export async function runSynthesis(input: {
  llm: LLMProvider;
  model: string;
  maxTokens: number;
  system: string;
  history: ChatMessage[];
  userMessage: string;
  goal: string;
  ledger: LedgerEntry[];
}): Promise<string> {
  const messages: ChatMessage[] = [
    ...input.history,
    { role: 'user', content: input.userMessage },
  ];

  if (input.ledger.length > 0) {
    messages.push({
      role: 'assistant',
      content: `Working on: ${input.goal}`,
      toolCalls: input.ledger.map((entry) => ({
        id: entry.instructionId,
        name: entry.toolName,
        args: {},
      })),
    });
    messages.push({
      role: 'user',
      content: '',
      toolResults: input.ledger.map((entry) => ({
        toolUseId: entry.instructionId,
        content:
          entry.status === 'success'
            ? capResult(JSON.stringify(entry.result))
            : JSON.stringify({ error: entry.result ?? 'tool failed' }),
      })),
    });
  }

  const result = await input.llm.complete({
    model: input.model,
    maxTokens: input.maxTokens,
    system: input.system,
    messages,
    tools: [],
    telemetry: { purpose: 'synthesis' },
  });

  return result.text.trim() || 'I could not generate a response right now.';
}
