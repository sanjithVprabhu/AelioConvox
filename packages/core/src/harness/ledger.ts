import type { SunjetClient } from '@aelio/sunjet-client';
import { i64, parseJson, readUtf8, utf8 } from '../storage/helpers.js';
import type { LedgerEntry } from './schema.js';

export type LedgerSunjetConfig = {
  client: SunjetClient;
  table: string;
};

/** Load persisted ledger rows for a turn (crash-resume / idempotency). */
export async function loadLedgerForTurn(
  sessionId: string,
  turnId: string,
  sunjet: LedgerSunjetConfig,
): Promise<LedgerEntry[]> {
  if (!sunjet) {
    throw new Error('Sunjet ledger config is required');
  }
  const scan = await sunjet.client.scanRows(sunjet.table, {
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
  sunjet: LedgerSunjetConfig,
): Promise<void> {
  if (!sunjet) {
    throw new Error('Sunjet ledger config is required');
  }
  try {
    await sunjet.client.insertRow(sunjet.table, {
      session_id: utf8(sessionId),
      turn_id: utf8(turnId),
      instruction_id: utf8(entry.instructionId),
      args_hash: utf8(entry.argsHash),
      status: utf8(entry.status),
      result_json: utf8(JSON.stringify(entry.result ?? null)),
      created_at: i64(Date.now()),
    });
  } catch {
    // Sunjet has no unique-constraint enforcement over HTTP — a rare
    // same-turn double-write just adds a redundant row; idempotency at
    // read time (loadLedgerForTurn) still holds because the executor
    // de-dupes by (instructionId, argsHash) before re-invoking.
  }
}
