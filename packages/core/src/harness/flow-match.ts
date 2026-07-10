import type { FlowDefinition } from '@aelio/protocol';
import { cosineSimilarity, embed } from '../analyst/embeddings.js';
import type { GeneratedPlan, PlannedStep } from './types.js';

export type FlowMatchResult = {
  flow: FlowDefinition;
  score: number;
};

export async function matchFlow(
  intent: string,
  flows: FlowDefinition[],
  threshold: number,
): Promise<FlowMatchResult | null> {
  if (flows.length === 0) {
    return null;
  }

  const intentEmbedding = await embed(intent);
  let best: FlowMatchResult | null = null;

  for (const flow of flows) {
    const descriptor = [flow.id, flow.description, ...flow.steps.map((step) => step.goal)].join(
      ' — ',
    );
    const score = cosineSimilarity(intentEmbedding, await embed(descriptor));
    if (!best || score > best.score) {
      best = { flow, score };
    }
  }

  if (best && best.score >= threshold) {
    return best;
  }
  return null;
}

export function flowToPlan(flow: FlowDefinition): GeneratedPlan {
  const steps: PlannedStep[] = [];
  for (const flowStep of flow.steps) {
    if (!flowStep.tool) {
      continue;
    }
    const dependsOn: number[] = [];
    if (steps.length > 0) {
      dependsOn.push(steps.length - 1);
    }
    steps.push({
      tool_name: flowStep.tool,
      input: {},
      depends_on_step_index: dependsOn,
    });
  }
  return { steps };
}
