import type { ChatMessage, LLMProvider, LLMUsage } from '@aelio/llm';
import { EMIT_TURN_INPUT_SCHEMA, EmitTurnSchema, type EmitTurn } from './schema.js';

const EMIT_TURN_DESCRIPTION = `Decide how to handle the user's message.
- mode "reply": answer directly. ONLY when the answer needs no tenant data, no account state, and no side effects. A reply that states an account fact or promises an action is NOT a reply — plan it.
- mode "refuse": the request is outside what this product can do. Give the concrete reason.
- mode "plan": anything requiring tools, data, or actions. Emit capability-level steps; name a tool only when one of the shown tools clearly fits. Declare produces for values later steps will need. Do NOT order or parallelize steps — the executor derives that from data dependencies.`;

export type PlannerResult = {
  turn: EmitTurn;
  /** True when the model failed structured output twice and we degraded. */
  degraded: boolean;
  /** Summed token usage across this call's attempts, for budget accounting. */
  usage?: LLMUsage;
};

function addUsage(a: LLMUsage | undefined, b: LLMUsage | undefined): LLMUsage | undefined {
  if (!a) return b;
  if (!b) return a;
  return { inputTokens: a.inputTokens + b.inputTokens, outputTokens: a.outputTokens + b.outputTokens };
}

/**
 * Pass 1: the merged router+planner. One forced `emit_turn` call decides
 * shallow reply vs refusal vs capability plan. Validation failures get exactly
 * one corrective retry; after that we degrade to treating any free text as a
 * shallow reply (never crash a turn on malformed planner output).
 */
export async function runPlanner(input: {
  llm: LLMProvider;
  model: string;
  maxTokens: number;
  system: string;
  history: ChatMessage[];
  userMessage: string;
  purpose?: 'plan' | 'replan';
}): Promise<PlannerResult> {
  const tools = [
    {
      name: 'emit_turn',
      description: EMIT_TURN_DESCRIPTION,
      input_schema: EMIT_TURN_INPUT_SCHEMA,
    },
  ];

  const messages: ChatMessage[] = [
    ...input.history,
    { role: 'user', content: input.userMessage },
  ];

  let usage: LLMUsage | undefined;
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const result = await input.llm.complete({
      model: input.model,
      maxTokens: input.maxTokens,
      system: input.system,
      messages,
      tools,
      toolChoice: { type: 'tool', name: 'emit_turn' },
      telemetry: { purpose: input.purpose ?? 'plan', iteration: attempt },
    });
    usage = addUsage(usage, result.usage);

    const call = result.toolCalls.find((entry) => entry.name === 'emit_turn');
    if (call) {
      const parsed = EmitTurnSchema.safeParse(call.args);
      if (parsed.success) {
        return { turn: parsed.data, degraded: false, ...(usage ? { usage } : {}) };
      }
      if (attempt === 0) {
        messages.push({ role: 'assistant', content: '', toolCalls: [call] });
        messages.push({
          role: 'user',
          content: '',
          toolResults: [
            {
              toolUseId: call.id,
              content: JSON.stringify({
                error: `Invalid emit_turn payload: ${parsed.error.issues
                  .map((issue) => `${issue.path.join('.')}: ${issue.message}`)
                  .join('; ')}. Call emit_turn again with a valid payload.`,
              }),
            },
          ],
        });
        continue;
      }
    }

    // No/invalid structured output on the final attempt: degrade gracefully.
    const text = result.text.trim();
    return {
      turn: text
        ? { mode: 'reply', text }
        : {
            mode: 'refuse',
            reason: 'I could not work out how to handle that request — please rephrase it.',
          },
      degraded: true,
      ...(usage ? { usage } : {}),
    };
  }

  // Unreachable (loop returns), but the compiler wants a tail.
  return {
    turn: { mode: 'refuse', reason: 'Planning failed.' },
    degraded: true,
  };
}
