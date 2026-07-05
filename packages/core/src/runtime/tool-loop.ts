import type { FunctionDefinition, InvocationContext } from '@aelio/protocol';
import type { ChatMessage, LLMProvider, ToolDefinition } from '@aelio/llm';
import type { SdkBridge } from '../sdk-bridge/types.js';
import { logFunctionCall } from '../audit/function-calls.js';
import type { AelioDatabase } from '@aelio/db';
import {
  buildConfirmationPrompt,
  type PendingConfirmation,
} from '../safety/confirmations.js';
import { evaluateSafety, type SafetyConfig } from '../safety/policy.js';
import { buildInputSchema, coerceArgs, findMissingRequiredArgs } from './tool-schema.js';

const MAX_TOOL_ITERATIONS = 5;

function toToolDefinitions(functions: FunctionDefinition[]): ToolDefinition[] {
  return functions.map((fn) => ({
    name: fn.name,
    description: fn.description,
    input_schema: buildInputSchema(fn.params),
  }));
}

export type ToolLoopResult = {
  reply: string;
  toolCallsExecuted: number;
  executedToolNames: string[];
  pendingConfirmation?: PendingConfirmation;
};

export async function runToolLoop(input: {
  database?: AelioDatabase;
  internalCustomerId?: string;
  llm: LLMProvider;
  sdk: SdkBridge;
  functions?: FunctionDefinition[];
  model: string;
  maxTokens: number;
  system: string;
  history: ChatMessage[];
  userMessage: string;
  context: InvocationContext;
  safety: SafetyConfig;
}): Promise<ToolLoopResult> {
  const functions = input.functions ?? input.sdk.getFunctions();
  const tools = toToolDefinitions(functions);
  const conversation: ChatMessage[] = [
    ...input.history,
    { role: 'user', content: input.userMessage },
  ];

  let toolCallsExecuted = 0;
  const executedToolNames: string[] = [];

  for (let iteration = 0; iteration < MAX_TOOL_ITERATIONS; iteration += 1) {
    const result = await input.llm.complete({
      model: input.model,
      maxTokens: input.maxTokens,
      system: input.system,
      messages: conversation,
      tools,
      telemetry: {
        purpose: iteration === 0 ? 'chat_completion' : 'tool_synthesis',
        iteration,
      },
    });

    if (result.toolCalls.length === 0) {
      return {
        reply: result.text || 'I could not generate a response right now.',
        toolCallsExecuted,
        executedToolNames,
      };
    }

    conversation.push({
      role: 'assistant',
      content: result.text,
      toolCalls: result.toolCalls,
    });

    const toolResults = [];
    for (const toolCall of result.toolCalls) {
      const fn = functions.find((entry) => entry.name === toolCall.name);
      if (!fn) {
        toolResults.push({
          toolUseId: toolCall.id,
          content: JSON.stringify({ error: `Function ${toolCall.name} not found` }),
        });
        continue;
      }

      // Guard: never invoke the dev's handler with required arguments missing.
      // Hand the gap back to the model so it can ask the user, instead of running
      // the function with undefined inputs.
      const missing = findMissingRequiredArgs(fn.params, toolCall.args);
      if (missing.length > 0) {
        toolResults.push({
          toolUseId: toolCall.id,
          content: JSON.stringify({
            error: `Missing required argument(s): ${missing.join(', ')}. Ask the user for the missing value(s) before calling ${fn.name} again.`,
          }),
        });
        continue;
      }

      // Coerce/validate arguments against the declared types ("5" -> 5, "true" ->
      // true). Genuinely wrong shapes are handed back to the model to correct.
      const coercion = coerceArgs(fn.params, toolCall.args);
      if (coercion.errors.length > 0) {
        toolResults.push({
          toolUseId: toolCall.id,
          content: JSON.stringify({
            error: `Invalid argument(s) for ${fn.name}: ${coercion.errors.join('; ')}. Fix and call again.`,
          }),
        });
        continue;
      }
      const args = coercion.args;

      const decision = evaluateSafety(fn, input.safety);
      if (!decision.allowed) {
        if (input.database && input.internalCustomerId) {
          await logFunctionCall(input.database.db, {
            sessionId: input.context.sessionId,
            customerId: input.internalCustomerId,
            functionName: fn.name,
            args,
            status: 'blocked',
            safetyLevel: decision.effectiveSafety,
            errorMessage: decision.reason,
          });
        }
        return {
          reply: decision.reason,
          toolCallsExecuted,
          executedToolNames,
        };
      }

      if (decision.requiresConfirmation) {
        if (input.database && input.internalCustomerId) {
          await logFunctionCall(input.database.db, {
            sessionId: input.context.sessionId,
            customerId: input.internalCustomerId,
            functionName: fn.name,
            args,
            status: 'pending',
            safetyLevel: decision.effectiveSafety,
            requiredConfirmation: true,
          });
        }
        return {
          reply: buildConfirmationPrompt(fn, args),
          toolCallsExecuted,
          executedToolNames,
          pendingConfirmation: {
            functionName: fn.name,
            args,
            description: fn.description,
            safetyLevel: 'write',
            createdAt: Date.now(),
          },
        };
      }

      const invokeResult = await input.sdk.invoke(toolCall.name, args, input.context);
      toolCallsExecuted += 1;
      executedToolNames.push(toolCall.name);

      if (input.database && input.internalCustomerId) {
        await logFunctionCall(input.database.db, {
          sessionId: input.context.sessionId,
          customerId: input.internalCustomerId,
          functionName: fn.name,
          args,
          result: invokeResult.data,
          status: invokeResult.ok ? 'success' : 'error',
          safetyLevel: decision.effectiveSafety,
          durationMs: invokeResult.durationMs,
          errorMessage: invokeResult.error,
        });
      }

      toolResults.push({
        toolUseId: toolCall.id,
        content: invokeResult.ok
          ? JSON.stringify(invokeResult.data)
          : JSON.stringify({ error: invokeResult.error ?? 'SDK invocation failed' }),
      });
    }

    conversation.push({
      role: 'user',
      content: '',
      toolResults,
    });
  }

  return {
    reply: 'I need a moment — please try again with a simpler request.',
    toolCallsExecuted,
    executedToolNames,
  };
}