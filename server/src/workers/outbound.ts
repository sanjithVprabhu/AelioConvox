import { claimJob, completeJob, failJob } from '@aelio/core/edge';
import type { RuntimeDeps } from '../runtime-deps.js';
import type { FastifyBaseLogger } from 'fastify';

const WORKER_ID = 'outbound-worker';

export async function deliverOutboundPayload(
  deps: RuntimeDeps,
  payload: Record<string, unknown>,
): Promise<void> {
  const channel = payload.channel as string;
  const to = payload.to as string;
  const text = payload.text as string;
  const content = (payload.content as { text?: string; type?: string } | undefined) ?? { text };

  if (!channel || !to || (!text && !content.text)) {
    throw new Error('Outbound job missing channel, recipient, or text content');
  }
  if (channel === 'whatsapp' && deps.whatsappSender) {
    await deps.whatsappSender.send(
      to,
      content.text ? { type: 'text', text: content.text } : (payload.content as never),
    );
  } else if (deps.sdkBridge.hasSendCapability()) {
    const result = await deps.sdkBridge.sendViaChannel(channel, to, content.text ?? text);
    if (!result.ok) {
      throw new Error(result.error ?? 'SDK channel delivery failed');
    }
  } else if (channel !== 'web') {
    throw new Error(`No delivery method configured for channel "${channel}"`);
  }
}

export function startOutboundWorker(deps: RuntimeDeps, logger: FastifyBaseLogger) {
  let polling = false;
  const interval = setInterval(() => {
    if (polling) return;
    polling = true;
    void (async () => {
      try {
        const job = await claimJob('outbound', WORKER_ID, deps.jobStore);
        if (!job) {
          return;
        }

        try {
          await deliverOutboundPayload(deps, job.payload);
          await completeJob(job.id, deps.jobStore);
        } catch (error) {
          logger.error(
            { err: error, jobId: job.id, queue: job.queue },
            'Outbound delivery job failed',
          );
          await failJob(
            job.id,
            error instanceof Error ? error.message : 'Outbound worker failed',
            deps.jobStore,
          );
        }
      } catch (error) {
        logger.error({ err: error }, 'Outbound worker polling failed');
      } finally {
        polling = false;
      }
    })();
  }, 250);

  interval.unref();
  return () => clearInterval(interval);
}
