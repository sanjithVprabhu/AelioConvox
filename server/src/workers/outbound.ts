import { claimJob, completeJob, failJob } from '@aelio/core';
import type { RuntimeDeps } from '../runtime-deps.js';

const WORKER_ID = 'outbound-worker';

export function startOutboundWorker(deps: RuntimeDeps) {
  const interval = setInterval(() => {
    void (async () => {
      const job = claimJob(deps.database, 'outbound', WORKER_ID);
      if (!job) {
        return;
      }

      try {
        const channel = job.payload.channel as string;
        const to = job.payload.to as string;
        const text = job.payload.text as string;
        const content = (job.payload.content as { text?: string; type?: string } | undefined) ?? {
          text,
        };

        if (!to || (!text && !content)) {
          throw new Error('Outbound job missing to or content');
        }

        if (channel === 'whatsapp' && deps.whatsappSender) {
          // Built-in Meta/mock adapter.
          await deps.whatsappSender.send(
            to,
            content.text ? { type: 'text', text: content.text } : (job.payload.content as never),
          );
        } else if (deps.sdkBridge.hasSendCapability()) {
          // Bring-your-own provider (any channel): deliver via the SDK onSend handler.
          const result = await deps.sdkBridge.sendViaChannel(channel, to, content.text ?? text);
          if (!result.ok) {
            throw new Error(result.error ?? 'SDK channel delivery failed');
          }
        } else if (channel === 'web') {
          // Web replies are delivered inline on the widget socket — nothing to do here.
        } else {
          throw new Error(`No delivery method configured for channel "${channel}"`);
        }

        await completeJob(deps.database, job.id);
      } catch (error) {
        await failJob(
          deps.database,
          job.id,
          error instanceof Error ? error.message : 'Outbound worker failed',
        );
      }
    })();
  }, 250);

  interval.unref();
  return () => clearInterval(interval);
}
