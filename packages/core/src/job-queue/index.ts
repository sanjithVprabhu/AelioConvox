import type { ConvoxJobStore } from '../storage/jobs.js';

export type JobRecord = {
  id: string;
  queue: string;
  payload: Record<string, unknown>;
};

function requireJobStore(jobStore: ConvoxJobStore | undefined): ConvoxJobStore {
  if (!jobStore) {
    throw new Error('AelioDb jobStore is required');
  }
  return jobStore;
}

export async function enqueueJob(
  queue: string,
  payload: Record<string, unknown>,
  jobStore: ConvoxJobStore,
): Promise<string> {
  return requireJobStore(jobStore).enqueue(queue, payload);
}

export async function claimJob(
  queue: string,
  workerId: string,
  jobStore: ConvoxJobStore,
): Promise<JobRecord | null> {
  return requireJobStore(jobStore).claim(queue, workerId);
}

export async function completeJob(jobId: string, jobStore: ConvoxJobStore): Promise<void> {
  await requireJobStore(jobStore).complete(jobId);
}

/** Reclaim jobs stuck in `processing` longer than `staleAfterMs` (CON-001). */
export async function requeueStaleJobs(
  jobStore: ConvoxJobStore,
  staleAfterMs = 5 * 60_000,
): Promise<number> {
  return requireJobStore(jobStore).requeueStale(staleAfterMs);
}

export async function failJob(
  jobId: string,
  errorMessage: string,
  jobStore: ConvoxJobStore,
  retryInMs = 5000,
): Promise<void> {
  await requireJobStore(jobStore).fail(jobId, errorMessage, retryInMs);
}
