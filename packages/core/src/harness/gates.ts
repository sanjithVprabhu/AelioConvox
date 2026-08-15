import type { FunctionDefinition, StateDefinition } from '@aelio/protocol';
import { evaluateSafety, type SafetyConfig } from '../safety/policy.js';
import { findMissingRequiredArgs } from '../runtime/tool-schema.js';
import type { GateVerdict } from './schema.js';

export type GateContext = {
  safety: SafetyConfig;
  /** The customer's active lifecycle state, if any. */
  state?: StateDefinition;
  /** Named customer-profile fields present (for guard `requires_fields`). */
  presentFields?: Set<string>;
};

/**
 * The verdict engine — the deterministic "code disposes" half. Evaluated on the
 * concrete, resolved args immediately before every invocation, no matter what
 * the plan proposed. Order matters: missing args first (recoverable via recoil),
 * then state gating, then guard conditions, then safety (deny/confirm).
 */
export function evaluateGate(
  fn: FunctionDefinition,
  args: Record<string, unknown>,
  ctx: GateContext,
): GateVerdict {
  const missing = findMissingRequiredArgs(fn.params, args);
  if (missing.length > 0) {
    return {
      verdict: 'needs_info',
      missing,
      reason: `Missing required argument(s): ${missing.join(', ')}`,
    };
  }

  // 2. State tool-gating (blocked in this state) → fatal, with a nudge reason.
  if (ctx.state) {
    if (ctx.state.blockedTools?.includes(fn.name)) {
      return {
        verdict: 'deny_fatal',
        reason: `"${fn.name}" is not available while in the ${ctx.state.id} stage.`,
      };
    }
    if (ctx.state.allowedTools && !ctx.state.allowedTools.includes(fn.name)) {
      return {
        verdict: 'deny_fatal',
        reason: `"${fn.name}" is not available while in the ${ctx.state.id} stage.`,
      };
    }

    // 3. State guard conditions unmet → needs_info (nudge for the missing prerequisite).
    const requires = ctx.state.guards?.requires_fields ?? [];
    const unmet = requires.filter((field) => !ctx.presentFields?.has(field));
    if (unmet.length > 0) {
      return {
        verdict: 'needs_info',
        missing: unmet,
        reason: `The ${ctx.state.id} stage still needs: ${unmet.join(', ')}.`,
      };
    }
  }

  // 4. Safety (deny / confirm).
  const decision = evaluateSafety(fn, ctx.safety);
  if (!decision.allowed) {
    return { verdict: 'deny_fatal', reason: decision.reason };
  }
  if (decision.requiresConfirmation) {
    return {
      verdict: 'needs_approval',
      reason: `${fn.name} needs your confirmation before it runs.`,
    };
  }

  return { verdict: 'allow' };
}
