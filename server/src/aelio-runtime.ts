import { randomUUID } from 'node:crypto';
import { createAelioConductor, runConductorEvent, withSessionLock } from '@aelio/core';
import type { RuntimeDeps } from './runtime-deps.js';

export type AelioRuntimeMessage = {
  tenantId: string;
  subjectId: string;
  idempotencyKey: string;
  message: string;
  channel: 'web' | 'whatsapp' | 'sdk';
  /** Present when the reply must be delivered asynchronously through the durable outbox. */
  delivery?: { channel: string; to: string };
};

/** How many times a turn may be recomputed when the subject moves under it. */
const CONFLICT_ATTEMPTS = 3;

export type AelioRuntimeResult =
  | { status: 'duplicate'; eventId: string }
  | { status: 'conflict'; eventId: string }
  | {
      status: 'committed';
      eventId: string;
      commitLsn: number;
      reply: string;
      /** True when the turn parked a plan awaiting an explicit yes/no on a write. */
      awaitingConfirmation: boolean;
    };

/**
 * The authoritative Aelio DB turn. Every capability of the legacy SQLite turn runs here —
 * lifecycle-scoped tools, relevance retrieval, memory recall, the plan/bind/resolve/execute
 * harness, confirmation gating, and durable park/resume — with Aelio DB as the only store.
 */
export async function processAelioRuntimeMessage(
  deps: RuntimeDeps,
  input: AelioRuntimeMessage,
): Promise<AelioRuntimeResult> {
  const store = deps.runtimeStore;
  const suspensionStore = deps.runtimeSuspensionStore;
  if (!store || !suspensionStore) {
    throw new Error('Aelio Runtime requires configured Aelio DB storage');
  }

  const sessionId = `${input.tenantId}:${input.subjectId}`;
  // Serialize a subject's turns in this process. Aelio DB's snapshot CAS is what makes the
  // guarantee hold across processes; the lock keeps a single replica from burning a model call
  // only to lose the commit race against itself.
  return withSessionLock(`aelio-runtime:${sessionId}`, async () => {
    const conductor = createAelioConductor({
      llm: deps.llm,
      sdk: deps.sdkBridge,
      model: deps.config.llm.model,
      maxTokens: deps.config.llm.max_tokens,
      historyWindow: deps.config.session.history_window,
      safety: {
        defaultMode: deps.config.safety.default_mode,
        requireConfirmationFor: deps.config.safety.require_confirmation_for,
        overrides: deps.config.safety.overrides,
      },
      suspensionStore,
      harness: {
        enabled: deps.config.harness.enabled,
        budgets: {
          maxInstructions: deps.config.harness.budgets.max_instructions,
          maxReplans: deps.config.harness.budgets.max_replans,
          maxRecoilsPerIntent: deps.config.harness.budgets.max_recoils_per_intent,
          maxToolCalls: deps.config.harness.budgets.max_tool_calls,
          wallClockMs: deps.config.harness.budgets.wall_clock_ms,
          maxTurnTokens: deps.config.harness.budgets.max_turn_tokens,
        },
        binding: {
          scoreMin: deps.config.harness.binding.score_min,
          ambiguityGap: deps.config.harness.binding.ambiguity_gap,
          cacheTtlMinutes: deps.config.harness.binding.cache_ttl_minutes,
        },
      },
      persona: deps.config.llm.system_prompt ?? null,
      lighthouse: deps.lighthouse,
      ...(deps.tracer ? { tracer: deps.tracer } : {}),
      ...(deps.runtimeMemory ? { memory: deps.runtimeMemory } : {}),
      memoryEnabled: deps.config.memory.enabled,
      memoryRecallLimit: deps.config.memory.recall_limit,
      // Makes a tool call exactly-once per logical event, even if the process dies between the
      // call returning and the turn committing.
      effectJournal: store,
    });

    // A conflict means the subject advanced while this turn was being decided, so the decision is
    // stale and must be recomputed against the new state. Retrying is safe because tool calls are
    // journaled on the event's idempotency key: a replay reuses the recorded result instead of
    // invoking the tool again.
    let eventId = randomUUID();
    let outcome = null;
    for (let attempt = 0; attempt < CONFLICT_ATTEMPTS; attempt += 1) {
      eventId = randomUUID();
      outcome = await runConductorEvent(store, conductor, {
        apiVersion: 'aelio.runtime.event/v1',
        eventId,
        idempotencyKey: input.idempotencyKey,
        tenantId: input.tenantId,
        subjectId: input.subjectId,
        kind: 'user_message',
        payload: {
          message: input.message,
          channel: input.channel,
          ...(input.delivery ? { delivery: input.delivery } : {}),
        },
        receivedAt: Date.now(),
      });
      if (outcome.status !== 'conflict') break;
    }

    if (!outcome || outcome.status === 'conflict') return { status: 'conflict', eventId };
    if (outcome.status === 'duplicate') return { status: 'duplicate', eventId };

    const parked = await suspensionStore.get(sessionId);
    return {
      status: 'committed',
      eventId,
      commitLsn: outcome.commitLsn,
      reply: outcome.decision.kind === 'reply' ? outcome.decision.text : '',
      awaitingConfirmation: parked?.reason === 'awaiting_confirmation',
    };
  });
}
