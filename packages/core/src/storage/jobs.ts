import { randomUUID } from 'node:crypto';
import { i64, parseJson, readI64, readUtf8, utf8 } from './helpers.js';
import type { AelioDbStorageConfig } from './types.js';

const SCAN_CAP = 2_000;
const DEFAULT_MAX_ATTEMPTS = 5;

export type ClaimedJob = {
  id: string;
  queue: string;
  payload: Record<string, unknown>;
};

/**
 * AelioDb has no atomic conditional update over HTTP, so `claim()` serializes
 * through this in-process mutex and re-reads the candidate row before
 * flipping it to `processing`. This only protects against races between
 * claimers in the same process — multiple processes still need queue-level
 * sharding or a single worker per queue.
 */
let claimMutex: Promise<unknown> = Promise.resolve();

function withClaimLock<T>(fn: () => Promise<T>): Promise<T> {
  const run = claimMutex.then(fn, fn);
  claimMutex = run.then(
    () => undefined,
    () => undefined,
  );
  return run;
}

export class ConvoxJobStore {
  readonly client: AelioDbStorageConfig['client'];
  readonly table: string;

  constructor(config: AelioDbStorageConfig) {
    this.client = config.client;
    this.table = config.tables.jobQueue;
  }

  async enqueue(queue: string, payload: Record<string, unknown>): Promise<string> {
    const id = randomUUID();
    const now = Date.now();
    await this.client.insertRow(this.table, {
      job_id: utf8(id),
      queue: utf8(queue),
      payload: utf8(JSON.stringify(payload)),
      status: utf8('pending'),
      attempts: i64(0),
      max_attempts: i64(DEFAULT_MAX_ATTEMPTS),
      next_run_at: i64(now),
      locked_by: utf8(''),
      locked_at: i64(0),
      error_message: utf8(''),
      created_at: i64(now),
      completed_at: i64(0),
    });
    return id;
  }

  async claim(queue: string, workerId: string): Promise<ClaimedJob | null> {
    return withClaimLock(async () => {
      const now = Date.now();
      const scan = await this.client.scanRows(this.table, {
        k: SCAN_CAP,
        filters: [
          { col: 'queue', op: 'eq', value: utf8(queue) },
          { col: 'status', op: 'eq', value: utf8('pending') },
          { col: 'next_run_at', op: 'le', value: i64(now) },
        ],
      });
      if (scan.rows.length === 0) {
        return null;
      }

      const candidate = scan.rows.reduce((a, b) =>
        readI64(a.values, 'next_run_at') <= readI64(b.values, 'next_run_at') ? a : b,
      );

      // Re-read before mutating: guards against a same-process double-claim
      // between the scan above and this update.
      const fresh = await this.client.getRow(this.table, candidate.row_id);
      if (readUtf8(fresh.values, 'status') !== 'pending') {
        return null;
      }

      const attempts = readI64(fresh.values, 'attempts') + 1;
      await this.client.updateRow(this.table, candidate.row_id, {
        status: utf8('processing'),
        locked_by: utf8(workerId),
        locked_at: i64(now),
        attempts: i64(attempts),
      });

      return {
        id: readUtf8(fresh.values, 'job_id'),
        queue: readUtf8(fresh.values, 'queue'),
        payload: parseJson(readUtf8(fresh.values, 'payload'), {}),
      };
    });
  }

  private async findByJobId(jobId: string) {
    const scan = await this.client.scanRows(this.table, {
      k: 1,
      filters: [{ col: 'job_id', op: 'eq', value: utf8(jobId) }],
    });
    return scan.rows[0] ?? null;
  }

  async complete(jobId: string): Promise<void> {
    const row = await this.findByJobId(jobId);
    if (!row) {
      return;
    }
    await this.client.updateRow(this.table, row.row_id, {
      status: utf8('done'),
      completed_at: i64(Date.now()),
      locked_by: utf8(''),
      locked_at: i64(0),
    });
  }

  async fail(jobId: string, errorMessage: string, retryInMs = 5_000): Promise<void> {
    const row = await this.findByJobId(jobId);
    if (!row) {
      return;
    }
    const attempts = readI64(row.values, 'attempts');
    const maxAttempts = readI64(row.values, 'max_attempts') || DEFAULT_MAX_ATTEMPTS;
    const shouldRetry = attempts < maxAttempts;

    await this.client.updateRow(this.table, row.row_id, {
      status: utf8(shouldRetry ? 'pending' : 'failed'),
      error_message: utf8(errorMessage),
      next_run_at: i64(Date.now() + retryInMs),
      locked_by: utf8(''),
      locked_at: i64(0),
    });
  }

  /** Reclaim jobs stuck in `processing` longer than `staleAfterMs`. */
  async requeueStale(staleAfterMs = 5 * 60_000): Promise<number> {
    const cutoff = Date.now() - staleAfterMs;
    const scan = await this.client.scanRows(this.table, {
      k: SCAN_CAP,
      filters: [
        { col: 'status', op: 'eq', value: utf8('processing') },
        { col: 'locked_at', op: 'gt', value: i64(0) },
        { col: 'locked_at', op: 'lt', value: i64(cutoff) },
      ],
    });

    for (const row of scan.rows) {
      await this.client.updateRow(this.table, row.row_id, {
        status: utf8('pending'),
        locked_by: utf8(''),
        locked_at: i64(0),
      });
    }

    return scan.rows.length;
  }
}

export function createConvoxJobStore(config: AelioDbStorageConfig): ConvoxJobStore {
  return new ConvoxJobStore(config);
}
