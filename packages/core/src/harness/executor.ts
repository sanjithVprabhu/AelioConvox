import type { FunctionDefinition, InvocationContext } from '@aelio/protocol';
import type { AelioDatabase } from '@aelio/db';
import type { LLMProvider } from '@aelio/llm';
import type { SdkBridge } from '../sdk-bridge/types.js';
import type { SafetyConfig } from '../safety/policy.js';
import type { HarnessConfig, PlanStepRecord } from './types.js';
import { BudgetExceededError } from './types.js';
import {
  findSuccessfulStepByIdempotencyKey,
  getPlan,
  loadPlanSteps,
  updatePlanStatus,
  updatePlanStep,
} from './store.js';
import { checkBudget } from './budget.js';
import { topologicalOrder } from './cycle-detect.js';
import { resolveReferences } from './references.js';
import { logFunctionCall } from '../audit/function-calls.js';
import { evaluateSafety } from '../safety/policy.js';
import {
  buildConfirmationPrompt,
  type PendingConfirmation,
} from '../safety/confirmations.js';
import { coerceArgs, findMissingRequiredArgs } from '../runtime/tool-schema.js';
import { replanFromFailure } from './replan.js';

const MAX_STEP_ATTEMPTS = 3;

export type ExecutePlanResult = {
  status: 'done' | 'awaiting_confirmation' | 'failed' | 'aborted';
  executedToolNames: string[];
  toolCallsExecuted: number;
  pendingConfirmation?: PendingConfirmation;
  failure?: { step: PlanStepRecord; error: string };
};

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function backoffMs(attempt: number): number {
  return Math.min(1000 * 2 ** (attempt - 1), 8000);
}

function isRetryable(error: string): boolean {
  const normalized = error.toUpperCase();
  return (
    normalized.includes('ETIMEDOUT') ||
    normalized.includes('ECONNRESET') ||
    normalized.includes('RATE_LIMIT') ||
    normalized.includes('TIMEOUT')
  );
}

async function invokeTool(input: {
  db: AelioDatabase['db'];
  sdk: SdkBridge;
  fn: FunctionDefinition;
  args: Record<string, unknown>;
  context: InvocationContext;
  internalCustomerId: string;
  idempotencyKey: string;
  safety: SafetyConfig;
}): Promise<{ ok: true; data: unknown } | { ok: false; error: string; retryable: boolean }> {
  const existing = await findSuccessfulStepByIdempotencyKey(input.db, input.idempotencyKey);
  if (existing?.output !== null && existing?.output !== undefined) {
    return { ok: true, data: existing.output };
  }

  const decision = evaluateSafety(input.fn, input.safety);
  if (!decision.allowed) {
    return { ok: false, error: decision.reason ?? 'Action blocked', retryable: false };
  }

  const invokeResult = await input.sdk.invoke(input.fn.name, input.args, input.context);
  await logFunctionCall(input.db, {
    sessionId: input.context.sessionId,
    customerId: input.internalCustomerId,
    functionName: input.fn.name,
    args: input.args,
    result: invokeResult.data,
    status: invokeResult.ok ? 'success' : 'error',
    safetyLevel: decision.effectiveSafety,
    requiredConfirmation: decision.requiresConfirmation,
    durationMs: invokeResult.durationMs,
    errorMessage: invokeResult.error,
  });

  if (!invokeResult.ok) {
    return {
      ok: false,
      error: invokeResult.error ?? 'SDK invocation failed',
      retryable: isRetryable(invokeResult.error ?? ''),
    };
  }

  return { ok: true, data: invokeResult.data };
}

export async function executePlan(input: {
  database: AelioDatabase;
  sdk: SdkBridge;
  functions: FunctionDefinition[];
  context: InvocationContext;
  internalCustomerId: string;
  safety: SafetyConfig;
  config: HarnessConfig;
  planId: string;
  llm: LLMProvider;
  model: string;
  maxTokens: number;
  policies: string[];
  userMessage: string;
}): Promise<ExecutePlanResult> {
  const db = input.database.db;
  const plan = await getPlan(db, input.planId);
  if (!plan) {
    return { status: 'failed', executedToolNames: [], toolCallsExecuted: 0 };
  }

  await updatePlanStatus(db, input.planId, 'running');

  const executedToolNames: string[] = [];
  let toolCallsExecuted = 0;

  const steps = await loadPlanSteps(db, input.planId);
  const ordered = topologicalOrder(steps).sort((a, b) => a.stepOrder - b.stepOrder);

  for (const step of ordered) {
    if (step.status === 'success' || step.status === 'skipped') {
      continue;
    }

    const budgetOk = await checkBudget(db, input.planId, input.config);
    if (!budgetOk) {
      await updatePlanStatus(db, input.planId, 'aborted', { abortReason: 'budget_exceeded' });
      throw new BudgetExceededError('Plan budget exceeded during execution');
    }

    const fn = input.functions.find((entry) => entry.name === step.toolName);
    if (!fn) {
      await updatePlanStep(db, step.id, {
        status: 'failed',
        errorMessage: `Tool ${step.toolName} not found`,
        finishedAt: new Date(),
      });
      return handleStepFailure(input, step, `Tool ${step.toolName} not found`, executedToolNames, toolCallsExecuted);
    }

    const currentSteps = await loadPlanSteps(db, input.planId);
    let resolvedInput: Record<string, unknown>;
    try {
      resolvedInput = resolveReferences(step.inputTemplate, currentSteps);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      await updatePlanStep(db, step.id, {
        status: 'failed',
        errorMessage: message,
        finishedAt: new Date(),
      });
      return handleStepFailure(input, step, message, executedToolNames, toolCallsExecuted);
    }

    const missing = findMissingRequiredArgs(fn.params, resolvedInput);
    if (missing.length > 0) {
      const message = `Missing required argument(s): ${missing.join(', ')}`;
      await updatePlanStep(db, step.id, {
        status: 'failed',
        errorMessage: message,
        finishedAt: new Date(),
      });
      return handleStepFailure(input, step, message, executedToolNames, toolCallsExecuted);
    }

    const coercion = coerceArgs(fn.params, resolvedInput);
    if (coercion.errors.length > 0) {
      const message = coercion.errors.join('; ');
      await updatePlanStep(db, step.id, {
        status: 'failed',
        errorMessage: message,
        finishedAt: new Date(),
      });
      return handleStepFailure(input, step, message, executedToolNames, toolCallsExecuted);
    }
    resolvedInput = coercion.args;

    const decision = evaluateSafety(fn, input.safety);
    if (!decision.allowed) {
      const message = decision.reason ?? 'Action blocked';
      await updatePlanStep(db, step.id, {
        status: 'failed',
        errorMessage: message,
        finishedAt: new Date(),
      });
      return handleStepFailure(input, step, message, executedToolNames, toolCallsExecuted);
    }
    if (decision.requiresConfirmation) {
      await updatePlanStep(db, step.id, {
        status: 'pending',
        resolvedInput,
      });
      await updatePlanStatus(db, input.planId, 'awaiting_confirmation');
      return {
        status: 'awaiting_confirmation',
        executedToolNames,
        toolCallsExecuted,
        pendingConfirmation: {
          functionName: fn.name,
          args: resolvedInput,
          description: fn.description,
          safetyLevel: 'write',
          createdAt: Date.now(),
          harnessPlanId: input.planId,
          harnessStepId: step.id,
        },
      };
    }

    await updatePlanStep(db, step.id, {
      resolvedInput,
      status: 'running',
      startedAt: new Date(),
      attemptCount: step.attemptCount + 1,
    });

    const result = await invokeTool({
      db,
      sdk: input.sdk,
      fn,
      args: resolvedInput,
      context: input.context,
      internalCustomerId: input.internalCustomerId,
      idempotencyKey: step.idempotencyKey,
      safety: input.safety,
    });

    if (!result.ok) {
      const attemptCount = step.attemptCount + 1;
      if (result.retryable && attemptCount < MAX_STEP_ATTEMPTS) {
        await updatePlanStep(db, step.id, {
          status: 'pending',
          attemptCount,
          errorMessage: result.error,
        });
        await sleep(backoffMs(attemptCount));
        return executePlan(input);
      }
      await updatePlanStep(db, step.id, {
        status: 'failed',
        errorMessage: result.error,
        finishedAt: new Date(),
        attemptCount,
      });
      return handleStepFailure(input, step, result.error, executedToolNames, toolCallsExecuted);
    }

    await updatePlanStep(db, step.id, {
      output: result.data,
      status: 'success',
      finishedAt: new Date(),
    });
    executedToolNames.push(fn.name);
    toolCallsExecuted += 1;
  }

  await updatePlanStatus(db, input.planId, 'done');
  return {
    status: 'done',
    executedToolNames,
    toolCallsExecuted,
  };
}

async function handleStepFailure(
  input: {
    database: AelioDatabase;
    sdk: SdkBridge;
    functions: FunctionDefinition[];
    context: InvocationContext;
    internalCustomerId: string;
    safety: SafetyConfig;
    config: HarnessConfig;
    planId: string;
    llm: LLMProvider;
    model: string;
    maxTokens: number;
    policies: string[];
    userMessage: string;
  },
  step: PlanStepRecord,
  error: string,
  executedToolNames: string[],
  toolCallsExecuted: number,
): Promise<ExecutePlanResult> {
  const replanResult = await replanFromFailure({
    database: input.database,
    planId: input.planId,
    failedStep: step,
    error,
    config: input.config,
    functions: input.functions,
    policies: input.policies,
    userMessage: input.userMessage,
    llm: input.llm,
    model: input.model,
    maxTokens: input.maxTokens,
  });

  if (replanResult === 'replan_limit') {
    await updatePlanStatus(input.database.db, input.planId, 'aborted', {
      abortReason: 'replan_limit_exceeded',
    });
    return {
      status: 'aborted',
      executedToolNames,
      toolCallsExecuted,
      failure: { step, error },
    };
  }

  if (replanResult === 'replanned') {
    return executePlan(input);
  }

  await updatePlanStatus(input.database.db, input.planId, 'failed', { abortReason: error });
  return {
    status: 'failed',
    executedToolNames,
    toolCallsExecuted,
    failure: { step, error },
  };
}

export async function resumeHarnessStep(input: {
  database: AelioDatabase;
  sdk: SdkBridge;
  functions: FunctionDefinition[];
  context: InvocationContext;
  internalCustomerId: string;
  safety: SafetyConfig;
  config: HarnessConfig;
  planId: string;
  stepId: string;
  fn: FunctionDefinition;
  args: Record<string, unknown>;
  llm: LLMProvider;
  model: string;
  maxTokens: number;
  policies: string[];
  userMessage: string;
}): Promise<ExecutePlanResult> {
  const db = input.database.db;
  const steps = await loadPlanSteps(db, input.planId);
  const step = steps.find((entry) => entry.id === input.stepId);
  if (!step) {
    return { status: 'failed', executedToolNames: [], toolCallsExecuted: 0 };
  }

  await updatePlanStep(db, step.id, {
    resolvedInput: input.args,
    status: 'running',
    startedAt: new Date(),
    attemptCount: step.attemptCount + 1,
  });
  await updatePlanStatus(db, input.planId, 'running');

  const result = await invokeTool({
    db,
    sdk: input.sdk,
    fn: input.fn,
    args: input.args,
    context: input.context,
    internalCustomerId: input.internalCustomerId,
    idempotencyKey: step.idempotencyKey,
    safety: input.safety,
  });

  if (!result.ok) {
    await updatePlanStep(db, step.id, {
      status: 'failed',
      errorMessage: result.error,
      finishedAt: new Date(),
    });
    return handleStepFailure(
      input,
      step,
      result.error,
      [input.fn.name],
      0,
    );
  }

  await updatePlanStep(db, step.id, {
    output: result.data,
    status: 'success',
    finishedAt: new Date(),
  });

  return executePlan(input);
}

export function buildAbortReply(reason: string): string {
  return `I couldn't complete that request (${reason}). Please try again or simplify your request.`;
}

export function buildHarnessConfirmationReply(
  fn: FunctionDefinition,
  args: Record<string, unknown>,
): string {
  return buildConfirmationPrompt(fn, args);
}
