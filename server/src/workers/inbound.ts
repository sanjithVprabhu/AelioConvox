import {
  claimJob,
  completeJob,
  enqueueJob,
  failJob,
  requeueStaleJobs,
  resolveWhatsAppIdentity,
} from '@aelio/core/edge';
import { executeConversationTurn } from '../conversation-turn.js';
import type { RuntimeDeps } from '../runtime-deps.js';
import type { FastifyBaseLogger } from 'fastify';

const WORKER_ID = 'inbound-worker';

export function startInboundWorker(deps: RuntimeDeps, logger: FastifyBaseLogger) {
  let polling = false;
  const interval = setInterval(() => {
    if (polling) return;
    polling = true;
    void (async () => {
      try {
        await requeueStaleJobs(deps.jobStore);

        const job = await claimJob('inbound', WORKER_ID, deps.jobStore);
        if (!job) {
          return;
        }

        try {
          const channel = job.payload.channel as string;
          const text = job.payload.text as string;
          const from = job.payload.from as string;
          const sourceTurnId = job.payload.messageId as string | undefined;

          if (!text || !from) {
            throw new Error('Inbound job missing text or from');
          }

          const identity =
            channel === 'whatsapp'
              ? resolveWhatsAppIdentity(from)
              : { externalId: from, channelAddress: from };

          const { reply } = await executeConversationTurn(deps, {
            customerExternalId: identity.externalId,
            channel,
            channelAddress: identity.channelAddress,
            message: text,
            sourceTurnId,
          });

          await enqueueJob('outbound', {
            channel,
            to: from,
            text: reply,
            sourceTurnId,
          }, deps.jobStore);

          await completeJob(job.id, deps.jobStore);
        } catch (error) {
          logger.error(
            { err: error, jobId: job.id, queue: job.queue },
            'Inbound conversation job failed',
          );
          await failJob(
            job.id,
            error instanceof Error ? error.message : 'Inbound worker failed',
            deps.jobStore,
          );
        }
      } catch (error) {
        logger.error({ err: error }, 'Inbound worker polling failed');
      } finally {
        polling = false;
      }
    })();
  }, 250);

  interval.unref();
  return () => clearInterval(interval);
}
