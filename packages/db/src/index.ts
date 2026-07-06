import Database from 'better-sqlite3';
import type { BetterSQLite3Database } from 'drizzle-orm/better-sqlite3';
import { drizzle } from 'drizzle-orm/better-sqlite3';
import { migrate } from 'drizzle-orm/better-sqlite3/migrator';
import { mkdirSync } from 'node:fs';
import { dirname } from 'node:path';
import { load as loadSqliteVec } from 'sqlite-vec';
import * as schema from './schema.js';

export * from './schema.js';

const VECTOR_DIMENSIONS = 1536;

export type MemoryVectorSearchRow = {
  memoryId: string;
  category: string | null;
  distance: number;
};

export type AelioDatabase = {
  db: BetterSQLite3Database<typeof schema>;
  sqlite: Database.Database;
  vectorEnabled: boolean;
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

function normalizeEmbedding(input: string | number[]): number[] {
  const parsed = Array.isArray(input) ? input : (JSON.parse(input) as unknown);
  const values = Array.isArray(parsed)
    ? parsed.map((value) => (typeof value === 'number' && Number.isFinite(value) ? value : 0))
    : [];

  if (values.length === VECTOR_DIMENSIONS) {
    return values;
  }

  if (values.length > VECTOR_DIMENSIONS) {
    return values.slice(0, VECTOR_DIMENSIONS);
  }

  return [...values, ...new Array<number>(VECTOR_DIMENSIONS - values.length).fill(0)];
}

function normalizeVecId(value: number | bigint): bigint {
  return typeof value === 'bigint' ? value : BigInt(Math.trunc(value));
}

function initializeVectorStore(sqlite: Database.Database): boolean {
  try {
    loadSqliteVec(sqlite);
    sqlite.exec(`
      CREATE TABLE IF NOT EXISTS memory_vec_index (
        memory_id TEXT PRIMARY KEY NOT NULL,
        vec_id INTEGER NOT NULL UNIQUE
      );
    `);
    sqlite.exec(`
      CREATE VIRTUAL TABLE IF NOT EXISTS memory_vec USING vec0(
        vec_id INTEGER,
        memory_id TEXT,
        customer_id TEXT PARTITION KEY,
        category TEXT,
        embedding FLOAT[${VECTOR_DIMENSIONS}]
      );
    `);
    return true;
  } catch {
    return false;
  }
}

function syncExistingMemoryRows(sqlite: Database.Database, vectorEnabled: boolean): void {
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
        jsonEmbedding(normalizeEmbedding(row.embedding)),
      );
    }
  });

  tx(rows);
}

export function createDatabase(databasePath: string): AelioDatabase {
  mkdirSync(dirname(databasePath), { recursive: true });

  const sqlite = new Database(databasePath);
  sqlite.pragma('journal_mode = WAL');
  sqlite.pragma('foreign_keys = ON');
  const vectorEnabled = initializeVectorStore(sqlite);

  // The reflections table is created here (not via a migration) so the daemon's
  // self-evaluation store is always present — same approach as the vector store.
  sqlite.exec(`
    CREATE TABLE IF NOT EXISTS reflections (
      id TEXT PRIMARY KEY NOT NULL,
      session_id TEXT NOT NULL,
      customer_id TEXT NOT NULL,
      outcome TEXT NOT NULL,
      score REAL,
      summary TEXT,
      issues TEXT,
      created_at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_reflections_session ON reflections(session_id);
    CREATE INDEX IF NOT EXISTS idx_reflections_customer ON reflections(customer_id, created_at);
  `);

  sqlite.exec(`
    CREATE TABLE IF NOT EXISTS proactive_messages (
      id TEXT PRIMARY KEY NOT NULL,
      customer_id TEXT NOT NULL,
      channel TEXT NOT NULL,
      to_address TEXT NOT NULL,
      content TEXT NOT NULL,
      dedup_key TEXT,
      status TEXT NOT NULL,
      reason TEXT,
      created_at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_proactive_customer ON proactive_messages(customer_id, created_at);
  `);

  // Channel webhooks retry (Meta redelivers on slow ACKs); processing the same
  // inbound message twice double-spends LLM calls and can re-run tools. Dedup by
  // provider message id.
  sqlite.exec(`
    CREATE TABLE IF NOT EXISTS inbound_dedup (
      message_id TEXT PRIMARY KEY NOT NULL,
      created_at INTEGER NOT NULL
    );
  `);

  sqlite.exec(`
    CREATE TABLE IF NOT EXISTS response_cache (
      id TEXT PRIMARY KEY NOT NULL,
      customer_id TEXT NOT NULL,
      query TEXT NOT NULL,
      embedding TEXT,
      reply TEXT NOT NULL,
      hits INTEGER DEFAULT 0,
      created_at INTEGER NOT NULL,
      expires_at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_respcache_customer ON response_cache(customer_id, expires_at);
  `);

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
      syncExistingMemoryRows(sqlite, vectorEnabled);
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
          jsonEmbedding(input.embedding),
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
        jsonEmbedding(input.embedding),
        input.limit,
      ) as MemoryVectorSearchRow[];
    },
    close() {
      sqlite.close();
    },
  };
}
