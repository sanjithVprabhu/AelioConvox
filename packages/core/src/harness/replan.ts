import type { FunctionDefinition } from '@aelio/protocol';
import type { LLMProvider } from '@aelio/llm';
import type { AelioDatabase } from '@aelio/db';
import type { HarnessConfig, PlanStepRecord } from './types.js';
import { deleteReplannablePlanSteps, getPlan, incrementReplanCount, persistPlanSteps } from './store.js';
import { generatePlan } from './planner.js';

export async function replanFromFailure(input: {
  database: AelioDatabase;
  planId: string;
  failedStep: PlanStepRecord;
  error: string;
  config: HarnessConfig;
  functions: FunctionDefinition[];
  policies: string[];
  userMessage: string;
  llm: LLMProvider;
  model: string;
  maxTokens: number;
}): Promise<'replanned' | 'replan_limit' | 'no_replan'> {
  const db = input.database.db;
  const plan = await getPlan(db, input.planId);
  if (!plan) {
    return 'no_replan';
  }

  const replanCount = await incrementReplanCount(db, input.planId);
  if (replanCount > input.config.replanCap) {
    return 'replan_limit';
  }

  const failureContext = `Step ${input.failedStep.stepOrder} (${input.failedStep.toolName}) failed: ${input.error}`;

  await deleteReplannablePlanSteps(db, input.planId);

  const newPlan = await generatePlan({
    llm: input.llm,
    model: input.model,
    maxTokens: input.maxTokens,
    userMessage: input.userMessage,
    tools: input.functions,
    policies: input.policies,
    config: input.config,
    database: db,
    planId: input.planId,
    failureContext,
  });

  await persistPlanSteps(db, input.planId, newPlan);
  return 'replanned';
}
