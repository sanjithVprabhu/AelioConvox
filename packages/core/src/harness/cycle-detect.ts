import type { GeneratedPlan } from './types.js';
import { PlanValidationError } from './types.js';

export function detectCycleInPlan(plan: GeneratedPlan): boolean {
  const graph = new Map<number, number[]>();
  for (let index = 0; index < plan.steps.length; index += 1) {
    graph.set(index, plan.steps[index]?.depends_on_step_index ?? []);
  }

  const visited = new Set<number>();
  const inStack = new Set<number>();

  function dfs(node: number): boolean {
    if (inStack.has(node)) {
      return true;
    }
    if (visited.has(node)) {
      return false;
    }
    visited.add(node);
    inStack.add(node);
    for (const dep of graph.get(node) ?? []) {
      if (dep < 0 || dep >= plan.steps.length) {
        throw new PlanValidationError(`Invalid dependency index ${dep} on step ${node}`);
      }
      if (dfs(dep)) {
        return true;
      }
    }
    inStack.delete(node);
    return false;
  }

  for (let index = 0; index < plan.steps.length; index += 1) {
    if (dfs(index)) {
      return true;
    }
  }
  return false;
}

export function assertValidPlan(plan: GeneratedPlan, stepCap: number): void {
  if (plan.steps.length === 0) {
    throw new PlanValidationError('Plan must contain at least one step');
  }
  if (plan.steps.length > stepCap) {
    throw new PlanValidationError(`Plan exceeded step cap: ${plan.steps.length} > ${stepCap}`);
  }
  for (const [index, step] of plan.steps.entries()) {
    if (!step.tool_name?.trim()) {
      throw new PlanValidationError(`Step ${index} is missing tool_name`);
    }
    for (const dep of step.depends_on_step_index ?? []) {
      if (dep >= index) {
        throw new PlanValidationError(
          `Step ${index} depends on step ${dep}, which is not earlier in the plan`,
        );
      }
    }
  }
  if (detectCycleInPlan(plan)) {
    throw new PlanValidationError('Plan contains a dependency cycle');
  }
}

export function topologicalOrder<T extends { id: string; dependsOn: string[] }>(
  steps: T[],
): T[] {
  const byId = new Map(steps.map((step) => [step.id, step]));
  const visited = new Set<string>();
  const output: T[] = [];

  function visit(step: T) {
    if (visited.has(step.id)) {
      return;
    }
    for (const depId of step.dependsOn) {
      const dep = byId.get(depId);
      if (dep) {
        visit(dep);
      }
    }
    visited.add(step.id);
    output.push(step);
  }

  for (const step of [...steps].sort((a, b) => a.id.localeCompare(b.id))) {
    visit(step);
  }
  return output;
}
