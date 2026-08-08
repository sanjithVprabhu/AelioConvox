import type { JsonValue, RuntimeScheduledEvent } from '@aelio/core';
import { processAelioRuntimeMessage } from '../aelio-runtime.js';
import type { RuntimeDeps } from '../runtime-deps.js';

const WORKER_ID = `runtime-scheduler:${process.pid}`;

/**
 * Consumes leased Aelio DB scheduled events. The schedule row is the durable inbox for
 * asynchronous channels: a provider webhook can acknowledge after its event is committed,
 * while LLM/conductor execution happens outside the request lifecycle.
 */
export function startRuntimeSchedulerWorker(deps: RuntimeDeps) {
  if (!deps.runtimeStore) return () => undefined;
  let active = false;
  const tick = () => {
    if (active) return;
    active = true;
    void drain(deps).catch((error) => console.error('[aelio] runtime scheduler failed:', error)).finally(() => { active = false; });
  };
  const interval = setInterval(tick, 250);
  interval.unref();
  tick();
  return () => clearInterval(interval);
}

async function drain(deps: RuntimeDeps): Promise<void> {
  const store = deps.runtimeStore!;
  const now = Date.now();
  await store.requeueExpiredScheduledEvents(now);
  const events = await store.listDueScheduledEvents(now);
  for (const event of events) {
    const claimed = await store.claimScheduledEvent(event, WORKER_ID);
    if (!claimed) continue;
    try {
      await executeScheduledMessage(deps, claimed);
      if (!(await store.completeScheduledEvent(claimed))) {
        throw new Error(`lost scheduled-event lease for ${claimed.scheduleId}`);
      }
    } catch (error) {
      // Retain the lease until it expires; requeueExpiredScheduledEvents gives another worker a
      // bounded, durable retry opportunity without acknowledging the provider event as done.
      console.error('[aelio] scheduled runtime event failed:', claimed.scheduleId, error);
    }
  }
}

async function executeScheduledMessage(deps: RuntimeDeps, event: RuntimeScheduledEvent): Promise<void> {
  const payload = object(event.payload);
  const message = string(payload.message);
  const channel = payload.channel;
  if (!message || (channel !== 'web' && channel !== 'whatsapp' && channel !== 'sdk')) {
    throw new Error(`scheduled event ${event.scheduleId} has an invalid runtime message payload`);
  }
  const deliveryValue = payload.delivery;
  const delivery = deliveryValue === undefined ? undefined : deliveryPayload(deliveryValue);
  const result = await processAelioRuntimeMessage(deps, {
    tenantId: event.tenantId,
    subjectId: event.subjectId,
    idempotencyKey: `scheduled:${event.scheduleId}`,
    message,
    channel,
    ...(delivery ? { delivery } : {}),
  });
  if (result.status === 'conflict') throw new Error(`runtime conflict for scheduled event ${event.scheduleId}`);
}

function object(value: JsonValue): Record<string, JsonValue> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('scheduled event payload must be an object');
  return value;
}
function string(value: JsonValue | undefined): string { return typeof value === 'string' ? value : ''; }
/**
 * Delivery channel is intentionally an open string: a tenant SDK may own channels Aelio has no
 * built-in adapter for, and the outbox routes anything it does not recognise through the SDK
 * bridge with the durable effect id as the provider idempotency key.
 */
function deliveryPayload(value: JsonValue): { channel: string; to: string } {
  const delivery = object(value);
  const channel = string(delivery.channel);
  const to = string(delivery.to);
  if (!channel || !to) throw new Error('scheduled runtime delivery is invalid');
  return { channel, to };
}
