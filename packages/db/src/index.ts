import Database from 'better-sqlite3';
import type { BetterSQLite3Database } from 'drizzle-orm/better-sqlite3';
import { drizzle } from 'drizzle-orm/better-sqlite3';
import { migrate } from 'drizzle-orm/better-sqlite3/migrator';
import { mkdirSync } from 'node:fs';
import { dirname } from 'node:path';
import { load as loadSqliteVec } from 'sqlite-vec';
import * as schema from './schema.js';

export * from './schema.js';

const DEFAULT_VECTOR_DIMENSIONS = 1536;

export type CreateDatabaseOptions = {
  /** Must match the embedding provider output dimension (e.g. 1536 openai, 768 gemini). */
  vectorDimensions?: number;
};

export type MemoryVectorSearchRow = {
  memoryId: string;
  category: string | null;
  distance: number;
};

export type AelioDatabase = {
  db: BetterSQLite3Database<typeof schema>;
  sqlite: Database.Database;
  vectorEnabled: boolean;
  vectorDimensions: number;
  migrate: (migrationsFolder: string) => void;
  upsertMemoryVector: (input: {
    memoryId: string;
    customerId: string;
    category: string | null;
    embedding: number[];
  }) => void;
  /**
   * Atomically claim an inbound message id. Returns true the first time an id
   * is seen, false on webhook redelivery — callers drop duplicates.
   */
  claimInboundMessage: (messageId: string) => boolean;
  searchMemoryVectors: (input: {
    customerId: string;
    embedding: number[];
    limit: number;
  }) => MemoryVectorSearchRow[];
  close: () => void;
};

function jsonEmbedding(vector: number[]): string {
  return JSON.stringify(vector);
}

function normalizeEmbedding(input: string | number[], dimensions: number): number[] {
  const parsed = Array.isArray(input) ? input : (JSON.parse(input) as unknown);
  const values = Array.isArray(parsed)
    ? parsed.map((value) => (typeof value === 'number' && Number.isFinite(value) ? value : 0))
    : [];

  if (values.length === dimensions) {
    return values;
  }

  if (values.length > dimensions) {
    return values.slice(0, dimensions);
  }

  return [...values, ...new Array<number>(dimensions - values.length).fill(0)];
}

function normalizeVecId(value: number | bigint): bigint {
  return typeof value === 'bigint' ? value : BigInt(Math.trunc(value));
}

function readStoredVectorDimensions(sqlite: Database.Database): number | null {
  try {
    const row = sqlite
      .prepare("SELECT value FROM aelio_meta WHERE key = 'vector_dimensions'")
      .get() as { value: string } | undefined;
    if (!row) {
      return null;
    }
    const parsed = Number.parseInt(row.value, 10);
    return Number.isFinite(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

function writeStoredVectorDimensions(sqlite: Database.Database, dimensions: number): void {
  sqlite.exec(`
    CREATE TABLE IF NOT EXISTS aelio_meta (
      key TEXT PRIMARY KEY NOT NULL,
      value TEXT NOT NULL
    );
  `);
  sqlite
    .prepare(
      `INSERT INTO aelio_meta (key, value) VALUES ('vector_dimensions', ?)
       ON CONFLICT(key) DO UPDATE SET value = excluded.value`,
    )
    .run(String(dimensions));
}

function initializeVectorStore(
  sqlite: Database.Database,
  dimensions: number,
): boolean {
  try {
    loadSqliteVec(sqlite);
    sqlite.exec(`
      CREATE TABLE IF NOT EXISTS memory_vec_index (
        memory_id TEXT PRIMARY KEY NOT NULL,
        vec_id INTEGER NOT NULL UNIQUE
      );
    `);

    const previous = readStoredVectorDimensions(sqlite);
    if (previous != null && previous !== dimensions) {
      console.warn(
        `[aelio] Embedding dimension changed (${previous} → ${dimensions}); recreating memory_vec index`,
      );
      sqlite.exec('DROP TABLE IF EXISTS memory_vec');
      sqlite.exec('DELETE FROM memory_vec_index');
    }

    sqlite.exec(`
      CREATE VIRTUAL TABLE IF NOT EXISTS memory_vec USING vec0(
        vec_id INTEGER,
        memory_id TEXT,
        customer_id TEXT PARTITION KEY,
        category TEXT,
        embedding FLOAT[${dimensions}]
      );
    `);
    writeStoredVectorDimensions(sqlite, dimensions);
    return true;
  } catch (error) {
    console.warn(
      '[aelio] sqlite-vec failed to load — memory recall using brute-force fallback:',
      error instanceof Error ? error.message : String(error),
    );
    return false;
  }
}

function syncExistingMemoryRows(
  sqlite: Database.Database,
  vectorEnabled: boolean,
  dimensions: number,
): void {
  if (!vectorEnabled) {
    return;
  }

  const rows = sqlite
    .prepare('SELECT id, customer_id, category, embedding FROM memory WHERE embedding IS NOT NULL')
    .all() as Array<{
    id: string;
    customer_id: string;
    category: string | null;
    embedding: string;
  }>;

  const getVecId = sqlite.prepare(
    'SELECT vec_id FROM memory_vec_index WHERE memory_id = ?',
  );
  const nextVecId = sqlite.prepare(
    'SELECT COALESCE(MAX(vec_id), 0) + 1 AS next_id FROM memory_vec_index',
  );
  const insertIndex = sqlite.prepare(
    'INSERT INTO memory_vec_index (memory_id, vec_id) VALUES (?, ?)',
  );
  const deleteVector = sqlite.prepare('DELETE FROM memory_vec WHERE memory_id = ?');
  const insertVector = sqlite.prepare(
    'INSERT INTO memory_vec (vec_id, memory_id, customer_id, category, embedding) VALUES (?, ?, ?, ?, ?)',
  );

  const tx = sqlite.transaction((items: typeof rows) => {
    for (const row of items) {
      const existing = getVecId.get(row.id) as { vec_id: number | bigint } | undefined;
      const vecId = normalizeVecId(
        existing?.vec_id ?? (nextVecId.get() as { next_id: number | bigint }).next_id,
      );
      if (!existing) {
        insertIndex.run(row.id, vecId);
      }
      deleteVector.run(row.id);
      insertVector.run(
        vecId,
        row.id,
        row.customer_id,
        row.category,
        jsonEmbedding(normalizeEmbedding(row.embedding, dimensions)),
      );
    }
  });

  tx(rows);
}

export function createDatabase(
  databasePath: string,
  options: CreateDatabaseOptions = {},
): AelioDatabase {
  const vectorDimensions = options.vectorDimensions ?? DEFAULT_VECTOR_DIMENSIONS;
  mkdirSync(dirname(databasePath), { recursive: true });

  const sqlite = new Database(databasePath);
  sqlite.pragma('journal_mode = WAL');
  sqlite.pragma('foreign_keys = ON');
  const vectorEnabled = initializeVectorStore(sqlite, vectorDimensions);

  const db = drizzle(sqlite, { schema });
  const getVecId = vectorEnabled
    ? sqlite.prepare('SELECT vec_id FROM memory_vec_index WHERE memory_id = ?')
    : null;
  const nextVecId = vectorEnabled
    ? sqlite.prepare('SELECT COALESCE(MAX(vec_id), 0) + 1 AS next_id FROM memory_vec_index')
    : null;
  const insertIndex = vectorEnabled
    ? sqlite.prepare('INSERT INTO memory_vec_index (memory_id, vec_id) VALUES (?, ?)')
    : null;
  const deleteVector = vectorEnabled
    ? sqlite.prepare('DELETE FROM memory_vec WHERE memory_id = ?')
    : null;
  const insertVector = vectorEnabled
    ? sqlite.prepare(
        'INSERT INTO memory_vec (vec_id, memory_id, customer_id, category, embedding) VALUES (?, ?, ?, ?, ?)',
      )
    : null;
  const searchVectors = vectorEnabled
    ? sqlite.prepare(
        'SELECT memory_id AS memoryId, category, distance FROM memory_vec WHERE customer_id = ? AND embedding MATCH ? AND k = ?',
      )
    : null;

  return {
    db,
    sqlite,
    vectorEnabled,
    vectorDimensions,
    migrate(migrationsFolder: string) {
      migrate(db, { migrationsFolder });
      // Additive columns for existing deployments (SQLite has no IF NOT EXISTS
      // for columns; inspect the table instead).
      const turnCallCols = sqlite
        .prepare("SELECT name FROM pragma_table_info('turn_api_calls')")
        .all() as Array<{ name: string }>;
      const colNames = new Set(turnCallCols.map((c) => c.name));
      if (colNames.size > 0 && !colNames.has('tokens_in')) {
        sqlite.exec('ALTER TABLE turn_api_calls ADD COLUMN tokens_in INTEGER');
        sqlite.exec('ALTER TABLE turn_api_calls ADD COLUMN tokens_out INTEGER');
      }
      syncExistingMemoryRows(sqlite, vectorEnabled, vectorDimensions);
    },
    upsertMemoryVector(input) {
      if (!vectorEnabled) {
        return;
      }

      const tx = sqlite.transaction(() => {
        const existing = getVecId!.get(input.memoryId) as { vec_id: number | bigint } | undefined;
        const vecId = normalizeVecId(
          existing?.vec_id ?? (nextVecId!.get() as { next_id: number | bigint }).next_id,
        );
        if (!existing) {
          insertIndex!.run(input.memoryId, vecId);
        }
        deleteVector!.run(input.memoryId);
        insertVector!.run(
          vecId,
          input.memoryId,
          input.customerId,
          input.category,
          jsonEmbedding(normalizeEmbedding(input.embedding, vectorDimensions)),
        );
      });

      tx();
    },
    claimInboundMessage(messageId: string): boolean {
      const inserted = sqlite
        .prepare('INSERT OR IGNORE INTO inbound_dedup (message_id, created_at) VALUES (?, ?)')
        .run(messageId, Date.now());
      // Opportunistic prune (~1% of claims): entries older than 7 days.
      if (Math.random() < 0.01) {
        sqlite
          .prepare('DELETE FROM inbound_dedup WHERE created_at < ?')
          .run(Date.now() - 7 * 24 * 60 * 60 * 1000);
      }
      return inserted.changes === 1;
    },
    searchMemoryVectors(input) {
      if (!vectorEnabled) {
        return [];
      }

      return searchVectors!.all(
        input.customerId,
        jsonEmbedding(normalizeEmbedding(input.embedding, vectorDimensions)),
        input.limit,
      ) as MemoryVectorSearchRow[];
    },
    close() {
      sqlite.close();
    },
  };
}
