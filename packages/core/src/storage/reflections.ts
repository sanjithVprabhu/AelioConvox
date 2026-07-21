import { randomUUID } from 'node:crypto';
import { f64, i64, parseJson, readF64, readI64, readUtf8, utf8 } from './helpers.js';
import type { SunjetStorageConfig } from './types.js';

export type ReflectionStoreInput = {
  sessionId: string;
  customerId: string;
  outcome: 'resolved' | 'unresolved' | 'unclear';
  score: number;
  summary: string;
  issues: string[];
  insight?: string;
  followup?: string;
};

export type ReflectionRecord = {
  id: string;
  sessionId: string;
  customerId: string;
  outcome: string;
  score: number;
  summary: string;
  issues: string[];
  insight: string | null;
  followup: string | null;
  createdAt: number;
};

export class ConvoxReflectionStore {
  readonly client: SunjetStorageConfig['client'];
  readonly table: string;

  constructor(config: SunjetStorageConfig) {
    this.client = config.client;
    this.table = config.tables.reflections;
  }

  async store(input: ReflectionStoreInput): Promise<{ reflectionId: string }> {
    const reflectionId = randomUUID();
    await this.client.insertRow(this.table, {
      reflection_id: utf8(reflectionId),
      session_id: utf8(input.sessionId),
      customer_id: utf8(input.customerId),
      outcome: utf8(input.outcome),
      score: f64(input.score),
      summary: utf8(input.summary),
      issues_json: utf8(JSON.stringify(input.issues ?? [])),
      insight: utf8(input.insight ?? ''),
      followup: utf8(input.followup ?? ''),
      created_at: i64(Date.now()),
    });
    return { reflectionId };
  }

  async hasForSession(sessionId: string): Promise<boolean> {
    const scan = await this.client.scanRows(this.table, {
      k: 1,
      filters: [{ col: 'session_id', op: 'eq', value: utf8(sessionId) }],
    });
    return scan.rows.length > 0;
  }

  async listBySession(sessionId: string): Promise<ReflectionRecord[]> {
    const scan = await this.client.scanRows(this.table, {
      k: 100,
      filters: [{ col: 'session_id', op: 'eq', value: utf8(sessionId) }],
    });

    return scan.rows
      .map((row) => {
        const insight = readUtf8(row.values, 'insight');
        const followup = readUtf8(row.values, 'followup');
        return {
          id: readUtf8(row.values, 'reflection_id'),
          sessionId: readUtf8(row.values, 'session_id'),
          customerId: readUtf8(row.values, 'customer_id'),
          outcome: readUtf8(row.values, 'outcome'),
          score: readF64(row.values, 'score'),
          summary: readUtf8(row.values, 'summary'),
          issues: parseJson<string[]>(readUtf8(row.values, 'issues_json'), []),
          insight: insight.length > 0 ? insight : null,
          followup: followup.length > 0 ? followup : null,
          createdAt: readI64(row.values, 'created_at'),
        };
      })
      .sort((a, b) => a.createdAt - b.createdAt);
  }
}

export function createConvoxReflectionStore(config: SunjetStorageConfig): ConvoxReflectionStore {
  return new ConvoxReflectionStore(config);
}
