import { setProactiveOptIn } from '@aelio/core/edge';
import type { FastifyInstance, FastifyRequest } from 'fastify';
import { createHash } from 'node:crypto';
import { z } from 'zod';
import { stableAgentUserId } from '../conversation-turn.js';
import type { RuntimeDeps } from '../runtime-deps.js';

const SendSchema = z.object({
  customerExternalId: z.string().min(1),
  channel: z.string().min(1),
  content: z.string().min(1),
  templateName: z.string().optional(),
  dedupKey: z.string().optional(),
});

const OptInSchema = z.object({
  customerExternalId: z.string().min(1),
  optIn: z.boolean(),
});

// The dev's backend triggers proactive messages on its own domain events, so
// these endpoints are authenticated with the same SDK secret.
function authorized(request: FastifyRequest, secret: string): boolean {
  const header = request.headers.authorization;
  const bearer = header?.startsWith('Bearer ') ? header.slice('Bearer '.length) : undefined;
  const provided = bearer ?? (request.headers['x-aelio-secret'] as string | undefined);
  return provided === secret;
}

export async function registerProactiveRoutes(app: FastifyInstance, deps: RuntimeDeps) {
  if (!deps.config.proactive.enabled) {
    return;
  }

  app.post('/proactive/opt-in', async (request, reply) => {
    if (!authorized(request, deps.config.secret)) {
      return reply.status(401).send({ error: 'Unauthorized' });
    }
    const parsed = OptInSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.status(400).send({ error: 'Invalid request body' });
    }
    const ok = await setProactiveOptIn(
      parsed.data.customerExternalId,
      parsed.data.optIn,
      deps.customerStore,
    );
    if (!ok) {
      return reply.status(404).send({ error: 'Unknown customer' });
    }
    return { ok: true, customerExternalId: parsed.data.customerExternalId, optIn: parsed.data.optIn };
  });

  app.post('/proactive', async (request, reply) => {
    if (!authorized(request, deps.config.secret)) {
      return reply.status(401).send({ error: 'Unauthorized' });
    }
    const parsed = SendSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.status(400).send({ error: 'Invalid request body' });
    }

    const customer = await deps.customerStore.getByExternalId(parsed.data.customerExternalId);
    if (!customer) {
      return reply.status(404).send({ ok: false, status: 'blocked', reason: 'Unknown customer' });
    }
    const address = await deps.customerStore.getChannelAddress(
      customer.id,
      parsed.data.channel,
    );
    if (!address) {
      return reply.status(422).send({
        ok: false,
        status: 'blocked',
        reason: `No ${parsed.data.channel} address for customer`,
      });
    }

    let channelGate = true;
    if (parsed.data.channel === 'whatsapp' && !parsed.data.templateName) {
      const lastInboundAt = await deps.messageStore.lastUserMessageAt(customer.id);
      channelGate =
        lastInboundAt != null &&
        Date.now() - lastInboundAt <= deps.config.proactive.window_hours * 60 * 60_000;
    }
    const optedIn =
      !deps.config.proactive.require_opt_in || customer.metadata?.proactiveOptIn === true;
    const fingerprint =
      parsed.data.dedupKey ??
      createHash('sha256')
        .update(
          `${parsed.data.customerExternalId}\u001f${parsed.data.channel}\u001f` +
          `${parsed.data.content}\u001f${parsed.data.templateName ?? ''}`,
        )
        .digest('hex');
    const decision = await deps.aelioRuntime.evaluateProactive({
      candidate: {
        user_id: stableAgentUserId(parsed.data.customerExternalId),
        fingerprint,
        payload: {
          channel: parsed.data.channel,
          to: address,
          text: parsed.data.content,
          proactive: true,
          ...(parsed.data.templateName ? { templateName: parsed.data.templateName } : {}),
        },
        opted_in: optedIn,
        deterministic_gate: channelGate,
        confidence_millis: 1_000,
        min_confidence_millis: 1_000,
      },
      policy: {
        min_cadence_ms: deps.config.proactive.min_cadence_minutes * 60_000,
        max_enqueues_per_day: deps.config.proactive.max_per_customer_per_day,
        suppression_ms: deps.config.proactive.suppression_minutes * 60_000,
        max_job_attempts: 5,
      },
    });
    if ('suppressed' in decision) {
      return reply.status(200).send({
        ok: false,
        status: 'blocked',
        reason: decision.suppressed.reason,
      });
    }
    const jobId =
      'enqueued' in decision
        ? decision.enqueued.job_id
        : decision.already_enqueued.job_id;
    return reply.status('enqueued' in decision ? 202 : 200).send({
      ok: 'enqueued' in decision,
      status: 'enqueued' in decision ? 'accepted' : 'duplicate',
      jobId,
    });
  });
}
