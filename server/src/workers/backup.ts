import type { RuntimeDeps } from '../runtime-deps.js';

/**
 * Aelio DB segment storage is the sole durability plane for this
 * AelioDb-only build — there is no SQLite file to back up. This worker is a
 * permanent no-op, kept only so `app.ts` has a stable start/stop hook.
 */
export function startBackupWorker(_deps: RuntimeDeps) {
  return () => {};
}
