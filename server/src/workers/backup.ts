import type { RuntimeDeps } from '../runtime-deps.js';

/**
 * Sunjet/Astrolobe segment storage is the sole durability plane for this
 * Sunjet-only build — there is no SQLite file to back up. This worker is a
 * permanent no-op, kept only so `app.ts` has a stable start/stop hook.
 */
export function startBackupWorker(_deps: RuntimeDeps) {
  return () => {};
}
