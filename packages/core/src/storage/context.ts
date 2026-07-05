import type { FlowDefinition, PolicyDefinition } from '@aelio/protocol';
import type { IntentFrame, IntentStack } from '../intent/stack.js';
import type {
  CustomerLifecycleMetadata,
  FlowProgressRecord,
} from '../lifecycle/index.js';

export type ConversationTurnContext = {
  customerExternalId: string;
  channelAddress: string;
  lifecycleState?: string;
  lifecycleStateReason?: string;
  intentLabel?: string;
  intentSummary?: string;
  intentStackJson?: string;
  flowId?: string;
  flowStepId?: string;
  flowStepIndex?: number;
  flowStepGoal?: string;
  activePoliciesJson?: string;
  toolsExecutedJson?: string;
  pendingConfirmation?: boolean;
};

export type ActiveFlowContext = {
  flowId?: string;
  flowStepId?: string;
  flowStepIndex?: number;
  flowStepGoal?: string;
};

export function resolveActiveFlowContext(
  stateId: string | undefined,
  flows: FlowDefinition[],
  flowProgress?: Record<string, FlowProgressRecord>,
): ActiveFlowContext {
  if (!stateId) {
    return {};
  }

  const activeFlows = flows.filter((flow) => flow.state === stateId);
  const flow = activeFlows[0];
  if (!flow || flow.steps.length === 0) {
    return {};
  }

  const progress = flowProgress?.[flow.id];
  const stepIndex = Math.min(
    progress?.currentStepIndex ?? 0,
    Math.max(flow.steps.length - 1, 0),
  );
  const step = flow.steps[stepIndex];

  return {
    flowId: flow.id,
    flowStepId: step?.id,
    flowStepIndex: stepIndex,
    flowStepGoal: step?.goal,
  };
}

export function serializePolicies(policies: PolicyDefinition[]): string {
  return JSON.stringify(
    policies.map((policy) => ({
      id: policy.id,
      severity: policy.severity,
    })),
  );
}

export function serializeIntentStack(stack: IntentStack): string {
  return JSON.stringify(
    stack.map((frame: IntentFrame) => ({
      id: frame.id,
      label: frame.label,
      summary: frame.summary,
      startedAt: frame.startedAt,
      lastActiveAt: frame.lastActiveAt,
      expiresAt: frame.expiresAt,
    })),
  );
}

export function buildConversationContext(input: {
  customerExternalId: string;
  channelAddress: string;
  lifecycle: CustomerLifecycleMetadata;
  intentStack: IntentStack;
  flows: FlowDefinition[];
  policies: PolicyDefinition[];
  toolsExecuted?: string[];
  pendingConfirmation?: boolean;
}): ConversationTurnContext {
  const top = input.intentStack[0];
  const flow = resolveActiveFlowContext(
    input.lifecycle.lifecycleState,
    input.flows,
    input.lifecycle.flowProgress,
  );

  return {
    customerExternalId: input.customerExternalId,
    channelAddress: input.channelAddress,
    lifecycleState: input.lifecycle.lifecycleState,
    lifecycleStateReason: input.lifecycle.lifecycleStateReason,
    intentLabel: top?.label,
    intentSummary: top?.summary,
    intentStackJson: input.intentStack.length > 0 ? serializeIntentStack(input.intentStack) : undefined,
    flowId: flow.flowId,
    flowStepId: flow.flowStepId,
    flowStepIndex: flow.flowStepIndex,
    flowStepGoal: flow.flowStepGoal,
    activePoliciesJson:
      input.policies.length > 0 ? serializePolicies(input.policies) : undefined,
    toolsExecutedJson:
      input.toolsExecuted && input.toolsExecuted.length > 0
        ? JSON.stringify(input.toolsExecuted)
        : undefined,
    pendingConfirmation: input.pendingConfirmation,
  };
}