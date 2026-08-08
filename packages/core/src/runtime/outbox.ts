import type { AelioRuntimeStore, RuntimeOutboxEffect } from './aelio-runtime-store.js';

/** An effect handler must forward `effectId` as its provider/tool idempotency key. */
export type RuntimeEffectHandler = (effect: RuntimeOutboxEffect) => Promise<void>;

/**
 * Decide whether an effect whose dispatch outcome is unknown may be retried.
 *
 * Only say yes when a redelivery would be deduplicated downstream. Internal effects are
 * idempotent by construction; a customer-visible send is only safe when the channel honours
 * `aelio_effect_id`.
 */
export type RedeliverySafetyPolicy = (effect: RuntimeOutboxEffect) => boolean;

/** Internal effects are keyed by derived ids and re-running them converges on the same state. */
export const INTERNAL_EFFECT_KINDS = new Set(['harness.start', 'harness.resume', 'continuation.create']);

/**
 * Bounded outbox dispatcher. Claiming and final status transitions use CAS in Aelio DB; handler
 * execution happens only after a durable `dispatching` claim. A worker crash leaves the record for
 * the lease reaper, which either retries it or parks it as `unknown` per the safety policy —
 * never silently loses it, and never silently repeats a customer-visible send.
 */
export async function dispatchPendingEffects(
  store: AelioRuntimeStore,
  handler: RuntimeEffectHandler,
  limit = 32,
  isRedeliverySafe: RedeliverySafetyPolicy = (effect) => INTERNAL_EFFECT_KINDS.has(effect.kind),
): Promise<{ claimed: number; delivered: number; failed: number }> {
  await store.requeueExpiredEffects(Date.now(), limit, isRedeliverySafe);
  const effects = await store.listPendingEffects(limit);
  let claimed = 0;
  let delivered = 0;
  let failed = 0;
  for (const effect of effects) {
    const claimedEffect = await store.claimEffect(effect);
    if (!claimedEffect) continue;
    claimed += 1;
    try {
      await handler(claimedEffect);
      if (await store.completeEffect(claimedEffect, 'delivered')) delivered += 1;
    } catch (error) {
      const outcome = await store.retryEffect(claimedEffect, error);
      if (outcome === 'failed') failed += 1;
    }
  }
  return { claimed, delivered, failed };
}
