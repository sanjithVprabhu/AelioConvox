import type {
  FlowDefinition,
  FunctionDefinition,
  PolicyDefinition,
  StateDefinition,
} from '@aelio/protocol';
import type { ConvoxCustomerStore } from '../storage/customers.js';

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

function requireCustomerStore(customerStore: ConvoxCustomerStore | undefined): ConvoxCustomerStore {
  if (!customerStore) {
    throw new Error('Sunjet customerStore is required');
  }
  return customerStore;
}

export async function getCustomerLifecycleMetadata(
  internalCustomerId: string,
  customerStore: ConvoxCustomerStore,
): Promise<CustomerLifecycleMetadata> {
  const store = requireCustomerStore(customerStore);
  const customer = await store.getById(internalCustomerId);
  return readLifecycleMetadata(customer?.metadata ?? null);
}

/** Reserved lifecycle keys on `customers.metadata` — not tenant profile fields. */
const RESERVED_METADATA_KEYS = new Set([
  'lifecycleState',
  'lifecycleStateUpdatedAt',
  'lifecycleStateReason',
  'flowProgress',
]);

/**
 * The set of customer-profile field names currently present (truthy) on the
 * customer, used to evaluate state/transition `requires_fields` guards. Sourced
 * from `customers.metadata` minus the reserved lifecycle keys — so a tenant that
 * stores e.g. `{ address: "...", payment_method: "card" }` gets those recognized
 * by guards. Empty until a tenant populates them (the guard then holds).
 */
export async function getCustomerPresentFields(
  internalCustomerId: string,
  customerStore: ConvoxCustomerStore,
): Promise<Set<string>> {
  const store = requireCustomerStore(customerStore);
  const customer = await store.getById(internalCustomerId);
  const metadata = customer?.metadata ?? null;
  const present = new Set<string>();
  if (metadata && typeof metadata === 'object') {
    for (const [key, value] of Object.entries(metadata)) {
      if (
        !RESERVED_METADATA_KEYS.has(key) &&
        value !== null &&
        value !== undefined &&
        value !== false &&
        value !== ''
      ) {
        present.add(key);
      }
    }
  }
  return present;
}

export async function upsertCustomerLifecycleState(
  externalId: string,
  stateId: string,
  reason: string | undefined,
  customerStore: ConvoxCustomerStore,
): Promise<void> {
  const store = requireCustomerStore(customerStore);
  const existing = await store.getByExternalId(externalId);
  const prior = readLifecycleMetadata(existing?.metadata ?? null);
  const metadata: Record<string, unknown> = {
    ...(existing?.metadata ?? {}),
    lifecycleState: stateId,
    lifecycleStateUpdatedAt: new Date().toISOString(),
    ...(reason ? { lifecycleStateReason: reason } : {}),
    // Reset guided flow progress when the lifecycle stage changes.
    flowProgress:
      prior.lifecycleState && prior.lifecycleState !== stateId ? {} : (prior.flowProgress ?? {}),
  };
  if (existing) {
    await store.updateMetadata(existing.id, metadata);
    return;
  }
  const customerId = await store.ensureCustomer(externalId, 'web', externalId);
  await store.updateMetadata(customerId, metadata);
}

export async function upsertCustomerFlowProgress(
  externalId: string,
  flowId: string,
  stepIndex: number,
  completedSteps: string[] | undefined,
  customerStore: ConvoxCustomerStore,
): Promise<void> {
  const store = requireCustomerStore(customerStore);
  const existing = await store.getByExternalId(externalId);
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
    await store.updateMetadata(existing.id, metadata);
    return;
  }
  const customerId = await store.ensureCustomer(externalId, 'web', externalId);
  await store.updateMetadata(customerId, metadata);
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
