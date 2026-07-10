import type { StateDefinition } from '@aelio/protocol';
import type { AelioDatabase } from '@aelio/db';
import { upsertCustomerLifecycleState } from '../lifecycle/index.js';

/**
 * Declarative lifecycle transitions. When a tool succeeds and the active state
 * declares `transitions: [{ on_tool_success, to, guard }]`, the executor moves
 * the customer forward — deterministically, in code. This is how state stays in
 * sync with reality (create_order succeeds → awaiting_payment) instead of the
 * LLM being trusted to "advance the state".
 *
 * An SDK `set_state` push always overrides this later: the tenant's own backend
 * is the ultimate source of truth for lifecycle.
 */
export async function applyStateTransition(input: {
  db: AelioDatabase['db'];
  externalId: string;
  state: StateDefinition | undefined;
  toolName: string;
  presentFields: Set<string>;
}): Promise<{ transitionedTo: string } | null> {
  const transitions = input.state?.transitions;
  if (!transitions || transitions.length === 0) {
    return null;
  }

  for (const transition of transitions) {
    if (transition.on_tool_success !== input.toolName) {
      continue;
    }
    const requires = transition.guard?.requires_fields ?? [];
    const unmet = requires.some((field) => !input.presentFields.has(field));
    if (unmet) {
      continue;
    }
    await upsertCustomerLifecycleState(
      input.db,
      input.externalId,
      transition.to,
      `auto: ${input.toolName} succeeded`,
    );
    return { transitionedTo: transition.to };
  }
  return null;
}
