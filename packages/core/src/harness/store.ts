import { planSteps, plans } from '@aelio/db';
import type { AelioDatabase } from '@aelio/db';
import { and, asc, eq } from 'drizzle-orm';
import { randomUUID } from 'node:crypto';
import type {
  GeneratedPlan,
  PlanRecord,
  PlanStatus,
  PlanStepRecord,
  RoutedIntent,
  StepStatus,
} from './types.js';

function mapPlan(row: typeof plans.$inferSelect): PlanRecord {
  return {
    id: row.id,
    sessionId: row.sessionId,
    customerId: row.customerId,
    turnId: row.turnId,
    status: row.status as PlanRecord['status'],
    matchedFlowId: row.matchedFlowId ?? null,
    userMessage: row.userMessage,
    intent: row.intent ?? null,
    intentCategory: row.intentCategory ?? null,
    tokenSpend: row.tokenSpend,
    replanCount: row.replanCount,
    abortReason: row.abortReason ?? null,
    createdAt: row.createdAt,
    updatedAt: row.updatedAt,
  };
}

function mapStep(row: typeof planSteps.$inferSelect): PlanStepRecord {
  return {
    id: row.id,
    planId: row.planId,
    stepOrder: row.stepOrder,
    toolName: row.toolName,
    dependsOn: row.dependsOn ?? [],
    inputTemplate: row.inputTemplate,
    resolvedInput: row.resolvedInput ?? null,
    output: row.output ?? null,
    status: row.status as StepStatus,
    idempotencyKey: row.idempotencyKey,
    attemptCount: row.attemptCount,
    errorMessage: row.errorMessage ?? null,
    startedAt: row.startedAt ?? null,
    finishedAt: row.finishedAt ?? null,
  };
}

export async function createPlan(
  db: AelioDatabase['db'],
  input: {
    sessionId: string;
    customerId: string;
    turnId: string;
    userMessage: string;
    intent?: RoutedIntent;
  },
): Promise<PlanRecord> {
  const now = new Date();
  const id = randomUUID();
  await db.insert(plans).values({
    id,
    sessionId: input.sessionId,
    customerId: input.customerId,
    turnId: input.turnId,
    status: 'pending',
    userMessage: input.userMessage,
    intent: input.intent?.intent,
    intentCategory: input.intent?.category ?? undefined,
    tokenSpend: 0,
    replanCount: 0,
    createdAt: now,
    updatedAt: now,
  });
  return {
    id,
    sessionId: input.sessionId,
    customerId: input.customerId,
    turnId: input.turnId,
    status: 'pending',
    matchedFlowId: null,
    userMessage: input.userMessage,
    intent: input.intent?.intent ?? null,
    intentCategory: input.intent?.category ?? null,
    tokenSpend: 0,
    replanCount: 0,
    abortReason: null,
    createdAt: now,
    updatedAt: now,
  };
}

export async function getPlan(
  db: AelioDatabase['db'],
  planId: string,
): Promise<PlanRecord | null> {
  const rows = await db.select().from(plans).where(eq(plans.id, planId)).limit(1);
  return rows[0] ? mapPlan(rows[0]) : null;
}

export async function updatePlanStatus(
  db: AelioDatabase['db'],
  planId: string,
  status: PlanStatus,
  extras?: { abortReason?: string; matchedFlowId?: string },
): Promise<void> {
  await db
    .update(plans)
    .set({
      status,
      updatedAt: new Date(),
      ...(extras?.abortReason !== undefined ? { abortReason: extras.abortReason } : {}),
      ...(extras?.matchedFlowId !== undefined ? { matchedFlowId: extras.matchedFlowId } : {}),
    })
    .where(eq(plans.id, planId));
}

export async function incrementTokenSpend(
  db: AelioDatabase['db'],
  planId: string,
  tokens: number,
): Promise<number> {
  const plan = await getPlan(db, planId);
  if (!plan) {
    return 0;
  }
  const next = plan.tokenSpend + tokens;
  await db
    .update(plans)
    .set({ tokenSpend: next, updatedAt: new Date() })
    .where(eq(plans.id, planId));
  return next;
}

export async function incrementReplanCount(
  db: AelioDatabase['db'],
  planId: string,
): Promise<number> {
  const plan = await getPlan(db, planId);
  if (!plan) {
    return 0;
  }
  const next = plan.replanCount + 1;
  await db
    .update(plans)
    .set({ replanCount: next, updatedAt: new Date() })
    .where(eq(plans.id, planId));
  return next;
}

export async function persistPlanSteps(
  db: AelioDatabase['db'],
  planId: string,
  generated: GeneratedPlan,
): Promise<PlanStepRecord[]> {
  const stepIds: string[] = [];
  for (let index = 0; index < generated.steps.length; index += 1) {
    stepIds.push(randomUUID());
  }

  const records: PlanStepRecord[] = [];
  for (let index = 0; index < generated.steps.length; index += 1) {
    const step = generated.steps[index]!;
    const id = stepIds[index]!;
    const dependsOn = (step.depends_on_step_index ?? [])
      .map((depIndex) => stepIds[depIndex])
      .filter((depId): depId is string => Boolean(depId));
    const idempotencyKey = `${planId}:${index}:${step.tool_name}`;

    await db.insert(planSteps).values({
      id,
      planId,
      stepOrder: index,
      toolName: step.tool_name,
      dependsOn,
      inputTemplate: step.input,
      status: 'pending',
      idempotencyKey,
      attemptCount: 0,
    });

    records.push({
      id,
      planId,
      stepOrder: index,
      toolName: step.tool_name,
      dependsOn,
      inputTemplate: step.input,
      resolvedInput: null,
      output: null,
      status: 'pending',
      idempotencyKey,
      attemptCount: 0,
      errorMessage: null,
      startedAt: null,
      finishedAt: null,
    });
  }

  return records;
}

export async function loadPlanSteps(
  db: AelioDatabase['db'],
  planId: string,
): Promise<PlanStepRecord[]> {
  const rows = await db
    .select()
    .from(planSteps)
    .where(eq(planSteps.planId, planId))
    .orderBy(asc(planSteps.stepOrder));
  return rows.map(mapStep);
}

export async function updatePlanStep(
  db: AelioDatabase['db'],
  stepId: string,
  patch: Partial<{
    status: StepStatus;
    resolvedInput: Record<string, unknown>;
    output: unknown;
    attemptCount: number;
    errorMessage: string | null;
    startedAt: Date;
    finishedAt: Date;
  }>,
): Promise<void> {
  await db
    .update(planSteps)
    .set({
      ...(patch.status !== undefined ? { status: patch.status } : {}),
      ...(patch.resolvedInput !== undefined ? { resolvedInput: patch.resolvedInput } : {}),
      ...(patch.output !== undefined ? { output: patch.output } : {}),
      ...(patch.attemptCount !== undefined ? { attemptCount: patch.attemptCount } : {}),
      ...(patch.errorMessage !== undefined ? { errorMessage: patch.errorMessage } : {}),
      ...(patch.startedAt !== undefined ? { startedAt: patch.startedAt } : {}),
      ...(patch.finishedAt !== undefined ? { finishedAt: patch.finishedAt } : {}),
    })
    .where(eq(planSteps.id, stepId));
}

export async function findSuccessfulStepByIdempotencyKey(
  db: AelioDatabase['db'],
  idempotencyKey: string,
): Promise<PlanStepRecord | null> {
  const rows = await db
    .select()
    .from(planSteps)
    .where(and(eq(planSteps.idempotencyKey, idempotencyKey), eq(planSteps.status, 'success')))
    .limit(1);
  return rows[0] ? mapStep(rows[0]) : null;
}

/** Remove steps that can be replaced on replan. Successful steps are kept for reference resolution. */
export async function deleteReplannablePlanSteps(
  db: AelioDatabase['db'],
  planId: string,
): Promise<void> {
  const rows = await loadPlanSteps(db, planId);
  for (const step of rows) {
    if (step.status !== 'success' && step.status !== 'skipped') {
      await db.delete(planSteps).where(eq(planSteps.id, step.id));
    }
  }
}
