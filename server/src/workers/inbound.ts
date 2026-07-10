import {
  claimJob,
  completeJob,
  enqueueCustomerTurn,
  enqueueJob,
  failJob,
  processTurn,
  requeueStaleJobs,
  resolveWhatsAppIdentity,
  updateJobPayload,
} from '@aelio/core';
import { buildTurnInput } from '../turn-options.js';
import type { RuntimeDeps } from '../runtime-deps.js';

const WORKER_ID = 'inbound-worker';
let staleSweepCounter = 0;

async function processInboundJob(deps: RuntimeDeps, job: { id: string; payload: Record<string, unknown> }) {
  const channel = job.payload.channel as string;
  const text = job.payload.text as string;
  const from = job.payload.from as string;

  if (!text || !from) {
    throw new Error('Inbound job missing text or from');
  }

  if (job.payload.turnCompleted === true) {
    if (!job.payload.outboundEnqueued) {
      const reply = job.payload.reply as string;
      await enqueueJob(deps.database, 'outbound', {
        channel,
        to: from,
        text: reply,
      });
      await updateJobPayload(deps.database, job.id, { outboundEnqueued: true });
    }
    await completeJob(deps.database, job.id);
    return;
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

  await updateJobPayload(deps.database, job.id, { turnCompleted: true, reply });
  await enqueueJob(deps.database, 'outbound', {
    channel,
    to: from,
    text: reply,
  });
  await updateJobPayload(deps.database, job.id, { outboundEnqueued: true });
  await completeJob(deps.database, job.id);
}

export function startInboundWorker(deps: RuntimeDeps) {
  const interval = setInterval(() => {
    staleSweepCounter += 1;
    if (staleSweepCounter % 40 === 0) {
      requeueStaleJobs(deps.database);
    }

    const job = claimJob(deps.database, 'inbound', WORKER_ID);
    if (!job) {
      return;
    }

    const channel = job.payload.channel as string;
    const from = job.payload.from as string;
    const customerKey = `${channel}:${from}`;

    enqueueCustomerTurn(customerKey, async () => {
      try {
        await processInboundJob(deps, job);
      } catch (error) {
        await failJob(
          deps.database,
          job.id,
          error instanceof Error ? error.message : 'Inbound worker failed',
        );
      }
    });
  }, 250);

  interval.unref();
  return () => clearInterval(interval);
}
