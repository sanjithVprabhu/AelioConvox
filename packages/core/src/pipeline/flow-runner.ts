import type { AelioDatabase } from '@aelio/db';
import type { FlowDefinition, FlowStepDefinition } from '@aelio/protocol';
import { upsertCustomerFlowProgress } from '../lifecycle/index.js';
import { hasAttribute, upsertCustomerAttribute, upsertPipelineState } from './state-store.js';

export type FlowProgress = {
  flowId: string;
  stepIndex: number;
  completedSteps: string[];
};

export function resolveFlowStep(flow: FlowDefinition, stepIndex: number): FlowStepDefinition | null {
  if (flow.steps.length === 0) {
    return null;
  }
  const index = Math.min(Math.max(stepIndex, 0), flow.steps.length - 1);
  return flow.steps[index] ?? null;
}

export function stepType(step: FlowStepDefinition): 'tool' | 'attribute' | 'content' {
  if (step.type) {
    return step.type;
  }
  if (step.tool) {
    return 'tool';
  }
  if (step.attribute) {
    return 'attribute';
  }
  return 'content';
}

export async function findFirstIncompleteStepIndex(
  db: AelioDatabase['db'],
  internalCustomerId: string,
  flow: FlowDefinition,
  startIndex: number,
): Promise<number> {
  for (let index = startIndex; index < flow.steps.length; index += 1) {
    const step = flow.steps[index];
    if (!step) {
      continue;
    }
    const skipKey = step.skip_if_present ?? step.attribute;
    if (skipKey && (await hasAttribute(db, internalCustomerId, skipKey))) {
      continue;
    }
    return index;
  }
  return flow.steps.length;
}

export async function advanceFlowProgress(
  db: AelioDatabase['db'],
  externalId: string,
  internalCustomerId: string,
  flow: FlowDefinition,
  currentIndex: number,
  completedStepId: string,
  nextGlobalStage?: string,
): Promise<FlowProgress> {
  const prior = flow.steps.slice(0, currentIndex).map((step) => step.id);
  const uniqueCompleted = [...new Set([...prior, completedStepId])];
  const nextIndex = await findFirstIncompleteStepIndex(db, internalCustomerId, flow, currentIndex + 1);

  if (nextIndex >= flow.steps.length) {
    await upsertCustomerFlowProgress(db, externalId, flow.id, flow.steps.length, uniqueCompleted);
    if (nextGlobalStage) {
      await upsertPipelineState(db, internalCustomerId, nextGlobalStage);
    }
    return { flowId: flow.id, stepIndex: flow.steps.length, completedSteps: uniqueCompleted };
  }

  await upsertCustomerFlowProgress(db, externalId, flow.id, nextIndex, uniqueCompleted);
  return { flowId: flow.id, stepIndex: nextIndex, completedSteps: uniqueCompleted };
}

export async function collectAttributeFromMessage(
  db: AelioDatabase['db'],
  internalCustomerId: string,
  attributeId: string,
  message: string,
  enumValues?: Array<string | number>,
): Promise<unknown> {
  const trimmed = message.trim();
  if (enumValues && enumValues.length > 0) {
    const match = matchEnumValue(trimmed, enumValues);
    if (match !== undefined) {
      await upsertCustomerAttribute(db, internalCustomerId, attributeId, match);
      return match;
    }
  }

  await upsertCustomerAttribute(db, internalCustomerId, attributeId, trimmed);
  return trimmed;
}

const AFFIRMATIVES = new Set([
  'yes',
  'yeah',
  'yep',
  'sure',
  'ok',
  'okay',
  'y',
  'lets go',
  "let's go",
]);

/** Map free-text replies to enum options (e.g. "yes" → "Let's go"). */
export function matchEnumValue(
  message: string,
  enumValues: Array<string | number>,
): string | number | undefined {
  const trimmed = message.trim();
  const lower = trimmed.toLowerCase();

  const exact = enumValues.find((value) => String(value).toLowerCase() === lower);
  if (exact !== undefined) {
    return exact;
  }

  if (AFFIRMATIVES.has(lower)) {
    const proceed = enumValues.find((value) =>
      /let'?s go|continue|proceed|yes|start/i.test(String(value)),
    );
    if (proceed !== undefined) {
      return proceed;
    }
    return enumValues[0];
  }

  return undefined;
}

export function stepMatchesTool(step: FlowStepDefinition, toolName: string): boolean {
  return stepType(step) === 'tool' && step.tool === toolName;
}
