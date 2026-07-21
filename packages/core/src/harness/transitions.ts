import type { StateDefinition } from '@aelio/protocol';
import { upsertCustomerLifecycleState } from '../lifecycle/index.js';
import type { ConvoxCustomerStore } from '../storage/customers.js';

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
  externalId: string;
  state: StateDefinition | undefined;
  toolName: string;
  presentFields: Set<string>;
  customerStore: ConvoxCustomerStore;
}): Promise<{ transitionedTo: string } | null> {
  if (!input.customerStore) {
    throw new Error('Sunjet customerStore is required');
  }
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
      input.externalId,
      transition.to,
      `auto: ${input.toolName} succeeded`,
      input.customerStore,
    );
    return { transitionedTo: transition.to };
  }
  return null;
}
