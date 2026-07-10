import type { FunctionDefinition, InvocationContext, StateDefinition } from '@aelio/protocol';
import type { ChatMessage, LLMProvider } from '@aelio/llm';
import type { AelioDatabase } from '@aelio/db';
import type { SafetyConfig } from '../safety/policy.js';
import type { SdkBridge } from '../sdk-bridge/types.js';
import type { ToolLoopResult } from '../runtime/tool-loop.js';
import type { LighthouseService } from '../lighthouse/index.js';
import { BudgetMeter } from './budgets.js';
import { bindInstructions } from './binder.js';
import { resolvePlan } from './resolver.js';
import { executePlan, newExecutorState, hashArgs, type ExecOutcome } from './executor.js';
import { runPlanner } from './planner.js';
import { runSynthesis } from './synthesis.js';
import { rehydrateSuspension, toSuspensionPayload } from './resume.js';
import { applyStateTransition } from './transitions.js';
import type { SuspensionStore } from './suspension.js';
import type { HarnessTracer, TraceKind } from './traces.js';
import {
  DEFAULT_BINDING,
  DEFAULT_BUDGETS,
  type EmitTurn,
  type HarnessBindingConfig,
  type HarnessBudgets,
  type ResolvedPlan,
} from './schema.js';

export { runPlanner } from './planner.js';
export { runSynthesis } from './synthesis.js';
export { BudgetMeter } from './budgets.js';
export { bindInstructions } from './binder.js';
export { resolvePlan, nextWave } from './resolver.js';
export { executePlan, newExecutorState, hashArgs } from './executor.js';
export { evaluateGate } from './gates.js';
export { rehydrateSuspension, toSuspensionPayload } from './resume.js';
export { applyStateTransition } from './transitions.js';

export type HarnessRunInput = {
  database?: AelioDatabase;
  internalCustomerId?: string;
  llm: LLMProvider;
  sdk: SdkBridge;
  /** State-scoped, relevance-filtered tool set for this turn. */
  functions: FunctionDefinition[];
  model: string;
  maxTokens: number;
  system: string;
  history: ChatMessage[];
  userMessage: string;
  context: InvocationContext;
  safety: SafetyConfig;
  /** Active lifecycle state (for executor guard checks). */
  state?: StateDefinition;
  presentFields?: Set<string>;
  lighthouse?: LighthouseService;
  tracer?: HarnessTracer;
  suspensionStore?: SuspensionStore;
  budgets?: HarnessBudgets;
  binding?: HarnessBindingConfig;
  turnId?: string;
};

/**
 * The harness: plan → bind → resolve → execute (wavefront) → synthesize, with
 * the LLM proposing and deterministic code disposing. Returns the same shape as
 * the legacy tool loop so the surrounding turn is agnostic to which engine ran.
 *
 * Phase-4 scope: full DAG execution with derived rock/river ordering, gates on
 * concrete args, idempotency ledger, and one level of segment replanning on a
 * surprising rock result. Suspension (recoil/confirmation) currently ends the
 * turn with the pending prompt; Phase 5 persists it for resume.
 */
export async function runHarness(input: HarnessRunInput): Promise<ToolLoopResult> {
  const budgets = new BudgetMeter(input.budgets ?? DEFAULT_BUDGETS);
  const binding = input.binding ?? DEFAULT_BINDING;
  const trace = (kind: TraceKind, payload: unknown) =>
    input.tracer?.trace({
      turnId: input.turnId ?? 'unknown',
      sessionId: input.context.sessionId,
      kind,
      payload,
    });

  // ---- Resume-first: a parked (recoil) plan intercepts this message ----
  // Confirmation resume is handled upstream by the pending-confirmation path;
  // here we only resume awaiting_info suspensions.
  if (input.suspensionStore) {
    const suspended = await input.suspensionStore.get(input.context.sessionId);
    if (suspended && suspended.reason === 'awaiting_info') {
      const resumed = rehydrateSuspension(
        suspended.payload,
        input.userMessage,
        input.functions,
        input.lighthouse?.getHash() ?? null,
      );
      if (resumed.ok) {
        trace('resume', { pending: resumed.pendingInstructionId });
        await input.suspensionStore.clear(input.context.sessionId);
        const outcome = await executePlan(resumed.plan, resumed.state, buildExecutorDeps(input, budgets, trace));
        return finishTurn(input, resumed.plan.goal, resumed.state, outcome, budgets, trace, {
          recoilCount: suspended.payload.recoilCount + 1,
          userMessage: suspended.payload.userMessage,
          plan: resumed.plan,
        });
      }
      if (resumed.reason === 'answer_rejected') {
        // Keep the plan parked; re-ask the same question.
        return {
          reply: suspended.payload.ask.question,
          toolCallsExecuted: 0,
          executedToolNames: [],
        };
      }
      // stale_registry / tool_gone → drop the suspension and plan fresh below.
      await input.suspensionStore.clear(input.context.sessionId);
      trace('resume', { discarded: resumed.reason });
    }
  }

  // ---- Pass 1: merged router + planner ----
  let planner = await runPlanner({
    llm: input.llm,
    model: input.model,
    maxTokens: input.maxTokens,
    system: input.system,
    history: input.history,
    userMessage: input.userMessage,
  });
  trace('plan', { turn: planner.turn, degraded: planner.degraded });

  // ---- Refusal: fail-open on feasibility ----
  if (planner.turn.mode === 'refuse' && input.lighthouse && !planner.degraded) {
    const score = await input.lighthouse.probeFeasibility(input.userMessage);
    if (score >= binding.scoreMin) {
      const replanCheck = budgets.noteReplan();
      if (replanCheck.ok) {
        planner = await runPlanner({
          llm: input.llm,
          model: input.model,
          maxTokens: input.maxTokens,
          system: `${input.system}\n\nNote: the capability index suggests this request MAY be servable with the available tools. Refuse only if you are certain it is not; otherwise emit a plan.`,
          history: input.history,
          userMessage: input.userMessage,
          purpose: 'replan',
        });
        trace('repair', { reason: 'feasibility_probe', score, turn: planner.turn });
      }
    }
  }

  if (planner.turn.mode === 'reply') {
    return { reply: planner.turn.text, toolCallsExecuted: 0, executedToolNames: [] };
  }
  if (planner.turn.mode === 'refuse') {
    return { reply: planner.turn.reason, toolCallsExecuted: 0, executedToolNames: [] };
  }

  // ---- Deep path ----
  const plan: Extract<EmitTurn, { mode: 'plan' }> = planner.turn;
  const sizeCheck = budgets.checkPlanSize(plan.instructions.length);
  if (!sizeCheck.ok) {
    trace('budget', sizeCheck);
    return {
      reply:
        'That request needs more steps than I can safely take in one go — could you split it into smaller asks?',
      toolCallsExecuted: 0,
      executedToolNames: [],
    };
  }

  // ---- Bind → resolve ----
  const bindResult = await bindInstructions(plan.instructions, input.functions, input.lighthouse, binding);
  trace('bind', {
    bound: bindResult.bound.map((b) => ({ id: b.instruction.id, tool: b.tool.name })),
    unbound: bindResult.ok ? [] : bindResult.unbound.map((i) => i.id),
  });
  if (!bindResult.ok) {
    // Unbindable capability — nothing in the registry serves it. End gracefully.
    return {
      reply: `I don't have a way to "${bindResult.unbound[0]?.capability ?? 'do that'}" right now.`,
      toolCallsExecuted: 0,
      executedToolNames: [],
    };
  }

  const resolveResult = resolvePlan(plan.goal, plan.nudge, bindResult.bound);
  if (!resolveResult.ok) {
    trace('repair', { reason: 'resolve_failed', detail: resolveResult.reason });
    // A structurally broken plan (cycle/dup) → one replan, else give up cleanly.
    return {
      reply: 'I got the steps tangled up — could you restate what you need?',
      toolCallsExecuted: 0,
      executedToolNames: [],
    };
  }

  // ---- Execute (wavefront) ----
  const state = newExecutorState();
  const outcome = await executePlan(resolveResult.plan, state, buildExecutorDeps(input, budgets, trace));

  return finishTurn(input, plan.goal, state, outcome, budgets, trace, {
    recoilCount: 0,
    userMessage: input.userMessage,
    plan: resolveResult.plan,
  });
}

function buildExecutorDeps(
  input: HarnessRunInput,
  budgets: BudgetMeter,
  trace: (kind: TraceKind, payload: unknown) => void,
) {
  return {
    sdk: input.sdk,
    context: input.context,
    safety: input.safety,
    ...(input.state ? { state: input.state } : {}),
    ...(input.presentFields ? { presentFields: input.presentFields } : {}),
    budgets,
    ...(input.database ? { database: input.database } : {}),
    ...(input.internalCustomerId ? { internalCustomerId: input.internalCustomerId } : {}),
    trace: (kind: 'wave' | 'gate' | 'repair', payload: unknown) => trace(kind, payload),
    // Declarative lifecycle transitions on tool success. context.customerId is
    // the external id (what upsertCustomerLifecycleState keys on).
    ...(input.database && input.state?.transitions?.length
      ? {
          onToolSuccess: async (toolName: string) => {
            const applied = await applyStateTransition({
              db: input.database!.db,
              externalId: input.context.customerId,
              state: input.state,
              toolName,
              presentFields: input.presentFields ?? new Set<string>(),
            });
            if (applied) {
              trace('repair', { transitionedTo: applied.transitionedTo, afterTool: toolName });
            }
          },
        }
      : {}),
  };
}

async function finishTurn(
  input: HarnessRunInput,
  goal: string,
  state: ReturnType<typeof newExecutorState>,
  outcome: ExecOutcome,
  budgets: BudgetMeter,
  trace: (kind: TraceKind, payload: unknown) => void,
  meta: { recoilCount: number; userMessage: string; plan?: ResolvedPlan },
): Promise<ToolLoopResult> {
  if (outcome.kind === 'suspend') {
    trace('suspend', { reason: outcome.reason, instruction: outcome.instruction.id });

    // Recoil: persist the plan+ledger so the next message resumes instead of
    // restarting. Guarded by the recoil budget so a user who can't supply the
    // value isn't asked forever.
    if (
      outcome.reason === 'awaiting_info' &&
      input.suspensionStore &&
      meta.plan &&
      meta.recoilCount < (input.budgets ?? DEFAULT_BUDGETS).maxRecoilsPerIntent
    ) {
      const payload = toSuspensionPayload({
        goal,
        userMessage: meta.userMessage,
        registryHash: input.lighthouse?.getHash() ?? '',
        plan: meta.plan,
        state,
        pendingInstructionId: outcome.instruction.id,
        ask: {
          ...(outcome.verdict.verdict === 'needs_info' && outcome.verdict.missing[0]
            ? { field: outcome.verdict.missing[0] }
            : {}),
          toolName: outcome.instruction.tool.name,
          question: outcome.question,
        },
        recoilCount: meta.recoilCount,
      });
      await input.suspensionStore.suspend(input.context.sessionId, 'awaiting_info', payload);
    }

    return {
      reply: outcome.question,
      toolCallsExecuted: state.toolCallsExecuted,
      executedToolNames: state.executedToolNames,
      ...(outcome.pendingConfirmation ? { pendingConfirmation: outcome.pendingConfirmation } : {}),
    };
  }

  if (outcome.kind === 'blocked') {
    trace('budget', { blocked: outcome.reason });
    // A policy denial is a real, user-facing reason; a budget stop is not.
    const reply = state.ledger.length === 0
      ? outcome.reason
      : await runSynthesis({
          llm: input.llm,
          model: input.model,
          maxTokens: input.maxTokens,
          system: input.system,
          history: input.history,
          userMessage: input.userMessage,
          goal,
          ledger: state.ledger,
        });
    return { reply, toolCallsExecuted: state.toolCallsExecuted, executedToolNames: state.executedToolNames };
  }

  // Complete → synthesize final reply from the ledger.
  const reply = await runSynthesis({
    llm: input.llm,
    model: input.model,
    maxTokens: input.maxTokens,
    system: input.system,
    history: input.history,
    userMessage: input.userMessage,
    goal,
    ledger: state.ledger,
  });
  trace('synthesis', { replyPreview: reply.slice(0, 200) });
  return { reply, toolCallsExecuted: state.toolCallsExecuted, executedToolNames: state.executedToolNames };
}
