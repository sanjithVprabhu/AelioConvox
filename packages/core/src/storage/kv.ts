import { randomUUID } from 'node:crypto';
import { i64, parseJson, readI64, readUtf8, utf8 } from './helpers.js';
import type { AelioDbStorageConfig } from './types.js';

const SCAN_CAP = 2_000;
const DEDUP_PRUNE_AFTER_MS = 7 * 24 * 60 * 60 * 1000;

/**
 * Inbound webhook message dedup. AelioDb has no unique-constraint enforcement
 * over HTTP, so `claim()` is scan-then-insert rather than a true atomic
 * `INSERT OR IGNORE`: a same-millisecond race between two claims for the same
 * `messageId` could both succeed. This mirrors the acceptable-risk tradeoff
 * called out for the SQLite version (BUG-041-style races are rare enough in
 * practice to not warrant a distributed lock here).
 */
export class ConvoxInboundDedupStore {
  readonly client: AelioDbStorageConfig['client'];
  readonly table: string;

  constructor(config: AelioDbStorageConfig) {
    this.client = config.client;
    this.table = config.tables.inboundDedup;
  }

  /** Returns true if this is the first claim for `messageId`, false if already claimed. */
  async claim(messageId: string): Promise<boolean> {
    const scan = await this.client.scanRows(this.table, {
      k: 1,
      filters: [{ col: 'message_id', op: 'eq', value: utf8(messageId) }],
    });
    if (scan.rows.length > 0) {
      return false;
    }

    await this.client.insertRow(this.table, {
      message_id: utf8(messageId),
      created_at: i64(Date.now()),
    });

    if (Math.random() < 0.01) {
      await this.prune(DEDUP_PRUNE_AFTER_MS);
    }

    return true;
  }

  async prune(maxAgeMs: number): Promise<number> {
    const cutoff = Date.now() - maxAgeMs;
    const scan = await this.client.scanRows(this.table, {
      k: SCAN_CAP,
      filters: [{ col: 'created_at', op: 'lt', value: i64(cutoff) }],
    });
    for (const row of scan.rows) {
      await this.client.deleteRow(this.table, row.row_id);
    }
    return scan.rows.length;
  }
}

export type MagicLinkRecord = {
  id: string;
  tokenHash: string;
  email: string;
  customerId: string | null;
  externalId: string | null;
  expiresAt: number;
  consumedAt: number | null;
  createdAt: number;
};

export type MagicLinkCreateInput = {
  tokenHash: string;
  email: string;
  customerId: string;
  externalId: string;
  expiresAtMs: number;
};

export class ConvoxMagicLinkStore {
  readonly client: AelioDbStorageConfig['client'];
  readonly table: string;

  constructor(config: AelioDbStorageConfig) {
    this.client = config.client;
    this.table = config.tables.magicLinks;
  }

  async create(input: MagicLinkCreateInput): Promise<{ id: string }> {
    const id = randomUUID();
    await this.client.insertRow(this.table, {
      link_id: utf8(id),
      token_hash: utf8(input.tokenHash),
      email: utf8(input.email),
      customer_id: utf8(input.customerId),
      external_id: utf8(input.externalId),
      expires_at: i64(input.expiresAtMs),
      consumed_at: i64(0),
      created_at: i64(Date.now()),
    });
    return { id };
  }

  async findByTokenHash(tokenHash: string): Promise<MagicLinkRecord | null> {
    const scan = await this.client.scanRows(this.table, {
      k: 1,
      filters: [{ col: 'token_hash', op: 'eq', value: utf8(tokenHash) }],
    });
    const row = scan.rows[0];
    if (!row) {
      return null;
    }
    const customerId = readUtf8(row.values, 'customer_id');
    const externalId = readUtf8(row.values, 'external_id');
    return {
      id: readUtf8(row.values, 'link_id'),
      tokenHash: readUtf8(row.values, 'token_hash'),
      email: readUtf8(row.values, 'email'),
      customerId: customerId.length > 0 ? customerId : null,
      externalId: externalId.length > 0 ? externalId : null,
      expiresAt: readI64(row.values, 'expires_at'),
      consumedAt: readI64(row.values, 'consumed_at') || null,
      createdAt: readI64(row.values, 'created_at'),
    };
  }

  /**
   * Best-effort atomic consume: re-reads the row and only flips `consumed_at`
   * if it is still unset. Returns false if already consumed (or missing).
   */
  async consume(id: string): Promise<boolean> {
    const scan = await this.client.scanRows(this.table, {
      k: 1,
      filters: [{ col: 'link_id', op: 'eq', value: utf8(id) }],
    });
    const row = scan.rows[0];
    if (!row) {
      return false;
    }
    if (readI64(row.values, 'consumed_at') > 0) {
      return false;
    }
    await this.client.updateRow(this.table, row.row_id, {
      consumed_at: i64(Date.now()),
    });
    return true;
  }
}

export type SdkConnectionUpsertInput = {
  id: string;
  sdkVersion: string;
  language: string;
  connectedAt: number;
  lastHeartbeatAt: number;
  functions: Array<Record<string, unknown>>;
};

export class ConvoxSdkConnectionStore {
  readonly client: AelioDbStorageConfig['client'];
  readonly table: string;

  constructor(config: AelioDbStorageConfig) {
    this.client = config.client;
    this.table = config.tables.sdkConnections;
  }

  private async findByConnectionId(connectionId: string) {
    const scan = await this.client.scanRows(this.table, {
      k: 1,
      filters: [{ col: 'connection_id', op: 'eq', value: utf8(connectionId) }],
    });
    return scan.rows[0] ?? null;
  }

  async upsert(connection: SdkConnectionUpsertInput): Promise<void> {
    const values = {
      connection_id: utf8(connection.id),
      sdk_version: utf8(connection.sdkVersion),
      language: utf8(connection.language),
      connected_at: i64(connection.connectedAt),
      last_heartbeat_at: i64(connection.lastHeartbeatAt),
      functions_json: utf8(JSON.stringify(connection.functions ?? [])),
    };

    const existing = await this.findByConnectionId(connection.id);
    if (existing) {
      await this.client.updateRow(this.table, existing.row_id, values);
    } else {
      await this.client.insertRow(this.table, values);
    }
  }

  async remove(connectionId: string): Promise<void> {
    const existing = await this.findByConnectionId(connectionId);
    if (existing) {
      await this.client.deleteRow(this.table, existing.row_id);
    }
  }

  /** Prune rows whose heartbeat predates `cutoffMs` (stale connections from prior boots). */
  async pruneStale(cutoffMs: number): Promise<number> {
    const scan = await this.client.scanRows(this.table, {
      k: SCAN_CAP,
      filters: [{ col: 'last_heartbeat_at', op: 'lt', value: i64(cutoffMs) }],
    });
    for (const row of scan.rows) {
      await this.client.deleteRow(this.table, row.row_id);
    }
    return scan.rows.length;
  }

  async listFunctions(): Promise<Array<Record<string, unknown>>> {
    const scan = await this.client.scanRows(this.table, { k: SCAN_CAP });
    const out: Array<Record<string, unknown>> = [];
    for (const row of scan.rows) {
      const functions = parseJson<Array<Record<string, unknown>>>(
        readUtf8(row.values, 'functions_json'),
        [],
      );
      out.push(...functions);
    }
    return out;
  }
}

export function createConvoxInboundDedupStore(
  config: AelioDbStorageConfig,
): ConvoxInboundDedupStore {
  return new ConvoxInboundDedupStore(config);
}

export function createConvoxMagicLinkStore(config: AelioDbStorageConfig): ConvoxMagicLinkStore {
  return new ConvoxMagicLinkStore(config);
}

export function createConvoxSdkConnectionStore(
  config: AelioDbStorageConfig,
): ConvoxSdkConnectionStore {
  return new ConvoxSdkConnectionStore(config);
}
