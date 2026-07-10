import type { PolicyDefinition } from '@aelio/protocol';
import { selectRelevantTools } from '../runtime/tool-retrieval.js';
import { routeIntent, shouldUseHarnessAfterRoute } from './router.js';
import { flowToPlan, matchFlow } from './flow-match.js';
import { generatePlan } from './planner.js';
import { executePlan, buildAbortReply, buildHarnessConfirmationReply, resumeHarnessStep } from './executor.js';
import { synthesizeReply } from './synthesize.js';
import {
  createPlan,
  getPlan,
  loadPlanSteps,
  persistPlanSteps,
  updatePlanStatus,
} from './store.js';
import { assertBudget, checkBudget } from './budget.js';
import { assertValidPlan } from './cycle-detect.js';
import type { HarnessTurnInput, HarnessTurnResult } from './types.js';
import { BudgetExceededError, PlanValidationError } from './types.js';

function resolveModel(configured: string | undefined, fallback: string): string {
  return configured?.trim() || fallback;
}

export async function runHarnessTurn(input: HarnessTurnInput): Promise<HarnessTurnResult> {
  const db = input.database.db;
  const routerModel = resolveModel(input.harness.routerModel, input.model);
  const plannerModel = resolveModel(input.harness.plannerModel, input.model);
  const synthesisModel = resolveModel(input.harness.synthesisModel, input.model);

  const plan = await createPlan(db, {
    sessionId: input.sessionId,
    customerId: input.internalCustomerId,
    turnId: input.turnId,
    userMessage: input.userMessage,
  });

  let route;
  try {
    route = await routeIntent({
      llm: input.llm,
      model: routerModel,
      userMessage: input.userMessage,
      database: db,
      planId: plan.id,
    });
    await assertBudget(db, plan.id, input.harness);
  } catch (error) {
    if (error instanceof BudgetExceededError) {
      await updatePlanStatus(db, plan.id, 'aborted', { abortReason: 'budget_exceeded' });
      return {
        reply: buildAbortReply('budget exceeded'),
        toolCallsExecuted: 0,
        executedToolNames: [],
        planId: plan.id,
        usedHarness: true,
      };
    }
    throw error;
  }

  const hasActiveFlow = Boolean(input.activeFlowId);
  if (!shouldUseHarnessAfterRoute({ route, config: input.harness, hasActiveFlow })) {
    await updatePlanStatus(db, plan.id, 'aborted', { abortReason: 'simple_turn' });
    return {
      reply: '',
      toolCallsExecuted: 0,
      executedToolNames: [],
      usedHarness: false,
    };
  }

  const policies = input.sdk.getPolicies().map((policy: PolicyDefinition) => policy.description);
  const retrievedTools = await selectRelevantTools(input.functions, route.intent, {
    topK: input.harness.toolRetrievalK,
    database: db,
  });

  let generatedPlan;
  const flowMatch = await matchFlow(
    route.intent,
    input.sdk.getFlows(),
    input.harness.flowConfidenceThreshold,
  );

  try {
    if (flowMatch) {
      generatedPlan = flowToPlan(flowMatch.flow);
      await updatePlanStatus(db, plan.id, 'pending', { matchedFlowId: flowMatch.flow.id });
    } else {
      if (!(await checkBudget(db, plan.id, input.harness))) {
        throw new BudgetExceededError('budget exceeded before planning');
      }
      generatedPlan = await generatePlan({
        llm: input.llm,
        model: plannerModel,
        maxTokens: input.maxTokens,
        userMessage: input.userMessage,
        tools: retrievedTools,
        policies,
        config: input.harness,
        database: db,
        planId: plan.id,
        flowHint: undefined,
      });
    }

    assertValidPlan(generatedPlan, input.harness.planStepCap);
    await persistPlanSteps(db, plan.id, generatedPlan);
    await assertBudget(db, plan.id, input.harness);
  } catch (error) {
    const reason =
      error instanceof PlanValidationError || error instanceof BudgetExceededError
        ? error.message
        : 'planning failed';
    await updatePlanStatus(db, plan.id, 'aborted', { abortReason: reason });
    return {
      reply: buildAbortReply(reason),
      toolCallsExecuted: 0,
      executedToolNames: [],
      planId: plan.id,
      usedHarness: true,
    };
  }

  const execution = await executePlan({
    database: input.database,
    sdk: input.sdk,
    functions: input.functions,
    context: input.context,
    internalCustomerId: input.internalCustomerId,
    safety: input.safety,
    config: input.harness,
    planId: plan.id,
    llm: input.llm,
    model: plannerModel,
    maxTokens: input.maxTokens,
    policies,
    userMessage: input.userMessage,
  });

  if (execution.status === 'awaiting_confirmation' && execution.pendingConfirmation) {
    const fn = input.functions.find(
      (entry) => entry.name === execution.pendingConfirmation!.functionName,
    );
    return {
      reply: fn
        ? buildHarnessConfirmationReply(fn, execution.pendingConfirmation.args)
        : `Reply **yes** to confirm or **no** to cancel.`,
      toolCallsExecuted: execution.toolCallsExecuted,
      executedToolNames: execution.executedToolNames,
      planId: plan.id,
      pendingConfirmation: execution.pendingConfirmation,
      usedHarness: true,
    };
  }

  const steps = await loadPlanSteps(db, plan.id);
  let reply: string;

  if (execution.status === 'done') {
    reply = await synthesizeReply({
      llm: input.llm,
      model: synthesisModel,
      maxTokens: input.maxTokens,
      userMessage: input.userMessage,
      steps,
      database: db,
      planId: plan.id,
    });
  } else {
    const abortReason =
      execution.failure?.error ??
      (execution.status === 'aborted' ? 'replan limit or budget exceeded' : 'execution failed');
    reply = await synthesizeReply({
      llm: input.llm,
      model: synthesisModel,
      maxTokens: input.maxTokens,
      userMessage: input.userMessage,
      steps,
      database: db,
      planId: plan.id,
      abortReason,
    });
  }

  return {
    reply,
    toolCallsExecuted: execution.toolCallsExecuted,
    executedToolNames: execution.executedToolNames,
    planId: plan.id,
    usedHarness: true,
  };
}

export { resumeHarnessStep } from './executor.js';

export async function resumeHarnessAfterConfirmation(input: {
  database: import('@aelio/db').AelioDatabase;
  llm: import('@aelio/llm').LLMProvider;
  sdk: import('../sdk-bridge/types.js').SdkBridge;
  model: string;
  maxTokens: number;
  harness: import('./types.js').HarnessConfig;
  internalCustomerId: string;
  sessionId: string;
  turnId: string;
  userMessage: string;
  context: import('@aelio/protocol').InvocationContext;
  safety: import('../safety/policy.js').SafetyConfig;
  functions: import('@aelio/protocol').FunctionDefinition[];
  planId: string;
  stepId: string;
  fn: import('@aelio/protocol').FunctionDefinition;
  args: Record<string, unknown>;
}): Promise<import('./types.js').HarnessTurnResult> {
  const db = input.database.db;
  const plannerModel = input.harness.plannerModel?.trim() || input.model;
  const synthesisModel = input.harness.synthesisModel?.trim() || input.model;
  const policies = input.sdk.getPolicies().map((policy) => policy.description);
  const plan = await getPlan(db, input.planId);
  const userMessage = plan?.userMessage ?? input.userMessage;

  const execution = await resumeHarnessStep({
    database: input.database,
    sdk: input.sdk,
    functions: input.functions,
    context: input.context,
    internalCustomerId: input.internalCustomerId,
    safety: input.safety,
    config: input.harness,
    planId: input.planId,
    stepId: input.stepId,
    fn: input.fn,
    args: input.args,
    llm: input.llm,
    model: plannerModel,
    maxTokens: input.maxTokens,
    policies,
    userMessage,
  });

  if (execution.status === 'awaiting_confirmation' && execution.pendingConfirmation) {
    return {
      reply: buildHarnessConfirmationReply(input.fn, execution.pendingConfirmation.args),
      toolCallsExecuted: execution.toolCallsExecuted,
      executedToolNames: execution.executedToolNames,
      planId: input.planId,
      pendingConfirmation: execution.pendingConfirmation,
      usedHarness: true,
    };
  }

  const steps = await loadPlanSteps(db, input.planId);
  let reply: string;

  if (execution.status === 'done') {
    reply = await synthesizeReply({
      llm: input.llm,
      model: synthesisModel,
      maxTokens: input.maxTokens,
      userMessage,
      steps,
      database: db,
      planId: input.planId,
    });
  } else {
    reply = await synthesizeReply({
      llm: input.llm,
      model: synthesisModel,
      maxTokens: input.maxTokens,
      userMessage,
      steps,
      database: db,
      planId: input.planId,
      abortReason: execution.failure?.error ?? 'execution failed after confirmation',
    });
  }

  return {
    reply,
    toolCallsExecuted: execution.toolCallsExecuted + 1,
    executedToolNames: [...execution.executedToolNames, input.fn.name],
    planId: input.planId,
    usedHarness: true,
  };
}

export {
  DEFAULT_HARNESS_CONFIG,
  type HarnessConfig,
  type HarnessTurnResult,
} from './types.js';

export { updatePlanStatus } from './store.js';
