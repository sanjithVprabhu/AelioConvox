import { randomUUID } from 'node:crypto';
import { i64, readI64, readUtf8, utf8 } from './helpers.js';
import type { SunjetStorageConfig } from './types.js';

const SCAN_CAP = 2_000;

export type ProactiveRecordInput = {
  customerId: string;
  channel: string;
  to: string;
  content: string;
  dedupKey?: string;
  status: 'sent' | 'blocked';
  reason?: string;
};

export type ProactiveRecord = {
  id: string;
  customerId: string;
  channel: string;
  toAddress: string;
  content: string;
  dedupKey: string | null;
  status: string;
  reason: string | null;
  createdAt: number;
};

export class ConvoxProactiveStore {
  readonly client: SunjetStorageConfig['client'];
  readonly table: string;

  constructor(config: SunjetStorageConfig) {
    this.client = config.client;
    this.table = config.tables.proactiveMessages;
  }

  async record(input: ProactiveRecordInput): Promise<{ proactiveId: string }> {
    const proactiveId = randomUUID();
    await this.client.insertRow(this.table, {
      proactive_id: utf8(proactiveId),
      customer_id: utf8(input.customerId),
      channel: utf8(input.channel),
      to_address: utf8(input.to),
      content: utf8(input.content),
      dedup_key: utf8(input.dedupKey ?? ''),
      status: utf8(input.status),
      reason: utf8(input.reason ?? ''),
      created_at: i64(Date.now()),
    });
    return { proactiveId };
  }

  async findSentByDedup(dedupKey: string): Promise<ProactiveRecord | null> {
    const scan = await this.client.scanRows(this.table, {
      k: 1,
      filters: [
        { col: 'dedup_key', op: 'eq', value: utf8(dedupKey) },
        { col: 'status', op: 'eq', value: utf8('sent') },
      ],
    });
    const row = scan.rows[0];
    if (!row) {
      return null;
    }
    const dedup = readUtf8(row.values, 'dedup_key');
    const reason = readUtf8(row.values, 'reason');
    return {
      id: readUtf8(row.values, 'proactive_id'),
      customerId: readUtf8(row.values, 'customer_id'),
      channel: readUtf8(row.values, 'channel'),
      toAddress: readUtf8(row.values, 'to_address'),
      content: readUtf8(row.values, 'content'),
      dedupKey: dedup.length > 0 ? dedup : null,
      status: readUtf8(row.values, 'status'),
      reason: reason.length > 0 ? reason : null,
      createdAt: readI64(row.values, 'created_at'),
    };
  }

  /** Count `sent` proactive messages for a customer since `sinceMs` (epoch ms). */
  async countSentSince(customerId: string, sinceMs: number): Promise<number> {
    const scan = await this.client.scanRows(this.table, {
      k: SCAN_CAP,
      filters: [
        { col: 'customer_id', op: 'eq', value: utf8(customerId) },
        { col: 'status', op: 'eq', value: utf8('sent') },
        { col: 'created_at', op: 'ge', value: i64(sinceMs) },
      ],
    });
    return scan.rows.length;
  }
}

export function createConvoxProactiveStore(config: SunjetStorageConfig): ConvoxProactiveStore {
  return new ConvoxProactiveStore(config);
}
