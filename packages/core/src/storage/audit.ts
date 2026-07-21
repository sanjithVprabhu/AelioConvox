import { randomUUID } from 'node:crypto';
import { bool, i64, parseJson, readBool, readI64, readUtf8, utf8 } from './helpers.js';
import type { SunjetStorageConfig } from './types.js';

const SCAN_CAP = 2_000;

export type FunctionCallLogInput = {
  sessionId: string;
  customerId: string;
  functionName: string;
  args: Record<string, unknown>;
  result?: unknown;
  status: 'success' | 'error' | 'timeout' | 'blocked' | 'pending';
  safetyLevel: string;
  requiredConfirmation?: boolean;
  confirmed?: boolean;
  durationMs?: number;
  errorMessage?: string;
};

export type FunctionCallRecord = {
  id: string;
  sessionId: string;
  customerId: string;
  functionName: string;
  args: Record<string, unknown>;
  result: unknown;
  status: string;
  safetyLevel: string;
  requiredConfirmation: boolean;
  confirmed: boolean;
  durationMs: number;
  errorMessage: string | null;
  createdAt: number;
};

export class ConvoxFunctionCallStore {
  readonly client: SunjetStorageConfig['client'];
  readonly table: string;

  constructor(config: SunjetStorageConfig) {
    this.client = config.client;
    this.table = config.tables.functionCalls;
  }

  async log(input: FunctionCallLogInput): Promise<{ callId: string }> {
    const callId = randomUUID();
    await this.client.insertRow(this.table, {
      call_id: utf8(callId),
      session_id: utf8(input.sessionId),
      customer_id: utf8(input.customerId),
      function_name: utf8(input.functionName),
      args_json: utf8(JSON.stringify(input.args ?? {})),
      result_json: utf8(input.result === undefined ? '' : JSON.stringify(input.result)),
      status: utf8(input.status),
      safety_level: utf8(input.safetyLevel),
      required_confirmation: bool(input.requiredConfirmation ?? false),
      confirmed: bool(input.confirmed ?? false),
      duration_ms: i64(input.durationMs ?? 0),
      error_message: utf8(input.errorMessage ?? ''),
      created_at: i64(Date.now()),
    });
    return { callId };
  }

  async listBySession(sessionId: string): Promise<FunctionCallRecord[]> {
    const scan = await this.client.scanRows(this.table, {
      k: SCAN_CAP,
      filters: [{ col: 'session_id', op: 'eq', value: utf8(sessionId) }],
    });

    return scan.rows
      .map((row) => {
        const errorMessage = readUtf8(row.values, 'error_message');
        return {
          id: readUtf8(row.values, 'call_id'),
          sessionId: readUtf8(row.values, 'session_id'),
          customerId: readUtf8(row.values, 'customer_id'),
          functionName: readUtf8(row.values, 'function_name'),
          args: parseJson(readUtf8(row.values, 'args_json'), {}),
          result: parseJson<unknown>(readUtf8(row.values, 'result_json'), null),
          status: readUtf8(row.values, 'status'),
          safetyLevel: readUtf8(row.values, 'safety_level'),
          requiredConfirmation: readBool(row.values, 'required_confirmation'),
          confirmed: readBool(row.values, 'confirmed'),
          durationMs: readI64(row.values, 'duration_ms'),
          errorMessage: errorMessage.length > 0 ? errorMessage : null,
          createdAt: readI64(row.values, 'created_at'),
        };
      })
      .sort((a, b) => a.createdAt - b.createdAt);
  }
}

export function createConvoxFunctionCallStore(
  config: SunjetStorageConfig,
): ConvoxFunctionCallStore {
  return new ConvoxFunctionCallStore(config);
}
