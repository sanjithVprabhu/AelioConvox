import type { PlanStepRecord } from './types.js';

const REF_PATTERN = /"\$step_(\d+)\.output\.(\w+)"/g;

export function resolveReferences(
  template: Record<string, unknown>,
  steps: PlanStepRecord[],
): Record<string, unknown> {
  const json = JSON.stringify(template);
  const resolved = json.replace(REF_PATTERN, (_, stepIndexRaw: string, field: string) => {
    const stepIndex = Number.parseInt(stepIndexRaw, 10);
    const sourceStep = steps.find((step) => step.stepOrder === stepIndex);
    if (!sourceStep || sourceStep.status !== 'success') {
      throw new Error(`Unresolved dependency: step ${stepIndexRaw} not yet successful`);
    }
    const output = sourceStep.output;
    if (!output || typeof output !== 'object' || !(field in (output as Record<string, unknown>))) {
      throw new Error(`Unresolved dependency: step ${stepIndexRaw}.output.${field} missing`);
    }
    return JSON.stringify((output as Record<string, unknown>)[field]);
  });
  return JSON.parse(resolved) as Record<string, unknown>;
}
