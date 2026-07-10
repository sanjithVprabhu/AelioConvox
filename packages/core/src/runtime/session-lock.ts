/**
 * Per-session serialization. A widget user commonly fires a second message
 * while the first turn is still executing (tools in flight, a plan suspended);
 * without ordering, two turns race over the same intent stack, suspension
 * record, and lifecycle state. This chains work per session key so turns run
 * one at a time, in arrival order.
 *
 * In-process is the correct scope for the single-container deployment. A
 * multi-instance deployment would move this to a durable per-session queue
 * (job-queue `turn:<sessionId>`); the call sites don't change.
 */
const chains = new Map<string, Promise<unknown>>();

export function withSessionLock<T>(key: string, task: () => Promise<T>): Promise<T> {
  const prior = chains.get(key) ?? Promise.resolve();
  // Swallow the predecessor's rejection so one failed turn never poisons the
  // queue for the next; each task still surfaces its own result/rejection.
  const run = prior.catch(() => undefined).then(task);
  chains.set(key, run);
  // Clean up the map entry once this is the tail, to avoid unbounded growth.
  void run.finally(() => {
    if (chains.get(key) === run) {
      chains.delete(key);
    }
  });
  return run;
}
