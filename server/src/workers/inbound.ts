import {
  claimJob,
  completeJob,
  enqueueJob,
  failJob,
  processTurn,
  requeueStaleJobs,
  resolveWhatsAppIdentity,
} from '@aelio/core';
import { buildTurnInput } from '../turn-options.js';
import type { RuntimeDeps } from '../runtime-deps.js';

const WORKER_ID = 'inbound-worker';

export function startInboundWorker(deps: RuntimeDeps) {
  const interval = setInterval(() => {
    void (async () => {
      await requeueStaleJobs(deps.jobStore);

      const job = await claimJob('inbound', WORKER_ID, deps.jobStore);
      if (!job) {
        return;
      }

      try {
        const channel = job.payload.channel as string;
        const text = job.payload.text as string;
        const from = job.payload.from as string;

        if (!text || !from) {
          throw new Error('Inbound job missing text or from');
        }

        const identity =
          channel === 'whatsapp'
            ? resolveWhatsAppIdentity(from)
            : { externalId: from, channelAddress: from };

        const { reply } = await processTurn(
          buildTurnInput(deps, {
            customerExternalId: identity.externalId,
            channel,
            channelAddress: identity.channelAddress,
            message: text,
          }),
        );

        await enqueueJob(
          'outbound',
          {
            channel,
            to: from,
            text: reply,
          },
          deps.jobStore,
        );

        await completeJob(job.id, deps.jobStore);
      } catch (error) {
        await failJob(
          job.id,
          error instanceof Error ? error.message : 'Inbound worker failed',
          deps.jobStore,
        );
      }
    })();
  }, 250);

  interval.unref();
  return () => clearInterval(interval);
}
