import type { FunctionDefinition } from '@aelio/protocol';
import type { LighthouseService } from '../lighthouse/index.js';
import type { HarnessBindingConfig, PlanInstruction } from './schema.js';
import type { BoundInstruction } from './resolver.js';

export type BindResult =
  | { ok: true; bound: BoundInstruction[] }
  | { ok: false; unbound: PlanInstruction[]; bound: BoundInstruction[] };

/**
 * Bind capability-level instructions to concrete tools. Two deterministic
 * sources, in order of confidence:
 *   1. The planner's own `tool` suggestion (it saw the tool card) — trusted if
 *      the name resolves in the live registry.
 *   2. Semantic search over the Lighthouse mirror (with prerequisite-graph
 *      expansion) — bound only when the top hit clears `scoreMin` AND beats the
 *      runner-up by `ambiguityGap` (otherwise it's genuinely ambiguous and
 *      left for the batched disambiguation call — Phase 6).
 * Argument filling is NOT done here; that's the resolver + executor's job.
 */
export async function bindInstructions(
  instructions: PlanInstruction[],
  registry: FunctionDefinition[],
  lighthouse: LighthouseService | undefined,
  binding: HarnessBindingConfig,
): Promise<BindResult> {
  const byName = new Map(registry.map((fn) => [fn.name, fn]));
  const bound: BoundInstruction[] = [];
  const unbound: PlanInstruction[] = [];

  for (const instruction of instructions) {
    // 1. Planner suggestion. The planner occasionally emits a namespaced form
    // (e.g. "functions.start_shopping_session") even though the registry has
    // no such prefix — likely bleed-through from other function-calling
    // conventions seen in training data. Retry against the bare name after
    // the last "." before falling through to semantic search; an unambiguous
    // exact-name suggestion shouldn't have to win a similarity contest just
    // because of a stray namespace prefix.
    const suggested = instruction.tool
      ? (byName.get(instruction.tool) ?? byName.get(instruction.tool.replace(/^.*\./, '')))
      : undefined;
    if (suggested) {
      bound.push({ instruction, tool: suggested });
      continue;
    }

    // 2. Semantic search.
    if (lighthouse) {
      const hits = await lighthouse.searchTools(instruction.capability, 3);
      const top = hits[0];
      const runner = hits[1];
      if (
        top &&
        top.score >= binding.scoreMin &&
        (!runner || top.score - runner.score >= binding.ambiguityGap)
      ) {
        const fn = byName.get(top.fn.name);
        if (fn) {
          bound.push({ instruction, tool: fn });
          continue;
        }
      }
    }

    unbound.push(instruction);
  }

  if (unbound.length > 0) {
    return { ok: false, unbound, bound };
  }
  return { ok: true, bound };
}
