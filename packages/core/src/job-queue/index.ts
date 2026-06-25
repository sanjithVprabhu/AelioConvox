import type { AelioDatabase } from '@aelio/db';
import { jobQueue } from '@aelio/db';
import { eq } from 'drizzle-orm';
import { randomUUID } from 'node:crypto';

export type JobRecord = {
  id: string;
  queue: string;
  payload: Record<string, unknown>;
};

export async function enqueueJob(
  database: AelioDatabase,
  queue: string,
  payload: Record<string, unknown>,
): Promise<string> {
  const id = randomUUID();
  const now = new Date();
  await database.db.insert(jobQueue).values({
    id,
    queue,
    payload,
    status: 'pending',
    attempts: 0,
    maxAttempts: 5,
    nextRunAt: now,
    createdAt: now,
  });
  return id;
}

export function claimJob(
  database: AelioDatabase,
  queue: string,
  workerId: string,
): JobRecord | null {
  const now = Date.now();
  const row = database.sqlite
    .prepare(
      `UPDATE job_queue
       SET status = 'processing', locked_by = ?, locked_at = ?, attempts = attempts + 1
       WHERE id = (
         SELECT id FROM job_queue
         WHERE queue = ? AND status = 'pending' AND next_run_at <= ?
         ORDER BY next_run_at ASC
         LIMIT 1
       )
       RETURNING id, queue, payload`,
    )
    .get(workerId, now, queue, now) as { id: string; queue: string; payload: string } | undefined;

  if (!row) {
    return null;
  }

  return {
    id: row.id,
    queue: row.queue,
    payload: JSON.parse(row.payload) as Record<string, unknown>,
  };
}

export async function completeJob(database: AelioDatabase, jobId: string): Promise<void> {
  await database.db
    .update(jobQueue)
    .set({
      status: 'done',
      completedAt: new Date(),
      lockedBy: null,
      lockedAt: null,
    })
    .where(eq(jobQueue.id, jobId));
}

export async function failJob(
  database: AelioDatabase,
  jobId: string,
  errorMessage: string,
  retryInMs = 5000,
): Promise<void> {
  const existing = await database.db
    .select()
    .from(jobQueue)
    .where(eq(jobQueue.id, jobId))
    .limit(1);

  const job = existing[0];
  if (!job) {
    return;
  }

  const attempts = job.attempts ?? 1;
  const maxAttempts = job.maxAttempts ?? 5;
  const shouldRetry = attempts < maxAttempts;

  await database.db
    .update(jobQueue)
    .set({
      status: shouldRetry ? 'pending' : 'failed',
      errorMessage,
      nextRunAt: new Date(Date.now() + retryInMs),
      lockedBy: null,
      lockedAt: null,
    })
    .where(eq(jobQueue.id, jobId));
}