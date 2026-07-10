import type { AelioDatabase } from '@aelio/db';
import { harnessLedger } from '@aelio/db';
import { and, eq } from 'drizzle-orm';
import { randomUUID } from 'node:crypto';
import type { LedgerEntry } from './schema.js';

/** Load persisted ledger rows for a turn (crash-resume / idempotency). */
export async function loadLedgerForTurn(
  database: AelioDatabase,
  sessionId: string,
  turnId: string,
): Promise<LedgerEntry[]> {
  const rows = await database.db
    .select()
    .from(harnessLedger)
    .where(and(eq(harnessLedger.sessionId, sessionId), eq(harnessLedger.turnId, turnId)));

  return rows.map((row) => ({
    instructionId: row.instructionId,
    argsHash: row.argsHash,
    status: row.status as 'success' | 'error',
    result: row.result,
    toolName: '',
    durationMs: 0,
  }));
}

/** Persist one ledger entry — duplicate (session, instruction, argsHash) is a no-op. */
export async function persistLedgerEntry(
  database: AelioDatabase,
  sessionId: string,
  turnId: string,
  entry: LedgerEntry,
): Promise<void> {
  try {
    await database.db.insert(harnessLedger).values({
      id: randomUUID(),
      sessionId,
      turnId,
      instructionId: entry.instructionId,
      argsHash: entry.argsHash,
      status: entry.status,
      result: entry.result,
      createdAt: new Date(),
    });
  } catch {
    // Unique index on (session_id, instruction_id, args_hash) — already recorded.
  }
}
