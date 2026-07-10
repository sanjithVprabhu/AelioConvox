import { z } from 'zod';
import type { FunctionDefinition } from '@aelio/protocol';

// ---------------------------------------------------------------------------
// Pass 1 output — the `emit_turn` forced tool call
// ---------------------------------------------------------------------------

/**
 * One capability-level instruction from the planner. The planner names WHAT to
 * do; `tool` is only a suggestion when the planner was shown a matching tool
 * card (most instructions arrive pre-bound this way). The binder verifies or
 * fills `tool`; the resolver wires dependencies from the bound schemas — the
 * LLM never declares rock/river itself.
 */
export const PlanInstructionSchema = z.object({
  id: z.string().min(1),
  capability: z.string().min(1),
  tool: z.string().min(1).optional(),
  /** Arg values the planner could already read from the conversation. Hints, not truth. */
  args_hint: z.record(z.unknown()).optional(),
  /** Names of values this step yields for later steps (e.g. ["cart_id"]). */
  produces: z.array(z.string().min(1)).optional(),
});
export type PlanInstruction = z.infer<typeof PlanInstructionSchema>;

export const EmitTurnSchema = z.discriminatedUnion('mode', [
  // Shallow: answerable directly — no tenant data, no side effects, no account state.
  z.object({ mode: z.literal('reply'), text: z.string().min(1) }),
  // Infeasible: surfaced only after the feasibility probe agrees (fail-open).
  z.object({ mode: z.literal('refuse'), reason: z.string().min(1) }),
  // Deep: a capability-level plan for the executor.
  z.object({
    mode: z.literal('plan'),
    goal: z.string().min(1),
    instructions: z.array(PlanInstructionSchema).min(1),
    /** Optional user-facing note when the plan advances a state/flow nudge. */
    nudge: z.string().optional(),
  }),
]);
export type EmitTurn = z.infer<typeof EmitTurnSchema>;

/**
 * The JSON input_schema advertised to the LLM for the forced `emit_turn` call.
 * Hand-written mirror of EmitTurnSchema above — keep the two in sync (they sit
 * together on purpose; a generic zod→json-schema converter is more machinery
 * than this one union warrants).
 */
export const EMIT_TURN_INPUT_SCHEMA: Record<string, unknown> = {
  type: 'object',
  properties: {
    mode: {
      type: 'string',
      enum: ['reply', 'refuse', 'plan'],
      description:
        'reply = answer directly (no tenant data, no side effects). refuse = the request cannot be served by this product. plan = actions are required.',
    },
    text: { type: 'string', description: 'mode=reply: the reply text.' },
    reason: { type: 'string', description: 'mode=refuse: why this cannot be done.' },
    goal: { type: 'string', description: 'mode=plan: one sentence describing the outcome.' },
    nudge: {
      type: 'string',
      description: 'mode=plan: optional note nudging the user toward a state/flow requirement.',
    },
    instructions: {
      type: 'array',
      description:
        'mode=plan: capability-level steps. Do NOT sequence or parallelize them yourself — the executor derives ordering from data dependencies.',
      items: {
        type: 'object',
        properties: {
          id: { type: 'string', description: 'Short unique id, e.g. "a", "b".' },
          capability: { type: 'string', description: 'What this step accomplishes.' },
          tool: {
            type: 'string',
            description: 'Name of the tool to use, when one of the shown tools clearly fits.',
          },
          args_hint: {
            type: 'object',
            description: 'Argument values already evident from the conversation.',
          },
          produces: {
            type: 'array',
            items: { type: 'string' },
            description: 'Value names this step yields for later steps (e.g. "cart_id").',
          },
        },
        required: ['id', 'capability'],
      },
    },
  },
  required: ['mode'],
};

// ---------------------------------------------------------------------------
// Bound + resolved plan (post-binder, post-resolver)
// ---------------------------------------------------------------------------

/** Where a required argument's value comes from. */
export type ArgSource =
  | { kind: 'literal'; value: unknown }
  | { kind: 'output'; instructionId: string; field: string }
  | { kind: 'missing' };

export type ResolvedInstruction = {
  id: string;
  capability: string;
  /** Bound tool (live-registry definition). */
  tool: FunctionDefinition;
  /** Per-required-param sources; optional params ride along as literals when hinted. */
  argSources: Record<string, ArgSource>;
  /** Instruction ids whose outputs this step needs — the derived rock edges. */
  needs: string[];
  effect: 'read' | 'write';
  produces: string[];
};

export type ResolvedPlan = {
  goal: string;
  nudge?: string;
  instructions: ResolvedInstruction[];
};

export type InstructionStatus = 'pending' | 'running' | 'success' | 'error' | 'skipped';

export type LedgerEntry = {
  instructionId: string;
  argsHash: string;
  status: 'success' | 'error';
  result: unknown;
  toolName: string;
  durationMs: number;
};

// ---------------------------------------------------------------------------
// Gates
// ---------------------------------------------------------------------------

export type GateVerdict =
  | { verdict: 'allow'; transformedArgs?: Record<string, unknown> }
  | { verdict: 'deny_fatal'; reason: string }
  | { verdict: 'needs_approval'; reason: string }
  | { verdict: 'needs_info'; missing: string[]; reason: string };

// ---------------------------------------------------------------------------
// Suspension (recoil / confirmation) — persisted payload
// ---------------------------------------------------------------------------

export const SuspensionReasonSchema = z.enum(['awaiting_info', 'awaiting_confirmation']);
export type SuspensionReason = z.infer<typeof SuspensionReasonSchema>;

/**
 * Everything needed to resume a parked plan in a later turn (or after a
 * restart): the serialized plan (tool names, not definitions — rebound against
 * the live registry on resume), the execution ledger so completed steps never
 * re-run, and what we asked the user for.
 */
export const SuspendedPlanPayloadSchema = z.object({
  version: z.literal(1),
  goal: z.string(),
  userMessage: z.string(),
  registryHash: z.string(),
  instructions: z.array(
    z.object({
      id: z.string(),
      capability: z.string(),
      toolName: z.string(),
      argSources: z.record(z.unknown()),
      needs: z.array(z.string()),
      effect: z.enum(['read', 'write']),
      produces: z.array(z.string()),
    }),
  ),
  ledger: z.array(
    z.object({
      instructionId: z.string(),
      argsHash: z.string(),
      status: z.enum(['success', 'error']),
      result: z.unknown(),
      toolName: z.string(),
      durationMs: z.number(),
    }),
  ),
  pendingInstructionId: z.string(),
  ask: z.object({
    /** For awaiting_info: the parameter being collected. */
    field: z.string().optional(),
    /** For awaiting_info: the tool whose param spec validates the answer. */
    toolName: z.string().optional(),
    question: z.string(),
  }),
  /** For awaiting_confirmation: the exact call being confirmed. */
  pendingCall: z
    .object({
      toolName: z.string(),
      args: z.record(z.unknown()),
      safetyLevel: z.enum(['write', 'destructive']),
      description: z.string(),
    })
    .optional(),
  recoilCount: z.number().int().nonnegative().default(0),
});
export type SuspendedPlanPayload = z.infer<typeof SuspendedPlanPayloadSchema>;

// ---------------------------------------------------------------------------
// Budgets
// ---------------------------------------------------------------------------

export type HarnessBudgets = {
  maxInstructions: number;
  maxReplans: number;
  maxRecoilsPerIntent: number;
  maxToolCalls: number;
  wallClockMs: number;
  maxTurnTokens: number;
};

export const DEFAULT_BUDGETS: HarnessBudgets = {
  maxInstructions: 12,
  maxReplans: 2,
  maxRecoilsPerIntent: 3,
  maxToolCalls: 15,
  wallClockMs: 60_000,
  maxTurnTokens: 30_000,
};

export type HarnessBindingConfig = {
  scoreMin: number;
  ambiguityGap: number;
  cacheTtlMinutes: number;
};

export const DEFAULT_BINDING: HarnessBindingConfig = {
  scoreMin: 0.55,
  ambiguityGap: 0.08,
  cacheTtlMinutes: 1440,
};
