import type { FunctionDefinition } from '@aelio/protocol';
import { requiredParamNames } from '../runtime/tool-schema.js';
import type { ArgSource, PlanInstruction, ResolvedInstruction, ResolvedPlan } from './schema.js';

export type BoundInstruction = {
  instruction: PlanInstruction;
  tool: FunctionDefinition;
};

export type ResolveResult =
  | { ok: true; plan: ResolvedPlan }
  | { ok: false; reason: string };

/** Normalize a field name for producer/consumer matching (cart_id ~ cartId ~ CartID). */
function normalizeField(name: string): string {
  return name.toLowerCase().replace(/[^a-z0-9]/g, '');
}

/**
 * The resolver — the spine-bug fix. Dependency structure is derived HERE, from
 * the bound tool schemas, never from the planner's prose. For each bound
 * instruction it sources every required param:
 *   (a) produced by an earlier instruction  → a dependency edge (a "rock")
 *   (b) supplied as a literal hint            → a literal
 *   (c) neither                               → missing (a recoil candidate)
 * Rock/river then falls out of the edges: the executor runs every instruction
 * whose needs are satisfied in parallel. Effect type (read vs write) also comes
 * from the schema, so write-sequencing is a schema property, not a guess.
 */
export function resolvePlan(
  goal: string,
  nudge: string | undefined,
  bound: BoundInstruction[],
): ResolveResult {
  const ids = new Set(bound.map((b) => b.instruction.id));
  if (ids.size !== bound.length) {
    return { ok: false, reason: 'plan has duplicate instruction ids' };
  }

  // Map each produced field → the earliest instruction id that yields it.
  const producerOf = new Map<string, string>();
  for (const { instruction } of bound) {
    for (const field of instruction.produces ?? []) {
      const key = normalizeField(field);
      if (!producerOf.has(key)) {
        producerOf.set(key, instruction.id);
      }
    }
  }

  const orderIndex = new Map(bound.map((b, index) => [b.instruction.id, index]));
  const resolved: ResolvedInstruction[] = [];

  for (const { instruction, tool } of bound) {
    const required = requiredParamNames(tool.params);
    const hints = (instruction.args_hint ?? {}) as Record<string, unknown>;
    const argSources: Record<string, ArgSource> = {};
    const needs = new Set<string>();

    // Optional hinted params ride along as literals so coerceArgs sees them.
    for (const [name, value] of Object.entries(hints)) {
      if (!required.includes(name)) {
        argSources[name] = { kind: 'literal', value };
      }
    }

    for (const param of required) {
      if (param in hints && hints[param] !== undefined && hints[param] !== null) {
        argSources[param] = { kind: 'literal', value: hints[param] };
        continue;
      }
      const producer = producerOf.get(normalizeField(param));
      if (producer && producer !== instruction.id) {
        // Only an EARLIER instruction can be a producer (no forward/self edges).
        const producerIdx = orderIndex.get(producer) ?? Number.MAX_SAFE_INTEGER;
        const selfIdx = orderIndex.get(instruction.id) ?? 0;
        if (producerIdx < selfIdx) {
          argSources[param] = { kind: 'output', instructionId: producer, field: param };
          needs.add(producer);
          continue;
        }
      }
      argSources[param] = { kind: 'missing' };
    }

    resolved.push({
      id: instruction.id,
      capability: instruction.capability,
      tool,
      argSources,
      needs: [...needs],
      effect: tool.safety === 'read' ? 'read' : 'write',
      produces: instruction.produces ?? [],
    });
  }

  const cycle = detectCycle(resolved);
  if (cycle) {
    return { ok: false, reason: `plan has a dependency cycle: ${cycle.join(' → ')}` };
  }

  return { ok: true, plan: { goal, ...(nudge ? { nudge } : {}), instructions: resolved } };
}

/** Kahn-style cycle detection over the derived `needs` edges. */
function detectCycle(instructions: ResolvedInstruction[]): string[] | null {
  const byId = new Map(instructions.map((i) => [i.id, i]));
  const state = new Map<string, 'visiting' | 'done'>();
  const path: string[] = [];

  const visit = (id: string): string[] | null => {
    const status = state.get(id);
    if (status === 'done') return null;
    if (status === 'visiting') {
      const start = path.indexOf(id);
      return [...path.slice(start), id];
    }
    state.set(id, 'visiting');
    path.push(id);
    for (const dep of byId.get(id)?.needs ?? []) {
      if (byId.has(dep)) {
        const found = visit(dep);
        if (found) return found;
      }
    }
    path.pop();
    state.set(id, 'done');
    return null;
  };

  for (const instruction of instructions) {
    const found = visit(instruction.id);
    if (found) return found;
  }
  return null;
}

/**
 * Topological wavefront generator: yields successive "ready" sets — each the
 * batch of instructions whose dependencies are all satisfied. Rivers (reads)
 * within a wave run in parallel; writes are sequenced by the executor even
 * within a wave. Consumed as the executor completes each wave.
 */
export function nextWave(
  instructions: ResolvedInstruction[],
  completed: Set<string>,
): ResolvedInstruction[] {
  return instructions.filter(
    (instruction) =>
      !completed.has(instruction.id) &&
      instruction.needs.every((dep) => completed.has(dep)),
  );
}
