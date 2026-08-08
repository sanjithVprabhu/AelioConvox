import type { FastifyInstance } from 'fastify';
import { z } from 'zod';
import { processAelioRuntimeMessage } from '../aelio-runtime.js';
import { requireSecret } from '../auth.js';
import type { RuntimeDeps } from '../runtime-deps.js';

const RuntimeEventRequest = z.object({
  tenant_id: z.string().min(1),
  subject_id: z.string().min(1),
  idempotency_key: z.string().min(1),
  message: z.string().min(1).max(16_384),
  channel: z.enum(['web', 'whatsapp', 'sdk']).default('web'),
  /** Ask for the reply to be delivered asynchronously through the durable outbox. */
  delivery: z.object({ channel: z.string().min(1), to: z.string().min(1) }).optional(),
});

/**
 * The authoritative Aelio Runtime ingress. It is independent of SQLite, and it is a
 * system-to-system endpoint: it runs model and tool work on a caller-named subject, so it is
 * guarded by the shared secret and rejects a tenant other than this deployment's.
 */
export async function registerRuntimeRoutes(app: FastifyInstance, deps: RuntimeDeps) {
  app.post('/v1/runtime/events', async (request, reply) => {
    if (!requireSecret(request, reply, deps.config.secret)) return reply;
    if (!deps.runtimeStore) {
      return reply.status(503).send({ error: 'Aelio Runtime requires Aelio DB storage' });
    }
    const parsed = RuntimeEventRequest.safeParse(request.body);
    if (!parsed.success) return reply.status(400).send({ error: parsed.error.flatten() });
    const input = parsed.data;
    // The tenant is a property of the deployment, not of the request body. Accepting an arbitrary
    // tenant id here would let one authenticated caller read and mutate another tenant's subjects.
    if (input.tenant_id !== deps.config.name) {
      return reply.status(403).send({ error: 'tenant_id does not match this Aelio deployment' });
    }

    const outcome = await processAelioRuntimeMessage(deps, {
      tenantId: input.tenant_id,
      subjectId: input.subject_id,
      idempotencyKey: input.idempotency_key,
      message: input.message,
      channel: input.channel,
      ...(input.delivery ? { delivery: input.delivery } : {}),
    });

    if (outcome.status === 'duplicate') return reply.status(200).send({ status: 'duplicate' });
    if (outcome.status === 'conflict') {
      return reply.status(409).send({ error: 'subject state changed; retry with the same idempotency key' });
    }
    return reply.status(200).send({
      status: 'committed',
      event_id: outcome.eventId,
      commit_lsn: outcome.commitLsn,
      reply: outcome.reply,
      awaiting_confirmation: outcome.awaitingConfirmation,
    });
  });
}
