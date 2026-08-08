import { createHash, randomUUID } from 'node:crypto';
import type { ApiValue, RowValues, SunjetClient } from '@aelio/sunjet-client';
import type { AelioStorageTableNames } from './types.js';

const utf8 = (value: string): ApiValue => ({ type: 'utf8', value });
const i64 = (value: number): ApiValue => ({ type: 'i64', value });

/** The singleton row every replica competes for before touching the schema. */
const LOCK_ID = 'schema:lock';
const DEFAULT_LEASE_MS = 60_000;

export type AelioMigration = {
  /** Stable identity. Never reuse one for different content — the checksum guard depends on it. */
  id: string;
  /** Monotonic schema version this migration brings the database to. */
  version: number;
  description: string;
  /**
   * Additive migrations only add tables or nullable columns, so an older build keeps working
   * against a newer schema. Anything else must be declared destructive and is refused unless the
   * operator explicitly allows it — that is the expand/contract protocol, not a suggestion.
   */
  kind: 'additive' | 'destructive';
  apply(client: SunjetClient, tables: AelioStorageTableNames): Promise<void>;
};

export type MigrationRecord = {
  rowId: number;
  migrationId: string;
  version: number;
  status: 'applied' | 'lock';
  checksum: string;
  appliedAt: number;
  leaseOwner: string;
  leaseExpiresAt: number;
  revision: number;
};

export type MigrationOutcome = {
  applied: AelioMigration[];
  skipped: AelioMigration[];
  schemaVersion: number;
};

export function migrationsTableSchema(): Array<{ name: string; kind: 'utf8' | 'i64' }> {
  return [
    { name: 'migration_id', kind: 'utf8' }, { name: 'version', kind: 'i64' },
    { name: 'status', kind: 'utf8' }, { name: 'checksum', kind: 'utf8' },
    { name: 'applied_at', kind: 'i64' }, { name: 'lease_owner', kind: 'utf8' },
    { name: 'lease_expires_at', kind: 'i64' }, { name: 'revision', kind: 'i64' },
    { name: 'description', kind: 'utf8' },
  ];
}

/** Content hash of what a migration claims to be, so a silently edited migration is caught. */
export function migrationChecksum(migration: AelioMigration): string {
  return createHash('sha256')
    .update(`${migration.id}|${migration.version}|${migration.kind}|${migration.description}`)
    .digest('hex')
    .slice(0, 32);
}

export class MigrationLockedError extends Error {
  constructor(owner: string, expiresAt: number) {
    super(`another replica holds the Aelio DB migration lock (owner ${owner}, expires ${new Date(expiresAt).toISOString()})`);
    this.name = 'MigrationLockedError';
  }
}

/**
 * The versioned schema migration runner.
 *
 * Three properties matter and each is enforced rather than documented:
 *
 *  1. **One migrator at a time.** A CAS lease on a singleton row means two replicas booting
 *     together cannot both mutate the catalog. An expired lease is reclaimable, so a replica that
 *     died mid-migration does not wedge the deployment forever.
 *  2. **A migration is applied at most once, ever.** Completion is recorded in the same database
 *     it migrated, keyed by migration id.
 *  3. **A rolled-back deploy cannot corrupt data.** If the database records a schema version this
 *     build does not know about, boot fails closed instead of writing rows an older schema will
 *     misread.
 */
export class AelioMigrationRunner {
  constructor(
    private readonly client: SunjetClient,
    private readonly tables: AelioStorageTableNames,
    private readonly owner = `${process.pid}:${randomUUID()}`,
  ) {}

  /**
   * Apply every migration this build knows about that the database has not seen.
   *
   * `allowDestructive` must be set explicitly for any non-additive step; the default refuses,
   * because a destructive migration cannot be rolled back by redeploying the previous build.
   */
  async migrate(
    migrations: AelioMigration[],
    options: { allowDestructive?: boolean; leaseMs?: number } = {},
  ): Promise<MigrationOutcome> {
    const ordered = [...migrations].sort((a, b) => a.version - b.version);
    assertMigrationsAreWellFormed(ordered);

    await this.client.ensureTable(this.tables.migrations, migrationsTableSchema());
    const lease = await this.acquireLease(options.leaseMs ?? DEFAULT_LEASE_MS);
    try {
      const records = await this.listRecords();
      const applied = new Map(records.filter((row) => row.status === 'applied').map((row) => [row.migrationId, row]));
      this.assertNoUnknownFutureSchema(ordered, records);

      const appliedNow: AelioMigration[] = [];
      const skipped: AelioMigration[] = [];
      for (const migration of ordered) {
        const record = applied.get(migration.id);
        if (record) {
          const checksum = migrationChecksum(migration);
          if (record.checksum !== checksum) {
            throw new Error(
              `migration '${migration.id}' was already applied with a different definition ` +
                `(recorded ${record.checksum}, this build has ${checksum}). Add a new migration instead of editing an applied one.`,
            );
          }
          skipped.push(migration);
          continue;
        }
        if (migration.kind === 'destructive' && !options.allowDestructive) {
          throw new Error(
            `migration '${migration.id}' is destructive and destructive migrations are not enabled. ` +
              'Run the expand phase first, verify, then re-run with allowDestructive.',
          );
        }
        await migration.apply(this.client, this.tables);
        await this.recordApplied(migration);
        appliedNow.push(migration);
      }

      const schemaVersion = ordered.reduce((max, migration) => Math.max(max, migration.version), 0);
      return { applied: appliedNow, skipped, schemaVersion };
    } finally {
      await this.releaseLease(lease);
    }
  }

  async schemaVersion(): Promise<number> {
    const records = await this.listRecords();
    return records.filter((row) => row.status === 'applied').reduce((max, row) => Math.max(max, row.version), 0);
  }

  async listRecords(): Promise<MigrationRecord[]> {
    const rows = await this.client.scanRows(this.tables.migrations, { k: 512 });
    return rows.rows.map((row) => parseRecord(row.row_id, row.values));
  }

  /**
   * A version recorded in the database but absent from this build means an older binary is
   * booting against a newer schema. Continuing would write rows the newer schema expects to be
   * shaped differently, so this fails closed.
   */
  private assertNoUnknownFutureSchema(migrations: AelioMigration[], records: MigrationRecord[]): void {
    const known = new Set(migrations.map((migration) => migration.id));
    const unknown = records.filter((row) => row.status === 'applied' && !known.has(row.migrationId));
    if (unknown.length === 0) return;
    const detail = unknown.map((row) => `${row.migrationId}@v${row.version}`).join(', ');
    throw new Error(
      `Aelio DB has migrations this build does not know about (${detail}). ` +
        'This binary is older than the database; deploy the matching or newer build.',
    );
  }

  private async acquireLease(leaseMs: number): Promise<MigrationRecord> {
    const now = Date.now();
    const expiresAt = now + leaseMs;
    const existing = (await this.listRecords()).find((row) => row.migrationId === LOCK_ID);

    if (!existing) {
      const result = await this.client.transact(
        [{
          op: 'insert', table: this.tables.migrations,
          values: lockValues(this.owner, expiresAt, 1),
        }],
        [{ kind: 'absent', table: this.tables.migrations, equals: { migration_id: utf8(LOCK_ID) } }],
      );
      if (!result.applied) throw new MigrationLockedError('unknown', expiresAt);
      const created = (await this.listRecords()).find((row) => row.migrationId === LOCK_ID);
      if (!created) throw new Error('migration lock disappeared immediately after being taken');
      return created;
    }

    // Only an unheld or expired lease may be taken, and taking it is a CAS on the revision, so
    // two replicas reclaiming the same expired lease cannot both win.
    if (existing.leaseOwner && existing.leaseExpiresAt > now) {
      throw new MigrationLockedError(existing.leaseOwner, existing.leaseExpiresAt);
    }
    const result = await this.client.transact(
      [{
        op: 'update', table: this.tables.migrations, row_id: existing.rowId,
        values: lockValues(this.owner, expiresAt, existing.revision + 1),
      }],
      [{
        kind: 'row_matches', table: this.tables.migrations, row_id: existing.rowId,
        equals: { revision: i64(existing.revision) },
      }],
    );
    if (!result.applied) throw new MigrationLockedError(existing.leaseOwner || 'unknown', existing.leaseExpiresAt);
    return { ...existing, leaseOwner: this.owner, leaseExpiresAt: expiresAt, revision: existing.revision + 1 };
  }

  private async releaseLease(lease: MigrationRecord): Promise<void> {
    await this.client.transact(
      [{
        op: 'update', table: this.tables.migrations, row_id: lease.rowId,
        values: lockValues('', 0, lease.revision + 1),
      }],
      [{
        kind: 'row_matches', table: this.tables.migrations, row_id: lease.rowId,
        equals: { revision: i64(lease.revision), lease_owner: utf8(this.owner) },
      }],
    );
  }

  private async recordApplied(migration: AelioMigration): Promise<void> {
    const result = await this.client.transact(
      [{
        op: 'insert', table: this.tables.migrations,
        values: {
          migration_id: utf8(migration.id), version: i64(migration.version), status: utf8('applied'),
          checksum: utf8(migrationChecksum(migration)), applied_at: i64(Date.now()),
          lease_owner: utf8(''), lease_expires_at: i64(0), revision: i64(1),
          description: utf8(migration.description),
        },
      }],
      [{ kind: 'absent', table: this.tables.migrations, equals: { migration_id: utf8(migration.id) } }],
    );
    if (!result.applied) {
      throw new Error(`migration '${migration.id}' was recorded concurrently; another migrator holds the lock`);
    }
  }
}

function assertMigrationsAreWellFormed(migrations: AelioMigration[]): void {
  const ids = new Set<string>();
  const versions = new Set<number>();
  for (const migration of migrations) {
    if (migration.id === LOCK_ID) throw new Error(`'${LOCK_ID}' is reserved for the migration lock`);
    if (ids.has(migration.id)) throw new Error(`duplicate migration id '${migration.id}'`);
    if (versions.has(migration.version)) throw new Error(`duplicate migration version ${migration.version}`);
    if (!Number.isInteger(migration.version) || migration.version < 1) {
      throw new Error(`migration '${migration.id}' has an invalid version`);
    }
    ids.add(migration.id);
    versions.add(migration.version);
  }
}

function lockValues(owner: string, expiresAt: number, revision: number): RowValues {
  return {
    migration_id: utf8(LOCK_ID), version: i64(0), status: utf8('lock'), checksum: utf8(''),
    applied_at: i64(Date.now()), lease_owner: utf8(owner), lease_expires_at: i64(expiresAt),
    revision: i64(revision), description: utf8('Aelio DB schema migration lock'),
  };
}

function parseRecord(rowId: number, values: RowValues): MigrationRecord {
  const read = (key: string): string => {
    const value = values[key];
    return value?.type === 'utf8' ? value.value : '';
  };
  const readNumber = (key: string): number => {
    const value = values[key];
    return value?.type === 'i64' ? value.value : 0;
  };
  return {
    rowId,
    migrationId: read('migration_id'),
    version: readNumber('version'),
    status: read('status') === 'lock' ? 'lock' : 'applied',
    checksum: read('checksum'),
    appliedAt: readNumber('applied_at'),
    leaseOwner: read('lease_owner'),
    leaseExpiresAt: readNumber('lease_expires_at'),
    revision: readNumber('revision'),
  };
}
