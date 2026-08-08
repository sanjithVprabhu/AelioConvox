import type { FastifyInstance } from 'fastify';
import { z } from 'zod';
import { readSubjectState, type JsonValue } from '@aelio/core';
import { requireSecret } from '../auth.js';
import type { RuntimeDeps } from '../runtime-deps.js';

const CancelBody = z.object({ reason: z.string().min(1).max(500) });

/**
 * The operator surface: inspect a subject, replay a turn's decision chain, read queue health, and
 * cancel a stuck instance.
 *
 * Every route is authenticated with the shared secret and scoped to this deployment's tenant.
 * Payloads are summarised rather than dumped: an admin view is a common place for customer content
 * and secrets to leak into logs and screenshots, so message bodies are reduced to lengths and
 * effect payloads to their shape.
 */
export async function registerAdminRuntimeRoutes(app: FastifyInstance, deps: RuntimeDeps) {
  const tenantId = deps.config.name;

  const guard = (request: Parameters<typeof requireSecret>[0], reply: Parameters<typeof requireSecret>[1]) => {
    if (!requireSecret(request, reply, deps.config.secret)) return false;
    if (!deps.runtimeStore) {
      void reply.status(503).send({ error: 'Aelio Runtime requires Aelio DB storage' });
      return false;
    }
    return true;
  };

  app.get('/api/v1/admin/runtime/health', async (request, reply) => {
    if (!guard(request, reply)) return reply;
    const health = await deps.runtimeStore!.health();
    // `degraded` is the single signal worth alerting on: work is queued but not moving, or an
    // effect's outcome is unknown and nobody has reconciled it.
    const degraded =
      health.outbox.unknown > 0 ||
      health.outbox.failed > 0 ||
      health.outbox.oldestPendingAgeMs > 60_000 ||
      health.instances.failed > 0;
    return reply.status(200).send({ status: degraded ? 'degraded' : 'ok', tenant_id: tenantId, ...health });
  });

  app.get('/api/v1/admin/runtime/subjects/:subjectId', async (request, reply) => {
    if (!guard(request, reply)) return reply;
    const { subjectId } = request.params as { subjectId: string };
    const snapshot = await deps.runtimeStore!.loadSnapshot(tenantId, subjectId);
    if (!snapshot) return reply.status(404).send({ error: 'unknown subject' });
    const state = readSubjectState(snapshot.state);
    const instances = await deps.runtimeStore!.listSubjectInstances(tenantId, subjectId);
    const parked = await deps.runtimeSuspensionStore?.get(`${tenantId}:${subjectId}`);
    return reply.status(200).send({
      tenant_id: tenantId,
      subject_id: subjectId,
      revision: snapshot.revision,
      updated_at: snapshot.updatedAt,
      lifecycle_state: state.lifecycleState,
      lifecycle_reason: state.lifecycleReason,
      flow_progress: state.flowProgress,
      turns: state.turns,
      last_channel: state.lastChannel,
      // Redacted: shape and size, never the words a customer wrote.
      history: state.history.map((entry) => ({ role: entry.role, at: entry.at, length: entry.content.length })),
      profile_fields: Object.keys(state.profile),
      parked_plan: parked ? { reason: parked.reason, expires_at: parked.expiresAt } : null,
      instances: instances.map((instance) => ({
        instance_id: instance.instanceId,
        artifact: `${instance.artifactId}@${instance.artifactVersion}`,
        parent_instance_id: instance.parentInstanceId ?? null,
        status: instance.status,
        revision: instance.revision,
        updated_at: instance.updatedAt,
      })),
    });
  });

  app.get('/api/v1/admin/runtime/turns/:eventId', async (request, reply) => {
    if (!guard(request, reply)) return reply;
    const { eventId } = request.params as { eventId: string };
    const entries = await deps.runtimeStore!.listLedgerForEvent(tenantId, eventId);
    if (entries.length === 0) return reply.status(404).send({ error: 'unknown turn' });
    return reply.status(200).send({
      tenant_id: tenantId,
      event_id: eventId,
      subject_id: entries[0]!.subjectId,
      entries: entries.map((entry) => ({
        record_id: entry.recordId,
        instance_id: entry.instanceId,
        kind: entry.kind,
        created_at: entry.createdAt,
        payload: summarize(entry.payload),
      })),
    });
  });

  app.post('/api/v1/admin/runtime/instances/:instanceId/cancel', async (request, reply) => {
    if (!guard(request, reply)) return reply;
    const { instanceId } = request.params as { instanceId: string };
    const parsed = CancelBody.safeParse(request.body);
    if (!parsed.success) return reply.status(400).send({ error: parsed.error.flatten() });

    const outcome = await deps.runtimeStore!.cancelInstance(tenantId, instanceId, parsed.data.reason);
    app.log.warn({ instanceId, tenantId, reason: parsed.data.reason, outcome }, 'Operator cancelled a runtime instance');
    if (outcome === 'not_found') return reply.status(404).send({ error: 'unknown instance' });
    if (outcome === 'already_terminal') return reply.status(409).send({ error: 'instance already reached a terminal status' });
    if (outcome === 'conflict') return reply.status(409).send({ error: 'instance changed while cancelling; retry' });
    return reply.status(200).send({ status: 'cancelled', instance_id: instanceId });
  });

  app.get('/api/v1/admin/runtime/effects/unknown', async (request, reply) => {
    if (!guard(request, reply)) return reply;
    const effects = await deps.runtimeStore!.listUnknownEffects(64);
    return reply.status(200).send({
      tenant_id: tenantId,
      // These are the reconciliation queue: an effect that may or may not have reached a provider.
      effects: effects.map((effect) => ({
        effect_id: effect.effectId,
        subject_id: effect.subjectId,
        kind: effect.kind,
        attempts: effect.attempts,
        last_error: effect.lastError ?? null,
        payload: summarize(effect.payload),
      })),
    });
  });

  app.post('/api/v1/admin/runtime/effects/:effectId/reconcile', async (request, reply) => {
    if (!guard(request, reply)) return reply;
    const { effectId } = request.params as { effectId: string };
    const parsed = z
      .object({ outcome: z.enum(['delivered', 'failed']), note: z.string().min(1).max(500) })
      .safeParse(request.body);
    if (!parsed.success) return reply.status(400).send({ error: parsed.error.flatten() });

    const effect = (await deps.runtimeStore!.listUnknownEffects(256)).find((entry) => entry.effectId === effectId);
    if (!effect) return reply.status(404).send({ error: 'no unknown effect with that id' });
    const resolved = await deps.runtimeStore!.resolveUnknownEffect(effect, parsed.data.outcome, parsed.data.note);
    app.log.warn({ effectId, tenantId, outcome: parsed.data.outcome }, 'Operator reconciled an unknown runtime effect');
    if (!resolved) return reply.status(409).send({ error: 'effect changed while reconciling; retry' });
    return reply.status(200).send({ status: parsed.data.outcome, effect_id: effectId });
  });
}

/**
 * Reduce a stored payload to its shape. Keys and scalar kinds are what an operator needs to
 * diagnose a turn; the values are customer content and are deliberately not returned.
 */
function summarize(payload: JsonValue): JsonValue {
  if (payload === null || typeof payload === 'boolean' || typeof payload === 'number') return payload;
  if (typeof payload === 'string') return `string(${payload.length})`;
  if (Array.isArray(payload)) return { array: payload.length };
  const summarized: Record<string, JsonValue> = {};
  for (const [key, value] of Object.entries(payload)) {
    // One level of nesting is enough to see a decision's shape without unrolling a whole plan.
    if (value === null || typeof value === 'boolean' || typeof value === 'number') {
      summarized[key] = value;
    } else if (typeof value === 'string') {
      summarized[key] = `string(${value.length})`;
    } else if (Array.isArray(value)) {
      summarized[key] = { array: value.length };
    } else {
      summarized[key] = { object: Object.keys(value).length };
    }
  }
  return summarized;
}
