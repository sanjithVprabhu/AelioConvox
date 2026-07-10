import type { AelioDatabase } from '@aelio/db';
import type { HarnessConfig, PlanRecord } from './types.js';
import { BudgetExceededError } from './types.js';
import { getPlan, incrementTokenSpend } from './store.js';

export async function checkBudget(
  db: AelioDatabase['db'],
  planId: string,
  config: HarnessConfig,
): Promise<boolean> {
  const plan = await getPlan(db, planId);
  if (!plan) {
    return false;
  }
  return evaluatePlanBudget(plan, config);
}

export function evaluatePlanBudget(plan: PlanRecord, config: HarnessConfig): boolean {
  const elapsedMs = Date.now() - plan.createdAt.getTime();
  if (plan.tokenSpend > config.tokenBudget) {
    return false;
  }
  if (plan.replanCount > config.replanCap) {
    return false;
  }
  if (elapsedMs > config.wallClockMs) {
    return false;
  }
  return true;
}

export async function assertBudget(
  db: AelioDatabase['db'],
  planId: string,
  config: HarnessConfig,
): Promise<void> {
  const ok = await checkBudget(db, planId, config);
  if (!ok) {
    throw new BudgetExceededError('Plan budget exceeded');
  }
}

export async function recordLlmTokenSpend(
  db: AelioDatabase['db'],
  planId: string,
  usage: { inputTokens: number; outputTokens: number } | undefined,
): Promise<void> {
  if (!usage) {
    return;
  }
  await incrementTokenSpend(db, planId, usage.inputTokens + usage.outputTokens);
}
