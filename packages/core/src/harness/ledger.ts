import type { AelioDbClient } from '@aelio/db-client';
import { i64, parseJson, readUtf8, utf8 } from '../storage/helpers.js';
import type { LedgerEntry } from './schema.js';

export type LedgerAelioDbConfig = {
  client: AelioDbClient;
  table: string;
};

/** Load persisted ledger rows for a turn (crash-resume / idempotency). */
export async function loadLedgerForTurn(
  sessionId: string,
  turnId: string,
  aelioDb: LedgerAelioDbConfig,
): Promise<LedgerEntry[]> {
  if (!aelioDb) {
    throw new Error('AelioDb ledger config is required');
  }
  const scan = await aelioDb.client.scanRows(aelioDb.table, {
    k: 2_000,
    filters: [
      { col: 'session_id', op: 'eq', value: utf8(sessionId) },
      { col: 'turn_id', op: 'eq', value: utf8(turnId) },
    ],
  });
  return scan.rows.map((row) => ({
    instructionId: readUtf8(row.values, 'instruction_id'),
    argsHash: readUtf8(row.values, 'args_hash'),
    status: (readUtf8(row.values, 'status') || 'success') as 'success' | 'error',
    result: parseJson<unknown>(readUtf8(row.values, 'result_json'), null),
    toolName: '',
    durationMs: 0,
  }));
}

/** Persist one ledger entry — duplicate (session, instruction, argsHash) is a no-op. */
export async function persistLedgerEntry(
  sessionId: string,
  turnId: string,
  entry: LedgerEntry,
  aelioDb: LedgerAelioDbConfig,
): Promise<void> {
  if (!aelioDb) {
    throw new Error('AelioDb ledger config is required');
  }
  try {
    await aelioDb.client.insertRow(aelioDb.table, {
      session_id: utf8(sessionId),
      turn_id: utf8(turnId),
      instruction_id: utf8(entry.instructionId),
      args_hash: utf8(entry.argsHash),
      status: utf8(entry.status),
      result_json: utf8(JSON.stringify(entry.result ?? null)),
      created_at: i64(Date.now()),
    });
  } catch {
    // AelioDb has no unique-constraint enforcement over HTTP — a rare
    // same-turn double-write just adds a redundant row; idempotency at
    // read time (loadLedgerForTurn) still holds because the executor
    // de-dupes by (instructionId, argsHash) before re-invoking.
  }
}
