import type { FunctionDefinition } from '@aelio/protocol';
import { coerceArgs, findMissingRequiredArgs } from '../runtime/tool-schema.js';
import type { ArgSource, LedgerEntry, ResolvedInstruction, ResolvedPlan, SuspendedPlanPayload } from './schema.js';
import { newExecutorState, type ExecutorState } from './executor.js';

export type RehydrateResult =
  | { ok: true; plan: ResolvedPlan; state: ExecutorState; pendingInstructionId: string }
  | { ok: false; reason: 'stale_registry' | 'tool_gone' | 'answer_rejected' };

/**
 * Rebuild an executable plan from a persisted suspension, applying the user's
 * answer to the parked instruction. Returns a seeded executor state (completed
 * steps + outputs replayed from the ledger) so resumption never re-invokes a
 * finished step — the idempotency guarantee across restarts.
 *
 * Guards: a registry change since suspension (hash mismatch) or a vanished tool
 * discards the plan (caller re-plans from scratch); an answer that fails the
 * pending field's schema is rejected so the caller can re-ask.
 */
export function rehydrateSuspension(
  payload: SuspendedPlanPayload,
  answer: string,
  registry: FunctionDefinition[],
  currentRegistryHash: string | null,
): RehydrateResult {
  if (currentRegistryHash && payload.registryHash && currentRegistryHash !== payload.registryHash) {
    return { ok: false, reason: 'stale_registry' };
  }

  const byName = new Map(registry.map((fn) => [fn.name, fn]));

  // Rebind tools; any missing tool invalidates the plan.
  const instructions: ResolvedInstruction[] = [];
  for (const stored of payload.instructions) {
    const tool = byName.get(stored.toolName);
    if (!tool) {
      return { ok: false, reason: 'tool_gone' };
    }
    instructions.push({
      id: stored.id,
      capability: stored.capability,
      tool,
      argSources: stored.argSources as Record<string, ArgSource>,
      needs: stored.needs,
      effect: stored.effect,
      produces: stored.produces,
    });
  }

  const pending = instructions.find((i) => i.id === payload.pendingInstructionId);
  if (!pending) {
    return { ok: false, reason: 'tool_gone' };
  }

  // Validate the answer against the asked field's schema.
  const field = payload.ask.field;
  if (field) {
    const coerced = coerceArgs(pending.tool.params, { [field]: answer });
    const stillMissing = findMissingRequiredArgs(pending.tool.params, {
      ...collectKnownArgs(pending, payload.ledger),
      [field]: coerced.args[field],
    });
    if (coerced.errors.length > 0 || stillMissing.includes(field)) {
      return { ok: false, reason: 'answer_rejected' };
    }
    // Pin the answered value as a literal so the executor uses it directly.
    pending.argSources[field] = { kind: 'literal', value: coerced.args[field] };
  }

  // Seed executor state from the persisted ledger.
  const state = newExecutorState();
  state.ledger = payload.ledger as LedgerEntry[];
  for (const entry of payload.ledger) {
    state.completed.add(entry.instructionId);
    if (entry.status === 'success') {
      state.outputs.set(entry.instructionId, entry.result);
    }
    state.executedToolNames.push(entry.toolName);
    state.toolCallsExecuted += 1;
  }

  return {
    ok: true,
    plan: { goal: payload.goal, instructions },
    state,
    pendingInstructionId: payload.pendingInstructionId,
  };
}

/** Args already known for an instruction from literals + ledger outputs. */
function collectKnownArgs(
  instruction: ResolvedInstruction,
  ledger: SuspendedPlanPayload['ledger'],
): Record<string, unknown> {
  const known: Record<string, unknown> = {};
  for (const [param, source] of Object.entries(instruction.argSources) as [string, ArgSource][]) {
    if (source.kind === 'literal') {
      known[param] = source.value;
    } else if (source.kind === 'output') {
      const entry = ledger.find((l) => l.instructionId === source.instructionId);
      if (entry?.status === 'success') {
        known[param] = entry.result;
      }
    }
  }
  return known;
}

/** Serialize a live resolved plan + executor state into a persistable payload. */
export function toSuspensionPayload(input: {
  goal: string;
  userMessage: string;
  registryHash: string;
  plan: ResolvedPlan;
  state: ExecutorState;
  pendingInstructionId: string;
  ask: { field?: string; toolName?: string; question: string };
  recoilCount: number;
}): SuspendedPlanPayload {
  return {
    version: 1,
    goal: input.goal,
    userMessage: input.userMessage,
    registryHash: input.registryHash,
    instructions: input.plan.instructions.map((i) => ({
      id: i.id,
      capability: i.capability,
      toolName: i.tool.name,
      argSources: i.argSources as Record<string, unknown>,
      needs: i.needs,
      effect: i.effect,
      produces: i.produces,
    })),
    ledger: input.state.ledger,
    pendingInstructionId: input.pendingInstructionId,
    ask: input.ask,
    recoilCount: input.recoilCount,
  };
}
