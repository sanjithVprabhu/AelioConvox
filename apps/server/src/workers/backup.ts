import { mkdirSync, readdirSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import type { RuntimeDeps } from '../runtime-deps.js';

function escapeSqlitePath(value: string): string {
  return value.replace(/'/g, "''");
}

function runBackup(deps: RuntimeDeps): void {
  const backupConfig = deps.config.storage.backup;
  if (!backupConfig.enabled) {
    return;
  }

  const databasePath = deps.config.storage.database_path;
  const backupDir = join(dirname(databasePath), 'backups');
  mkdirSync(backupDir, { recursive: true });

  const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
  const target = join(backupDir, `aelio-${timestamp}.sqlite`);

  deps.database.sqlite.pragma('wal_checkpoint(TRUNCATE)');
  deps.database.sqlite.exec(`VACUUM INTO '${escapeSqlitePath(target)}'`);

  const backups = readdirSync(backupDir)
    .filter((name) => name.endsWith('.sqlite'))
    .sort()
    .reverse();

  for (const stale of backups.slice(backupConfig.retain_count)) {
    rmSync(join(backupDir, stale), { force: true });
  }
}

export function startBackupWorker(deps: RuntimeDeps) {
  if (!deps.config.storage.backup.enabled) {
    return () => {};
  }

  const intervalMs = deps.config.storage.backup.interval_hours * 60 * 60 * 1000;
  const timer = setInterval(() => {
    try {
      runBackup(deps);
    } catch (error) {
      deps.database.sqlite.pragma('optimize');
      deps.database.sqlite.exec('PRAGMA wal_checkpoint(PASSIVE)');
      console.error('Backup worker failed', error);
    }
  }, intervalMs);

  timer.unref();
  return () => clearInterval(timer);
}
