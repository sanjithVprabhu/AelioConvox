import {
  dispatchPendingEffects,
  INTERNAL_EFFECT_KINDS,
  type JsonValue,
  type RuntimeOutboxEffect,
} from '@aelio/core';
import type { RuntimeDeps } from '../runtime-deps.js';
import { runHarnessEffect, resumeHarnessInstance } from '../runtime-artifact-runner.js';

/** Dispatches durable Aelio Runtime effects. It never reads or writes the legacy job queue. */
export function startRuntimeOutboxWorker(deps: RuntimeDeps) {
  if (!deps.runtimeStore) return () => undefined;
  let active = false;
  const tick = () => {
    if (active) return;
    active = true;
    void dispatchPendingEffects(
      deps.runtimeStore!,
      (effect) => deliverEffect(deps, effect),
      32,
      (effect) => isRedeliverySafe(deps, effect),
    )
      .catch((error) => console.error('[aelio] runtime outbox dispatch failed:', error))
      .finally(() => { active = false; });
  };
  const interval = setInterval(tick, 250);
  interval.unref();
  tick();
  return () => clearInterval(interval);
}

/**
 * After a crash mid-dispatch we do not know whether the provider accepted the message. Retrying is
 * only safe where a duplicate would be suppressed downstream:
 *  - internal effects converge, so they always retry;
 *  - `web` replies are read from committed state, never pushed, so there is nothing to duplicate;
 *  - SDK channels receive `aelio_effect_id` and are contractually required to dedupe on it;
 *  - Meta WhatsApp offers no idempotency key, so an ambiguous send is parked as `unknown` for
 *    reconciliation rather than risking a second message to a real customer.
 */
function isRedeliverySafe(deps: RuntimeDeps, effect: RuntimeOutboxEffect): boolean {
  if (INTERNAL_EFFECT_KINDS.has(effect.kind)) return true;
  if (effect.kind !== 'reply.send') return false;
  const channel = string(object(effect.payload).channel);
  if (channel === 'web') return true;
  if (channel === 'whatsapp') return false;
  return deps.sdkBridge.hasSendCapability();
}

async function deliverEffect(deps: RuntimeDeps, effect: RuntimeOutboxEffect): Promise<void> {
  if (effect.kind === 'harness.start') {
    await runHarnessEffect(deps, effect);
    return;
  }
  if (effect.kind === 'harness.resume') {
    await resumeHarnessInstance(deps, effect);
    return;
  }
  if (effect.kind === 'continuation.create') {
    const payload = object(effect.payload);
    const token = string(payload.token);
    const prompt = string(payload.prompt);
    const expiresAt = number(payload.expiresAt);
    const instanceId = string(payload.instanceId);
    if (!token || !prompt || !expiresAt || !instanceId) throw new Error('invalid continuation effect payload');
    const created = await deps.runtimeStore!.createContinuation({
      token, prompt, expiresAt, instanceId, tenantId: effect.tenantId, subjectId: effect.subjectId, payload: payload as JsonValue,
    });
    // Duplicate delivery means the continuation was already safely created.
    if (!created) return;
    return;
  }
  if (effect.kind !== 'reply.send') throw new Error(`no runtime outbox handler for ${effect.kind}`);
  const payload = object(effect.payload);
  const channel = string(payload.channel);
  const to = string(payload.to);
  const text = string(payload.text);
  if (!channel || !to || !text) throw new Error('invalid reply effect payload');
  if (channel === 'web') return; // Web sockets receive the committed reply synchronously.
  if (channel === 'whatsapp' && deps.whatsappSender) {
    await deps.whatsappSender.send(to, { type: 'text', text });
    return;
  }
  if (deps.sdkBridge.hasSendCapability()) {
    // SDK adapters receive the durable effect identity and must use it as their provider-side
    // idempotency key. This is the only safe way to reconcile an ambiguous timeout after send.
    const result = await deps.sdkBridge.sendViaChannel(channel, to, text, { aelio_effect_id: effect.effectId });
    if (result.ok) return;
    throw new Error(result.error ?? 'SDK channel delivery failed');
  }
  throw new Error(`no runtime delivery adapter for ${channel}`);
}

function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('effect payload must be an object');
  return value as Record<string, unknown>;
}
function string(value: unknown): string { return typeof value === 'string' ? value : ''; }
function number(value: unknown): number { return typeof value === 'number' && Number.isFinite(value) ? value : 0; }
