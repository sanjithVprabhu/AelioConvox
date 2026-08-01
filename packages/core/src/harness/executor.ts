import { createHash } from 'node:crypto';
import type { InvocationContext, StateDefinition } from '@aelio/protocol';
import { logFunctionCall } from '../audit/function-calls.js';
import { buildConfirmationPrompt, type PendingConfirmation } from '../safety/confirmations.js';
import type { SafetyConfig } from '../safety/policy.js';
import type { SdkBridge } from '../sdk-bridge/types.js';
import { coerceArgs } from '../runtime/tool-schema.js';
import { evaluateGate, type GateContext } from './gates.js';
import { loadLedgerForTurn, persistLedgerEntry, type LedgerAelioDbConfig } from './ledger.js';
import { nextWave } from './resolver.js';
import type { BudgetMeter } from './budgets.js';
import type { ConvoxFunctionCallStore } from '../storage/audit.js';
import type {
  ArgSource,
  GateVerdict,
  LedgerEntry,
  ResolvedInstruction,
  ResolvedPlan,
} from './schema.js';

/** Recursively sort object keys so hashing is order-insensitive. */
function canonicalize(value: unknown): unknown {
  if (value instanceof Date) {
    return value.toISOString();
  }
  if (typeof Buffer !== 'undefined' && Buffer.isBuffer(value)) {
    return value.toString('base64');
  }
  if (Array.isArray(value)) {
    return value.map(canonicalize);
  }
  if (value && typeof value === 'object' && Object.getPrototypeOf(value) === Object.prototype) {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
        .map(([k, v]) => [k, canonicalize(v)]),
    );
  }
  return value;
}

/**
 * Stable content hash of a call's arguments — the idempotency key. Canonicalizes
 * key order first: `{a,b}` and `{b,a}` are the same call, so a retry with the
 * args in a different order still dedups against the ledger (never double-runs).
 */
export function hashArgs(args: Record<string, unknown>): string {
  return createHash('sha256').update(JSON.stringify(canonicalize(args ?? {}))).digest('hex').slice(0, 16);
}

/**
 * The outcome of executing (part of) a plan. The executor stops early and
 * reports WHY so the harness can synthesize, suspend, or replan:
 *  - 'complete'  → every instruction reached a terminal state; synthesize.
 *  - 'suspend'   → a gate returned needs_info/needs_approval; park the plan.
 *  - 'blocked'   → a fatal gate (deny) or budget exhaustion; synthesize the reason.
 *  - 'replan'    → a rock resolved with a surprising result; re-plan the tail.
 */
export type ExecOutcome =
  | { kind: 'complete' }
  | {
      kind: 'suspend';
      reason: 'awaiting_info' | 'awaiting_confirmation';
      instruction: ResolvedInstruction;
      verdict: Extract<GateVerdict, { verdict: 'needs_info' | 'needs_approval' }>;
      pendingConfirmation?: PendingConfirmation;
      question: string;
    }
  // `fatal` = a policy/state denial whose reason IS the user-facing message
  // (must be surfaced even when earlier steps produced a ledger). Non-fatal =
  // a budget stop, where a graceful synthesis over the ledger reads better.
  // `cause` disambiguates other non-fatal stops (dependency/stall) that must
  // also surface their reason instead of synthesizing over a partial ledger.
  | { kind: 'blocked'; reason: string; fatal: boolean; cause?: 'dependency' | 'budget' | 'stall' }
  | { kind: 'replan'; afterInstruction: string; surprise: string };

export type ExecutorState = {
  ledger: LedgerEntry[];
  completed: Set<string>;
  /** instructionId → the tool's returned data (for output-sourced args). */
  outputs: Map<string, unknown>;
  executedToolNames: string[];
  toolCallsExecuted: number;
};

export function newExecutorState(existingLedger: LedgerEntry[] = []): ExecutorState {
  const outputs = new Map<string, unknown>();
  const completed = new Set<string>();
  for (const entry of existingLedger) {
    if (entry.status === 'success') {
      completed.add(entry.instructionId);
      outputs.set(entry.instructionId, entry.result);
    }
  }
  return {
    ledger: [...existingLedger],
    completed,
    outputs,
    executedToolNames: [],
    toolCallsExecuted: 0,
  };
}

/** Hydrate executor state from the AelioDb ledger for a turn (HAR-005). */
export async function hydrateExecutorState(
  sessionId: string,
  turnId: string,
  ledgerAelioDb: LedgerAelioDbConfig,
): Promise<ExecutorState> {
  const existing = await loadLedgerForTurn(sessionId, turnId, ledgerAelioDb);
  return newExecutorState(existing);
}

export type ExecutorDeps = {
  sdk: SdkBridge;
  context: InvocationContext;
  safety: SafetyConfig;
  state?: StateDefinition;
  presentFields?: Set<string>;
  budgets: BudgetMeter;
  internalCustomerId?: string;
  turnId?: string;
  functionCallStore: ConvoxFunctionCallStore;
  ledgerAelioDb: LedgerAelioDbConfig;
  /**
   * Instruction ids the user has already confirmed (a resumed
   * awaiting_confirmation plan). Their needs_approval gate is treated as
   * granted so the write executes and the rest of the plan continues.
   */
  approvedInstructions?: Set<string>;
  /** Called after each rock wave with the just-completed instructions' results. */
  onWaveComplete?: (completed: ResolvedInstruction[], state: ExecutorState) => void;
  /** Applied after a successful invoke — used for declarative state transitions. */
  onToolSuccess?: (toolName: string, args: Record<string, unknown>, result: unknown) => Promise<void>;
  trace?: (kind: 'wave' | 'gate' | 'repair', payload: unknown) => void;
};

/** Read a producer output by field name (case/underscore-insensitive), if present. */
function extractField(value: unknown, field: string): unknown {
  if (value && typeof value === 'object' && !Array.isArray(value)) {
    const record = value as Record<string, unknown>;
    if (field in record) return record[field];
    const norm = field.toLowerCase().replace(/[^a-z0-9]/g, '');
    for (const [key, entry] of Object.entries(record)) {
      if (key.toLowerCase().replace(/[^a-z0-9]/g, '') === norm) return entry;
    }
  }
  // A scalar output (e.g. an id string) satisfies a single-field consumer.
  return value;
}

/** Assemble concrete args for an instruction from its resolved sources. */
function assembleArgs(
  instruction: ResolvedInstruction,
  outputs: Map<string, unknown>,
): { args: Record<string, unknown>; missing: string[] } {
  const args: Record<string, unknown> = {};
  const missing: string[] = [];
  for (const [param, source] of Object.entries(instruction.argSources) as [string, ArgSource][]) {
    if (source.kind === 'literal') {
      args[param] = source.value;
    } else if (source.kind === 'output') {
      const producerOutput = outputs.get(source.instructionId);
      const value = extractField(producerOutput, source.field);
      if (value === undefined || value === null) {
        missing.push(param);
      } else {
        args[param] = value;
      }
    } else {
      missing.push(param);
    }
  }
  return { args, missing };
}

/**
 * Execute a resolved plan as a topological wavefront. Reads in a wave run in
 * parallel; writes run alone, in id order, even within a wave. After each wave
 * that contained a dependency barrier (a rock — some later instruction needed
 * its output), the caller's repair-check may request a replan.
 */
export async function executePlan(
  plan: ResolvedPlan,
  state: ExecutorState,
  deps: ExecutorDeps,
): Promise<ExecOutcome> {
  const gateCtx: GateContext = {
    safety: deps.safety,
    ...(deps.state ? { state: deps.state } : {}),
    ...(deps.presentFields ? { presentFields: deps.presentFields } : {}),
  };

  while (state.completed.size < plan.instructions.length) {
    const wave = nextWave(plan.instructions, state.completed);
    if (wave.length === 0) {
      // Nothing runnable but not everything done → unsatisfiable deps (defensive;
      // the resolver's cycle check should already have caught structural cases).
      return {
        kind: 'blocked',
        reason: 'plan stalled: unresolved dependencies',
        fatal: false,
        cause: 'stall',
      };
    }

    const reads = wave.filter((i) => i.effect === 'read');
    const writes = wave.filter((i) => i.effect === 'write');

    // Rivers: reads in parallel.
    const readResults = await Promise.all(
      reads.map((instruction) => runInstruction(instruction, state, deps, gateCtx)),
    );
    for (const outcome of readResults) {
      if (outcome && outcome.kind !== 'complete') {
        return outcome;
      }
    }

    // Writes: sequential, one at a time (side effects never race).
    for (const instruction of writes) {
      const outcome = await runInstruction(instruction, state, deps, gateCtx);
      if (outcome && outcome.kind !== 'complete') {
        return outcome;
      }
    }

    // Halt if a tool failed and a not-yet-run step depends on its output —
    // continuing would feed a downstream step undefined inputs (or make the
    // synthesis hallucinate success). Failures with no dependents are left in
    // the ledger and the plan continues best-effort; synthesis reports them.
    for (const instruction of wave) {
      const entry = state.ledger.find((l) => l.instructionId === instruction.id);
      if (entry?.status !== 'error') {
        continue;
      }
      const hasPendingDependent = plan.instructions.some(
        (other) => !state.completed.has(other.id) && other.needs.includes(instruction.id),
      );
      if (hasPendingDependent) {
        deps.trace?.('gate', { failedProducer: instruction.id, halted: true });
        return {
          kind: 'blocked',
          reason: `I couldn't complete "${instruction.capability}", so I stopped before the steps that depend on it.`,
          fatal: false,
          cause: 'dependency',
        };
      }
    }

    const justCompleted = wave.filter((i) => state.completed.has(i.id));
    deps.onWaveComplete?.(justCompleted, state);
    deps.trace?.('wave', {
      ran: justCompleted.map((i) => i.id),
      remaining: plan.instructions.length - state.completed.size,
    });
  }

  return { kind: 'complete' };
}

/**
 * Run a single instruction: assemble args → gate → (idempotent) invoke → ledger.
 * Returns a non-'complete' outcome to halt the whole plan (suspend/block), or a
 * 'complete' marker (the instruction finished; keep going).
 */
async function runInstruction(
  instruction: ResolvedInstruction,
  state: ExecutorState,
  deps: ExecutorDeps,
  gateCtx: GateContext,
): Promise<ExecOutcome> {
  const { args, missing } = assembleArgs(instruction, state.outputs);
  const coercion = coerceArgs(instruction.tool.params, args);
  const finalArgs = coercion.args;

  // Gate on concrete args.
  const verdict = evaluateGate(instruction.tool, finalArgs, gateCtx);
  deps.trace?.('gate', { instruction: instruction.id, verdict: verdict.verdict });

  if (verdict.verdict === 'deny_fatal') {
    if (deps.internalCustomerId) {
      await logFunctionCall(
        {
          sessionId: deps.context.sessionId,
          customerId: deps.internalCustomerId,
          functionName: instruction.tool.name,
          args: finalArgs,
          status: 'blocked',
          safetyLevel: instruction.tool.safety,
          errorMessage: verdict.reason,
        },
        deps.functionCallStore,
      );
    }
    return { kind: 'blocked', reason: verdict.reason, fatal: true };
  }

  if (verdict.verdict === 'needs_info') {
    const missingFields = missing.length > 0 ? missing : verdict.missing;
    return {
      kind: 'suspend',
      reason: 'awaiting_info',
      instruction,
      verdict,
      question: `To continue I still need: ${missingFields.join(', ')}. Could you provide ${
        missingFields.length === 1 ? 'it' : 'them'
      }?`,
    };
  }

  // needs_approval → suspend for confirmation, UNLESS this instruction was
  // already confirmed on a resume (the user said yes) — then it falls through
  // to invoke. This is what lets a mid-plan confirmation resume the WHOLE plan.
  if (verdict.verdict === 'needs_approval' && !deps.approvedInstructions?.has(instruction.id)) {
    if (deps.internalCustomerId) {
      await logFunctionCall(
        {
          sessionId: deps.context.sessionId,
          customerId: deps.internalCustomerId,
          functionName: instruction.tool.name,
          args: finalArgs,
          status: 'pending',
          safetyLevel: instruction.tool.safety,
          requiredConfirmation: true,
        },
        deps.functionCallStore,
      );
    }
    return {
      kind: 'suspend',
      reason: 'awaiting_confirmation',
      instruction,
      verdict,
      question: buildConfirmationPrompt(instruction.tool, finalArgs),
      pendingConfirmation: {
        functionName: instruction.tool.name,
        args: finalArgs,
        description: instruction.tool.description,
        safetyLevel: 'write',
        createdAt: Date.now(),
      },
    };
  }

  // allow (or a pre-approved confirmation that fell through) — apply any
  // transform, then invoke.
  const invokeArgs =
    verdict.verdict === 'allow' && verdict.transformedArgs ? verdict.transformedArgs : finalArgs;
  const argsHash = hashArgs(invokeArgs);

  // Idempotency: a completed identical call (this turn's ledger) never re-runs.
  const prior = state.ledger.find(
    (entry) => entry.instructionId === instruction.id && entry.argsHash === argsHash,
  );
  if (prior?.status === 'success') {
    state.completed.add(instruction.id);
    state.outputs.set(instruction.id, prior.result);
    return { kind: 'complete' };
  }

  const budgetCheck = deps.budgets.noteToolCall(instruction.tool.name, argsHash);
  if (!budgetCheck.ok) {
    return { kind: 'blocked', reason: budgetCheck.reason, fatal: false, cause: 'budget' };
  }

  const invokeResult = await deps.sdk.invoke(instruction.tool.name, invokeArgs, deps.context);
  state.toolCallsExecuted += 1;
  state.executedToolNames.push(instruction.tool.name);
  state.completed.add(instruction.id);
  state.ledger.push({
    instructionId: instruction.id,
    argsHash,
    status: invokeResult.ok ? 'success' : 'error',
    result: invokeResult.ok ? invokeResult.data : invokeResult.error,
    toolName: instruction.tool.name,
    durationMs: invokeResult.durationMs,
  });
  const ledgerEntry = state.ledger[state.ledger.length - 1]!;
  if (deps.turnId) {
    void persistLedgerEntry(deps.context.sessionId, deps.turnId, ledgerEntry, deps.ledgerAelioDb);
  }
  if (invokeResult.ok) {
    state.outputs.set(instruction.id, invokeResult.data);
  }

  if (deps.internalCustomerId) {
    await logFunctionCall(
      {
        sessionId: deps.context.sessionId,
        customerId: deps.internalCustomerId,
        functionName: instruction.tool.name,
        args: invokeArgs,
        result: invokeResult.data,
        status: invokeResult.ok ? 'success' : 'error',
        safetyLevel: instruction.tool.safety,
        durationMs: invokeResult.durationMs,
        errorMessage: invokeResult.error,
      },
      deps.functionCallStore,
    );
  }

  if (invokeResult.ok && deps.onToolSuccess) {
    await deps.onToolSuccess(instruction.tool.name, invokeArgs, invokeResult.data);
  }

  return { kind: 'complete' };
}
