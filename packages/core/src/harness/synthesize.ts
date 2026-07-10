import type { LLMProvider } from '@aelio/llm';
import type { AelioDatabase } from '@aelio/db';
import type { PlanStepRecord } from './types.js';
import { recordLlmTokenSpend } from './budget.js';

export async function synthesizeReply(input: {
  llm: LLMProvider;
  model: string;
  maxTokens: number;
  userMessage: string;
  steps: PlanStepRecord[];
  database?: AelioDatabase['db'];
  planId?: string;
  abortReason?: string;
}): Promise<string> {
  const successful = input.steps
    .filter((step) => step.status === 'success')
    .sort((a, b) => a.stepOrder - b.stepOrder);

  const stepSummary = successful
    .map(
      (step) =>
        `Step ${step.stepOrder + 1} (${step.toolName}): ${JSON.stringify(step.output ?? {})}`,
    )
    .join('\n');

  const system = input.abortReason
    ? 'Summarize what was attempted and explain clearly why the request could not be fully completed. Be concise and helpful.'
    : 'Format a clear, concise user-facing reply based on the completed plan steps. Do not mention internal step numbers or tool names unless helpful.';

  const userContent = input.abortReason
    ? `User request: ${input.userMessage}\nAbort reason: ${input.abortReason}\nCompleted steps:\n${stepSummary || '(none)'}`
    : `User request: ${input.userMessage}\nCompleted steps:\n${stepSummary}`;

  const result = await input.llm.complete({
    model: input.model,
    maxTokens: input.maxTokens,
    system,
    messages: [{ role: 'user', content: userContent }],
    tools: [],
    telemetry: { purpose: 'harness_synthesis' },
  });

  if (input.database && input.planId) {
    await recordLlmTokenSpend(input.database, input.planId, result.usage);
  }

  return result.text.trim() || 'Done.';
}
