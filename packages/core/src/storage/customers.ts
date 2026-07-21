import { randomUUID } from 'node:crypto';
import type { RowValueResponse } from '@aelio/sunjet-client';
import { i64, parseJson, readI64, readUtf8, utf8 } from './helpers.js';
import type { SunjetStorageConfig } from './types.js';

const SCAN_CAP = 500;

export type CustomerRecord = {
  id: string;
  externalId: string | null;
  displayName: string | null;
  metadata: Record<string, unknown>;
  createdAt: number;
  updatedAt: number;
};

export class ConvoxCustomerStore {
  readonly client: SunjetStorageConfig['client'];
  readonly table: string;
  readonly channelTable: string;

  constructor(config: SunjetStorageConfig) {
    this.client = config.client;
    this.table = config.tables.customers;
    this.channelTable = config.tables.channelAddresses;
  }

  private toRecord(row: RowValueResponse): CustomerRecord {
    const externalId = readUtf8(row.values, 'external_id');
    const displayName = readUtf8(row.values, 'display_name');
    return {
      id: readUtf8(row.values, 'customer_id'),
      externalId: externalId.length > 0 ? externalId : null,
      displayName: displayName.length > 0 ? displayName : null,
      metadata: parseJson(readUtf8(row.values, 'metadata'), {}),
      createdAt: readI64(row.values, 'created_at'),
      updatedAt: readI64(row.values, 'updated_at'),
    };
  }

  private async findByCustomerId(customerId: string): Promise<RowValueResponse | null> {
    const scan = await this.client.scanRows(this.table, {
      k: 1,
      filters: [{ col: 'customer_id', op: 'eq', value: utf8(customerId) }],
    });
    return scan.rows[0] ?? null;
  }

  private async findByExternalIdRow(externalId: string): Promise<RowValueResponse | null> {
    const scan = await this.client.scanRows(this.table, {
      k: 1,
      filters: [{ col: 'external_id', op: 'eq', value: utf8(externalId) }],
    });
    return scan.rows[0] ?? null;
  }

  async getById(customerId: string): Promise<CustomerRecord | null> {
    const row = await this.findByCustomerId(customerId);
    return row ? this.toRecord(row) : null;
  }

  async getByExternalId(externalId: string): Promise<CustomerRecord | null> {
    const row = await this.findByExternalIdRow(externalId);
    return row ? this.toRecord(row) : null;
  }

  async updateMetadata(customerId: string, metadata: Record<string, unknown>): Promise<void> {
    const row = await this.findByCustomerId(customerId);
    if (!row) {
      return;
    }
    await this.client.updateRow(this.table, row.row_id, {
      metadata: utf8(JSON.stringify(metadata)),
      updated_at: i64(Date.now()),
    });
  }

  async getChannelAddress(customerId: string, channel: string): Promise<string | null> {
    const scan = await this.client.scanRows(this.channelTable, {
      k: 1,
      filters: [
        { col: 'customer_id', op: 'eq', value: utf8(customerId) },
        { col: 'channel', op: 'eq', value: utf8(channel) },
      ],
    });
    const row = scan.rows[0];
    if (!row) {
      return null;
    }
    const address = readUtf8(row.values, 'address');
    return address.length > 0 ? address : null;
  }

  /**
   * Resolve (or create) the customer behind a channel address. Mirrors the
   * SQLite `ensureCustomer` semantics: an existing verified channel address
   * wins outright, otherwise fall back to (or create) the customer by
   * external id and record the new address against it.
   */
  async ensureCustomer(
    externalId: string,
    channel: string,
    channelAddress: string,
  ): Promise<string> {
    const addressScan = await this.client.scanRows(this.channelTable, {
      k: SCAN_CAP,
      filters: [
        { col: 'channel', op: 'eq', value: utf8(channel) },
        { col: 'address', op: 'eq', value: utf8(channelAddress) },
      ],
    });
    const existingAddress = addressScan.rows[0];
    if (existingAddress) {
      const customerId = readUtf8(existingAddress.values, 'customer_id');
      if (customerId) {
        return customerId;
      }
    }

    const existingCustomer = await this.findByExternalIdRow(externalId);
    const now = Date.now();
    const customerId = existingCustomer
      ? readUtf8(existingCustomer.values, 'customer_id')
      : randomUUID();

    if (!existingCustomer) {
      await this.client.insertRow(this.table, {
        customer_id: utf8(customerId),
        external_id: utf8(externalId),
        display_name: utf8(externalId),
        metadata: utf8('{}'),
        created_at: i64(now),
        updated_at: i64(now),
      });
    }

    await this.client.insertRow(this.channelTable, {
      address_id: utf8(randomUUID()),
      customer_id: utf8(customerId),
      channel: utf8(channel),
      address: utf8(channelAddress),
      verified_at: i64(now),
      created_at: i64(now),
    });

    return customerId;
  }
}

export function createConvoxCustomerStore(config: SunjetStorageConfig): ConvoxCustomerStore {
  return new ConvoxCustomerStore(config);
}
