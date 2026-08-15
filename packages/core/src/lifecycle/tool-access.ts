import type { FunctionDefinition, StateDefinition } from '@aelio/protocol';

/**
 * Resolve lifecycle access without expanding category rules into long tool-name lists.
 *
 * Semantics:
 * - any matching block rule denies the tool;
 * - when at least one non-empty allow rule exists, matching any allow rule admits it;
 * - with no allow rules, tools are admitted unless blocked;
 * - an omitted tool intent falls back to its name, matching the rest of the runtime.
 */
export function isFunctionAllowedByState(
  fn: FunctionDefinition,
  state?: StateDefinition,
): boolean {
  if (!state) return true;

  const intent = fn.intent ?? fn.name;
  const blocked =
    (state.blockedTools?.includes(fn.name) ?? false) ||
    (state.blockedIntents?.includes(intent) ?? false) ||
    (state.blockedSafety?.includes(fn.safety) ?? false);
  if (blocked) return false;

  const hasAllowRules =
    (state.allowedTools?.length ?? 0) > 0 ||
    (state.allowedIntents?.length ?? 0) > 0 ||
    (state.allowedSafety?.length ?? 0) > 0;
  if (!hasAllowRules) return true;

  return (
    (state.allowedTools?.includes(fn.name) ?? false) ||
    (state.allowedIntents?.includes(intent) ?? false) ||
    (state.allowedSafety?.includes(fn.safety) ?? false)
  );
}
