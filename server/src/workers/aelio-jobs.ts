import type { FastifyBaseLogger } from 'fastify';
import type { RuntimeDeps } from '../runtime-deps.js';
import { deliverOutboundPayload } from './outbound.js';

const WORKER_ID = `aelio-edge-${process.pid}`;

/** Executes edge-only effects selected and durably leased by the Rust runtime. */
export function startAelioJobWorker(deps: RuntimeDeps, logger: FastifyBaseLogger) {
  let polling = false;
  const interval = setInterval(() => {
    if (polling) return;
    polling = true;
    void (async () => {
      try {
        const leases = await deps.aelioRuntime.leaseAgentJobs(
          WORKER_ID,
          'proactive_delivery',
        );
        for (const job of leases) {
          try {
            const payload = job.payload;
            if (!payload || typeof payload !== 'object' || Array.isArray(payload)) {
              throw new Error('Rust proactive job payload is not an object');
            }
            await deliverOutboundPayload(deps, payload as Record<string, unknown>);
            await deps.aelioRuntime.completeAgentJob(WORKER_ID, job.id);
          } catch (error) {
            const reason = error instanceof Error ? error.message : 'Aelio edge job failed';
            logger.error({ err: error, jobId: job.id }, 'Aelio edge job failed');
            await deps.aelioRuntime.failAgentJob(WORKER_ID, job.id, reason);
          }
        }
      } catch (error) {
        logger.error({ err: error }, 'Aelio Rust job polling failed');
      } finally {
        polling = false;
      }
    })();
  }, 250);
  interval.unref();
  return () => clearInterval(interval);
}
