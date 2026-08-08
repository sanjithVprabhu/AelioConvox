import { randomUUID } from 'node:crypto';
import type { AelioRuntimeStore } from './aelio-runtime-store.js';
import {
  assertRuntimeEvent,
  type JsonValue,
  type RuntimeDecisionV1,
  type RuntimeEffectV1,
  type RuntimeEventV1,
  type RuntimeSnapshotV1,
} from './contracts.js';

export type Conductor = {
  /** Select only one of the explicit runtime decisions; it cannot directly run a tool or send a message. */
  decide(input: { event: RuntimeEventV1; snapshot: RuntimeSnapshotV1 | null }): Promise<RuntimeDecisionV1>;
};

export type ConductorResult =
  | { status: 'committed'; decision: RuntimeDecisionV1; snapshot: RuntimeSnapshotV1; commitLsn: number }
  | { status: 'duplicate' }
  | { status: 'conflict' };

/**
 * The sole control-plane entry point. It reads the current subject snapshot, asks a constrained
 * conductor for the next action, then writes the event claim + updated snapshot + immutable
 * decision ledger + any reply/continuation effects atomically. The outbox dispatcher is the
 * only component allowed to make those effects external.
 */
export async function runConductorEvent(
  store: AelioRuntimeStore,
  conductor: Conductor,
  event: RuntimeEventV1,
): Promise<ConductorResult> {
  assertRuntimeEvent(event);
  const snapshot = await store.loadSnapshot(event.tenantId, event.subjectId);
  const decision = await conductor.decide({ event, snapshot });
  const instanceId = decision.kind === 'continue_harness' ? decision.instanceId : randomUUID();
  const state = decision.state ?? snapshot?.state ?? {};
  const effects = decisionEffects(decision, event, instanceId);
  const commit = await store.commit({
    event,
    state,
    instanceId,
    // The decision was computed from this exact revision; anything newer invalidates it.
    expectedRevision: snapshot?.revision ?? 0,
    ledger: [
      {
        recordId: randomUUID(), instanceId, kind: 'event_claimed',
        payload: { eventKind: event.kind, idempotencyKey: event.idempotencyKey },
      },
      { recordId: randomUUID(), instanceId, kind: 'decision', payload: decision as unknown as JsonValue },
    ],
    effects,
  });
  if (!commit.applied) return { status: commit.reason === 'duplicate_event' ? 'duplicate' : 'conflict' };
  return { status: 'committed', decision, snapshot: commit.snapshot, commitLsn: commit.commitLsn };
}

function decisionEffects(decision: RuntimeDecisionV1, event: RuntimeEventV1, instanceId: string): RuntimeEffectV1[] {
  if (decision.kind === 'reply') {
    // Synchronous surfaces (for example the web socket) return the reply directly. Channel
    // adapters that require asynchronous delivery add an explicit `reply.send` effect to the
    // decision, keeping delivery exactly-once policy at the channel boundary.
    return decision.effects ?? [];
  }
  if (decision.kind === 'start_harness') {
    return [{
      effectId: randomUUID(), idempotencyKey: `harness:${event.eventId}`, kind: 'harness.start',
      payload: { artifactId: decision.artifactId, version: decision.version, input: decision.input, instanceId },
    }];
  }
  if (decision.kind === 'await_input') {
    return [{
      effectId: randomUUID(), idempotencyKey: `continuation:${decision.continuation.token}`, kind: 'continuation.create',
      payload: { ...decision.continuation, instanceId },
    }];
  }
  return [];
}
