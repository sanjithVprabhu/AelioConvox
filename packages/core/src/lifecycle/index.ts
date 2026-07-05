import type { AelioDatabase } from '@aelio/db';
import { customers } from '@aelio/db';
import type {
  FlowDefinition,
  FunctionDefinition,
  PolicyDefinition,
  StateDefinition,
} from '@aelio/protocol';
import { eq } from 'drizzle-orm';
import { randomUUID } from 'node:crypto';

export type FlowProgressRecord = {
  currentStepIndex: number;
  completedSteps: string[];
};

export type CustomerLifecycleMetadata = {
  lifecycleState?: string;
  lifecycleStateUpdatedAt?: string;
  lifecycleStateReason?: string;
  flowProgress?: Record<string, FlowProgressRecord>;
};

export function readLifecycleMetadata(
  metadata: Record<string, unknown> | null | undefined,
): CustomerLifecycleMetadata {
  if (!metadata || typeof metadata !== 'object') {
    return {};
  }

  const flowProgressRaw = metadata.flowProgress;
  const flowProgress =
    flowProgressRaw && typeof flowProgressRaw === 'object' && !Array.isArray(flowProgressRaw)
      ? (flowProgressRaw as Record<string, FlowProgressRecord>)
      : undefined;

  return {
    lifecycleState:
      typeof metadata.lifecycleState === 'string' ? metadata.lifecycleState : undefined,
    lifecycleStateUpdatedAt:
      typeof metadata.lifecycleStateUpdatedAt === 'string'
        ? metadata.lifecycleStateUpdatedAt
        : undefined,
    lifecycleStateReason:
      typeof metadata.lifecycleStateReason === 'string' ? metadata.lifecycleStateReason : undefined,
    flowProgress,
  };
}

export async function getCustomerLifecycleMetadata(
  db: AelioDatabase['db'],
  internalCustomerId: string,
): Promise<CustomerLifecycleMetadata> {
  const rows = await db
    .select({ metadata: customers.metadata })
    .from(customers)
    .where(eq(customers.id, internalCustomerId))
    .limit(1);

  return readLifecycleMetadata(rows[0]?.metadata ?? null);
}

export async function upsertCustomerLifecycleState(
  db: AelioDatabase['db'],
  externalId: string,
  stateId: string,
  reason?: string,
): Promise<void> {
  const now = new Date();
  const rows = await db
    .select()
    .from(customers)
    .where(eq(customers.externalId, externalId))
    .limit(1);

  const existing = rows[0];
  const prior = readLifecycleMetadata(existing?.metadata ?? null);
  const metadata: Record<string, unknown> = {
    ...(existing?.metadata ?? {}),
    lifecycleState: stateId,
    lifecycleStateUpdatedAt: now.toISOString(),
    ...(reason ? { lifecycleStateReason: reason } : {}),
    // Reset guided flow progress when the lifecycle stage changes.
    flowProgress:
      prior.lifecycleState && prior.lifecycleState !== stateId ? {} : (prior.flowProgress ?? {}),
  };

  if (existing) {
    await db
      .update(customers)
      .set({ metadata, updatedAt: now })
      .where(eq(customers.id, existing.id));
    return;
  }

  await db.insert(customers).values({
    id: randomUUID(),
    externalId,
    displayName: externalId,
    createdAt: now,
    updatedAt: now,
    metadata,
  });
}

export async function upsertCustomerFlowProgress(
  db: AelioDatabase['db'],
  externalId: string,
  flowId: string,
  stepIndex: number,
  completedSteps?: string[],
): Promise<void> {
  const now = new Date();
  const rows = await db
    .select()
    .from(customers)
    .where(eq(customers.externalId, externalId))
    .limit(1);

  const existing = rows[0];
  const prior = readLifecycleMetadata(existing?.metadata ?? null);
  const flowProgress = { ...(prior.flowProgress ?? {}) };
  flowProgress[flowId] = {
    currentStepIndex: stepIndex,
    completedSteps: completedSteps ?? flowProgress[flowId]?.completedSteps ?? [],
  };

  const metadata: Record<string, unknown> = {
    ...(existing?.metadata ?? {}),
    flowProgress,
  };

  if (existing) {
    await db
      .update(customers)
      .set({ metadata, updatedAt: now })
      .where(eq(customers.id, existing.id));
    return;
  }

  await db.insert(customers).values({
    id: randomUUID(),
    externalId,
    displayName: externalId,
    createdAt: now,
    updatedAt: now,
    metadata,
  });
}

export function filterFunctionsByState(
  functions: FunctionDefinition[],
  state?: StateDefinition,
): FunctionDefinition[] {
  if (!state) {
    return functions;
  }

  const allowed = state.allowedTools?.length ? new Set(state.allowedTools) : null;
  const blocked = new Set(state.blockedTools ?? []);

  return functions.filter((fn) => {
    if (blocked.has(fn.name)) {
      return false;
    }
    if (allowed && !allowed.has(fn.name)) {
      return false;
    }
    return true;
  });
}

export function buildLifecycleSystemPrompt(input: {
  stateId?: string;
  state?: StateDefinition;
  policies: PolicyDefinition[];
  flows: FlowDefinition[];
  flowProgress?: Record<string, FlowProgressRecord>;
}): string {
  const sections: string[] = [];

  if (input.stateId && input.state) {
    sections.push(
      [
        `Customer lifecycle state: ${input.stateId}`,
        `State boundaries:\n${input.state.description.trim()}`,
        'You MUST stay within this lifecycle state. Do not pursue goals, offers, or actions that belong to later stages.',
        'If the user asks for something outside this state, explain what they can do now and what unlocks after they advance.',
        'Do NOT advance the customer to a later lifecycle stage yourself — only the SaaS backend can change their state.',
      ].join('\n'),
    );
  } else if (input.stateId) {
    sections.push(
      `Customer lifecycle state: ${input.stateId}. Stay within the scope appropriate for this stage.`,
    );
  }

  if (input.policies.length > 0) {
    const policyLines = input.policies.map((policy) => {
      const tag = policy.severity === 'hard' ? '[REQUIRED]' : '[GUIDELINE]';
      return `${tag} ${policy.id}: ${policy.description.trim()}`;
    });
    sections.push(`Active policies:\n${policyLines.join('\n')}`);
  }

  const activeFlows = input.stateId
    ? input.flows.filter((flow) => flow.state === input.stateId)
    : [];

  if (activeFlows.length > 0) {
    const flowLines = activeFlows.map((flow) => {
      const progress = input.flowProgress?.[flow.id];
      const stepIndex = Math.min(
        progress?.currentStepIndex ?? 0,
        Math.max(flow.steps.length - 1, 0),
      );
      const step = flow.steps[stepIndex];
      const completed = progress?.completedSteps?.length
        ? ` Completed steps: ${progress.completedSteps.join(', ')}.`
        : '';
      const stepLine = step
        ? `Current step (${stepIndex + 1}/${flow.steps.length}): ${step.id} — ${step.goal}${
            step.tool ? ` (tool: ${step.tool})` : ''
          }.`
        : '';
      return [
        `Flow "${flow.id}": ${flow.description.trim()}`,
        stepLine,
        completed,
        'Guide the user through this flow one step at a time. Do not skip ahead to later steps.',
      ]
        .filter(Boolean)
        .join(' ');
    });
    sections.push(`Active guided flows:\n${flowLines.join('\n')}`);
  }

  return sections.join('\n\n');
}