import { createHash } from 'node:crypto';
import type { FunctionDefinition, InvocationContext } from '@aelio/protocol';
import type { ChatMessage, LLMProvider } from '@aelio/llm';
import type { AelioDatabase } from '@aelio/db';
import { logFunctionCall } from '../audit/function-calls.js';
import { buildConfirmationPrompt } from '../safety/confirmations.js';
import { evaluateSafety, type SafetyConfig } from '../safety/policy.js';
import type { SdkBridge } from '../sdk-bridge/types.js';
import { coerceArgs, findMissingRequiredArgs } from '../runtime/tool-schema.js';
import type { ToolLoopResult } from '../runtime/tool-loop.js';
import type { LighthouseService } from '../lighthouse/index.js';
import { BudgetMeter } from './budgets.js';
import { runPlanner } from './planner.js';
import { runSynthesis } from './synthesis.js';
import type { HarnessTracer } from './traces.js';
import {
  DEFAULT_BINDING,
  DEFAULT_BUDGETS,
  type EmitTurn,
  type HarnessBindingConfig,
  type HarnessBudgets,
  type LedgerEntry,
} from './schema.js';

export { runPlanner } from './planner.js';
export { runSynthesis } from './synthesis.js';
export { BudgetMeter } from './budgets.js';

export type HarnessRunInput = {
  database?: AelioDatabase;
  internalCustomerId?: string;
  llm: LLMProvider;
  sdk: SdkBridge;
  /** State-scoped, relevance-filtered tool set for this turn. */
  functions: FunctionDefinition[];
  model: string;
  maxTokens: number;
  system: string;
  history: ChatMessage[];
  userMessage: string;
  context: InvocationContext;
  safety: SafetyConfig;
  lighthouse?: LighthouseService;
  tracer?: HarnessTracer;
  budgets?: HarnessBudgets;
  binding?: HarnessBindingConfig;
  turnId?: string;
};

export function hashArgs(args: Record<string, unknown>): string {
  return createHash('sha256').update(JSON.stringify(args ?? {})).digest('hex').slice(0, 16);
}

/**
 * The harness: plan → execute → synthesize, with the LLM proposing and
 * deterministic code disposing. Returns the same result shape as the legacy
 * tool loop so the surrounding turn (confirmations, intent stack, persistence)
 * is agnostic to which engine ran.
 */
export async function runHarness(input: HarnessRunInput): Promise<ToolLoopResult> {
  const budgets = new BudgetMeter(input.budgets ?? DEFAULT_BUDGETS);
  const binding = input.binding ?? DEFAULT_BINDING;
  const trace = (kind: Parameters<HarnessTracer['trace']>[0]['kind'], payload: unknown) =>
    input.tracer?.trace({
      turnId: input.turnId ?? 'unknown',
      sessionId: input.context.sessionId,
      kind,
      payload,
    });

  // ---- Pass 1: merged router + planner ----
  let planner = await runPlanner({
    llm: input.llm,
    model: input.model,
    maxTokens: input.maxTokens,
    system: input.system,
    history: input.history,
    userMessage: input.userMessage,
  });
  trace('plan', { turn: planner.turn, degraded: planner.degraded });

  // ---- Refusal: fail-open on feasibility ----
  if (planner.turn.mode === 'refuse' && input.lighthouse && !planner.degraded) {
    const score = await input.lighthouse.probeFeasibility(input.userMessage);
    if (score >= binding.scoreMin) {
      // Something in the index resembles the ask — give the planner one more
      // look with that knowledge before surfacing a "cannot do".
      const replanCheck = budgets.noteReplan();
      if (replanCheck.ok) {
        planner = await runPlanner({
          llm: input.llm,
          model: input.model,
          maxTokens: input.maxTokens,
          system: `${input.system}\n\nNote: the capability index suggests this request MAY be servable with the available tools. Refuse only if you are certain it is not; otherwise emit a plan.`,
          history: input.history,
          userMessage: input.userMessage,
          purpose: 'replan',
        });
        trace('repair', { reason: 'feasibility_probe', score, turn: planner.turn });
      }
    }
  }

  if (planner.turn.mode === 'reply') {
    return { reply: planner.turn.text, toolCallsExecuted: 0, executedToolNames: [] };
  }
  if (planner.turn.mode === 'refuse') {
    return { reply: planner.turn.reason, toolCallsExecuted: 0, executedToolNames: [] };
  }

  // ---- Deep path ----
  const plan: Extract<EmitTurn, { mode: 'plan' }> = planner.turn;
  const sizeCheck = budgets.checkPlanSize(plan.instructions.length);
  if (!sizeCheck.ok) {
    trace('budget', sizeCheck);
    return {
      reply: 'That request needs more steps than I can safely take in one go — could you split it into smaller asks?',
      toolCallsExecuted: 0,
      executedToolNames: [],
    };
  }

  const registryByName = new Map(input.functions.map((fn) => [fn.name, fn]));
  const ledger: LedgerEntry[] = [];
  const executedToolNames: string[] = [];
  let toolCallsExecuted = 0;

  for (const instruction of plan.instructions) {
    // ---- Bind: planner suggestion first, semantic search fallback ----
    let fn = instruction.tool ? registryByName.get(instruction.tool) : undefined;
    if (!fn && input.lighthouse) {
      const hits = await input.lighthouse.searchTools(instruction.capability, 2);
      const top = hits[0];
      const runner = hits[1];
      if (
        top &&
        top.score >= binding.scoreMin &&
        (!runner || top.score - runner.score >= binding.ambiguityGap) &&
        registryByName.has(top.fn.name)
      ) {
        fn = registryByName.get(top.fn.name);
      }
    }
    if (!fn) {
      ledger.push({
        instructionId: instruction.id,
        argsHash: '',
        status: 'error',
        result: `No available tool matches "${instruction.capability}"`,
        toolName: instruction.tool ?? 'unbound',
        durationMs: 0,
      });
      trace('bind', { instruction: instruction.id, bound: null });
      continue;
    }
    trace('bind', { instruction: instruction.id, bound: fn.name });

    // ---- Args: hints coerced against the schema ----
    const coercion = coerceArgs(fn.params, (instruction.args_hint ?? {}) as Record<string, unknown>);
    const args = coercion.args;
    const missing = findMissingRequiredArgs(fn.params, args);
    if (missing.length > 0 || coercion.errors.length > 0) {
      // Phase-5 recoil suspends the plan here; until then, ask inline.
      const need = missing.length > 0 ? missing : coercion.errors;
      trace('gate', { instruction: instruction.id, verdict: 'needs_info', need });
      return {
        reply: `To do that I still need: ${need.join(', ')}. Could you provide ${missing.length === 1 ? 'it' : 'them'}?`,
        toolCallsExecuted,
        executedToolNames,
      };
    }

    // ---- Gate: deterministic, on concrete args, regardless of what the plan says ----
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
      trace('gate', { instruction: instruction.id, verdict: 'deny_fatal', reason: decision.reason });
      return { reply: decision.reason, toolCallsExecuted, executedToolNames };
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
      trace('gate', { instruction: instruction.id, verdict: 'needs_approval', tool: fn.name });
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

    // ---- Invoke, with budget + progress protection ----
    const argsHash = hashArgs(args);
    const budgetCheck = budgets.noteToolCall(fn.name, argsHash);
    if (!budgetCheck.ok) {
      trace('budget', budgetCheck);
      break;
    }

    const invokeResult = await input.sdk.invoke(fn.name, args, input.context);
    toolCallsExecuted += 1;
    executedToolNames.push(fn.name);
    ledger.push({
      instructionId: instruction.id,
      argsHash,
      status: invokeResult.ok ? 'success' : 'error',
      result: invokeResult.ok ? invokeResult.data : invokeResult.error,
      toolName: fn.name,
      durationMs: invokeResult.durationMs,
    });

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
  }
  trace('wave', { executed: executedToolNames, ledgerSize: ledger.length });

  // ---- Synthesis ----
  const reply = await runSynthesis({
    llm: input.llm,
    model: input.model,
    maxTokens: input.maxTokens,
    system: input.system,
    history: input.history,
    userMessage: input.userMessage,
    goal: plan.goal,
    ledger,
  });
  trace('synthesis', { replyPreview: reply.slice(0, 200) });

  return { reply, toolCallsExecuted, executedToolNames };
}
