/**
 * Immediate Context Engine — tiered short-term conversation context, per customer.
 *
 * Messages live in a hot 5-minute window (in-memory cache, verbatim). Once a
 * message ages past the hot window it "slides down" through time buckets
 * persisted in the Sunjet compactions table:
 *
 *   tier 1: 5–15 minutes ago
 *   tier 2: 15–30 minutes ago
 *   tier 3: 30–60 minutes ago
 *   tier 4: 1–24 hours ago
 *
 * A bucket's tier is derived from the age of its newest covered message
 * (`covers_to`). On every roll pass, buckets whose age crossed their tier
 * ceiling are re-tiered; buckets landing on the same tier are merged (and
 * LLM-condensed when the merged text grows past a threshold). Buckets older
 * than 24 hours are dropped — beyond that horizon, long-term semantic memory
 * (ConvoxMemoryStore) is the source of context, not this engine.
 *
 * The rendered context block is cached per customer with a short TTL and
 * invalidated on every recorded message, so the hot path usually costs zero
 * Sunjet reads. The block is designed to be fed into the system prompt via
 * the prompt factory so the model keeps continuity across sessions/channels.
 */

import type { LLMProvider } from '@aelio/llm';
import type { ApiValue } from '@aelio/sunjet-client';
import { i64, readI64, readUtf8, utf8 } from '../storage/helpers.js';
import type { SunjetStorageConfig } from '../storage/types.js';

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;

export const HOT_WINDOW_MS = 5 * MINUTE_MS;

export type ContextTierDef = {
  tier: number;
  label: string;
  /** Human-readable window used in the rendered prompt block. */
  human: string;
  /** Bucket belongs to this tier while `now - covers_to` is in [fromMs, toMs). */
  fromMs: number;
  toMs: number;
};

export const CONTEXT_TIERS: ContextTierDef[] = [
  { tier: 1, label: '5m-15m', human: '5–15 minutes ago', fromMs: HOT_WINDOW_MS, toMs: 15 * MINUTE_MS },
  { tier: 2, label: '15m-30m', human: '15–30 minutes ago', fromMs: 15 * MINUTE_MS, toMs: 30 * MINUTE_MS },
  { tier: 3, label: '30m-60m', human: '30–60 minutes ago', fromMs: 30 * MINUTE_MS, toMs: HOUR_MS },
  { tier: 4, label: '1h-24h', human: 'earlier today (1–24 hours ago)', fromMs: HOUR_MS, toMs: DAY_MS },
];

/** Tier for a bucket whose newest message is `ageMs` old. 0 = hot, -1 = expired. */
export function tierForAge(ageMs: number): number {
  if (ageMs < HOT_WINDOW_MS) return 0;
  for (const def of CONTEXT_TIERS) {
    if (ageMs < def.toMs) return def.tier;
  }
  return -1;
}

type HotMessage = {
  role: string;
  content: string;
  createdAt: number;
};

type CompactionBucket = {
  rowId: number | null;
  tier: number;
  content: string;
  coversFrom: number;
  coversTo: number;
};

export type ImmediateContextSnapshot = {
  /** Rendered prompt block ('' when there is nothing to say). */
  prompt: string;
  hotMessages: number;
  buckets: Array<{ tier: number; label: string; coversFrom: number; coversTo: number }>;
  builtAt: number;
};

export type ImmediateContextEngineOptions = {
  storage: SunjetStorageConfig;
  /** When present, tier merges are condensed by the LLM; otherwise deterministic truncation. */
  llm?: LLMProvider;
  model?: string;
  /** Snapshot cache TTL. Default 30s. */
  snapshotTtlMs?: number;
  /** Per-bucket character ceiling before an LLM condense (or truncation) kicks in. */
  maxBucketChars?: number;
};

const SCAN_CAP = 2_000;
const DEFAULT_SNAPSHOT_TTL_MS = 30_000;
const DEFAULT_MAX_BUCKET_CHARS = 1_600;
const HOT_MESSAGE_MAX_CHARS = 600;

function clip(textValue: string, maxChars: number): string {
  if (textValue.length <= maxChars) return textValue;
  return `${textValue.slice(0, maxChars)}…`;
}

function text(value: string): ApiValue {
  return { type: 'utf8', value };
}

export class ImmediateContextEngine {
  private readonly client: SunjetStorageConfig['client'];
  private readonly table: string;
  private readonly messagesTable: string;
  private readonly llm?: LLMProvider;
  private readonly model?: string;
  private readonly snapshotTtlMs: number;
  private readonly maxBucketChars: number;

  private readonly hot = new Map<string, HotMessage[]>();
  private readonly hotSeeded = new Set<string>();
  private readonly snapshots = new Map<string, ImmediateContextSnapshot>();
  /** Per-customer roll serialization — rolls mutate compaction rows and must not interleave. */
  private readonly locks = new Map<string, Promise<unknown>>();

  constructor(options: ImmediateContextEngineOptions) {
    this.client = options.storage.client;
    this.table = options.storage.tables.compactions;
    this.messagesTable = options.storage.tables.messages;
    this.llm = options.llm;
    this.model = options.model;
    this.snapshotTtlMs = options.snapshotTtlMs ?? DEFAULT_SNAPSHOT_TTL_MS;
    this.maxBucketChars = options.maxBucketChars ?? DEFAULT_MAX_BUCKET_CHARS;
  }

  /** Feed a just-persisted message into the hot window and invalidate the cached snapshot. */
  record(customerId: string, message: { role: string; content: string; createdAt?: number }): void {
    const entry: HotMessage = {
      role: message.role,
      content: message.content,
      createdAt: message.createdAt ?? Date.now(),
    };
    // Only append when the hot list has been seeded from Sunjet — otherwise the
    // next seed scan would double-count this message (it is already persisted).
    if (this.hotSeeded.has(customerId)) {
      const list = this.hot.get(customerId) ?? [];
      list.push(entry);
      this.hot.set(customerId, list);
    }
    this.snapshots.delete(customerId);
  }

  /**
   * Current immediate context for a customer: rolls aged messages down their
   * time buckets, then renders the prompt block. Cached between messages.
   */
  async getContext(customerId: string, now = Date.now()): Promise<ImmediateContextSnapshot> {
    const cached = this.snapshots.get(customerId);
    if (cached && now - cached.builtAt < this.snapshotTtlMs) {
      return cached;
    }
    return this.withLock(customerId, async () => {
      // Re-check under the lock — a concurrent caller may have just built it.
      const fresh = this.snapshots.get(customerId);
      if (fresh && now - fresh.builtAt < this.snapshotTtlMs) {
        return fresh;
      }
      const snapshot = await this.buildSnapshot(customerId, now);
      this.snapshots.set(customerId, snapshot);
      return snapshot;
    });
  }

  /** Convenience wrapper: prompt text only, never throws (context loss must not fail a turn). */
  async getPromptBlock(customerId: string, now = Date.now()): Promise<string> {
    try {
      const snapshot = await this.getContext(customerId, now);
      return snapshot.prompt;
    } catch (error) {
      console.warn(
        `[aelio] immediate context unavailable for ${customerId}: ${error instanceof Error ? error.message : String(error)}`,
      );
      return '';
    }
  }

  // ---- internals ----

  private withLock<T>(customerId: string, fn: () => Promise<T>): Promise<T> {
    const prev = this.locks.get(customerId) ?? Promise.resolve();
    const next = prev.then(fn, fn);
    this.locks.set(customerId, next.catch(() => undefined));
    return next;
  }

  private async buildSnapshot(customerId: string, now: number): Promise<ImmediateContextSnapshot> {
    const buckets = await this.loadBuckets(customerId);
    const rolledUpTo = buckets.reduce((max, bucket) => Math.max(max, bucket.coversTo), 0);

    // 1. Messages that aged out of the hot window and are not yet bucketed.
    const aged = await this.loadAgedMessages(customerId, rolledUpTo, now);
    const newBuckets = this.bucketizeMessages(aged, now);

    // 2. Slide: re-tier existing buckets whose newest edge crossed a boundary.
    const { keep, moved, expired } = this.retier(buckets, now);
    const merged = await this.mergeIntoTiers([...moved, ...newBuckets], keep, customerId, now);

    for (const bucket of expired) {
      if (bucket.rowId !== null) {
        await this.client.deleteRow(this.table, bucket.rowId);
      }
    }

    // 3. Hot window: verbatim last-5-minutes messages.
    const hotMessages = await this.loadHotWindow(customerId, now);

    const active = [...merged].sort((a, b) => a.tier - b.tier);
    return {
      prompt: this.render(hotMessages, active),
      hotMessages: hotMessages.length,
      buckets: active.map((bucket) => ({
        tier: bucket.tier,
        label: CONTEXT_TIERS.find((def) => def.tier === bucket.tier)?.label ?? String(bucket.tier),
        coversFrom: bucket.coversFrom,
        coversTo: bucket.coversTo,
      })),
      builtAt: now,
    };
  }

  private async loadBuckets(customerId: string): Promise<CompactionBucket[]> {
    const scan = await this.client.scanRows(this.table, {
      k: SCAN_CAP,
      filters: [{ col: 'customer_id', op: 'eq', value: utf8(customerId) }],
    });
    return scan.rows
      .map((row) => ({
        rowId: row.row_id,
        tier: readI64(row.values, 'tier'),
        content: readUtf8(row.values, 'content'),
        coversFrom: readI64(row.values, 'covers_from'),
        coversTo: readI64(row.values, 'covers_to'),
      }))
      .filter((bucket) => bucket.tier >= 1 && bucket.content.length > 0);
  }

  private async loadAgedMessages(
    customerId: string,
    rolledUpTo: number,
    now: number,
  ): Promise<HotMessage[]> {
    const floor = Math.max(rolledUpTo, now - DAY_MS);
    const ceiling = now - HOT_WINDOW_MS;
    if (ceiling <= floor) {
      return [];
    }
    const scan = await this.client.scanRows(this.messagesTable, {
      k: SCAN_CAP,
      filters: [
        { col: 'customer_id', op: 'eq', value: utf8(customerId) },
        { col: 'created_at', op: 'gt', value: i64(floor) },
        { col: 'created_at', op: 'lt', value: i64(ceiling) },
      ],
    });
    return scan.rows
      .map((row) => ({
        role: readUtf8(row.values, 'role'),
        content: readUtf8(row.values, 'content'),
        createdAt: readI64(row.values, 'created_at'),
      }))
      .filter((msg) => (msg.role === 'user' || msg.role === 'assistant') && msg.content.length > 0)
      .sort((a, b) => a.createdAt - b.createdAt);
  }

  /** Group aged messages by the tier their age puts them in and render each group as a transcript. */
  private bucketizeMessages(messages: HotMessage[], now: number): CompactionBucket[] {
    const groups = new Map<number, HotMessage[]>();
    for (const msg of messages) {
      const tier = tierForAge(now - msg.createdAt);
      if (tier < 1) continue;
      const list = groups.get(tier) ?? [];
      list.push(msg);
      groups.set(tier, list);
    }
    const out: CompactionBucket[] = [];
    for (const [tier, list] of groups) {
      out.push({
        rowId: null,
        tier,
        content: list
          .map((msg) => `${msg.role}: ${clip(msg.content, HOT_MESSAGE_MAX_CHARS)}`)
          .join('\n'),
        coversFrom: list[0]!.createdAt,
        coversTo: list[list.length - 1]!.createdAt,
      });
    }
    return out;
  }

  private retier(
    buckets: CompactionBucket[],
    now: number,
  ): { keep: CompactionBucket[]; moved: CompactionBucket[]; expired: CompactionBucket[] } {
    const keep: CompactionBucket[] = [];
    const moved: CompactionBucket[] = [];
    const expired: CompactionBucket[] = [];
    for (const bucket of buckets) {
      const target = tierForAge(now - bucket.coversTo);
      if (target === -1) {
        expired.push(bucket);
      } else if (target === bucket.tier) {
        keep.push(bucket);
      } else {
        moved.push({ ...bucket, tier: Math.max(target, 1) });
      }
    }
    return { keep, moved, expired };
  }

  /**
   * Persist moved/new buckets, merging any that land on a tier that already
   * has content. Returns the full active bucket set.
   */
  private async mergeIntoTiers(
    incoming: CompactionBucket[],
    keep: CompactionBucket[],
    customerId: string,
    now: number,
  ): Promise<CompactionBucket[]> {
    if (incoming.length === 0) {
      return keep;
    }

    const byTier = new Map<number, CompactionBucket[]>();
    for (const bucket of [...keep, ...incoming]) {
      const list = byTier.get(bucket.tier) ?? [];
      list.push(bucket);
      byTier.set(bucket.tier, list);
    }

    const result: CompactionBucket[] = [];
    for (const [tier, list] of byTier) {
      if (list.length === 1 && list[0]!.rowId !== null && keep.includes(list[0]!)) {
        // Untouched bucket — nothing to write.
        result.push(list[0]!);
        continue;
      }

      const sorted = [...list].sort((a, b) => a.coversFrom - b.coversFrom);
      let content = sorted.map((bucket) => bucket.content).join('\n');
      if (content.length > this.maxBucketChars) {
        content = await this.condense(content, tier);
      }
      const mergedBucket: CompactionBucket = {
        rowId: null,
        tier,
        content,
        coversFrom: sorted[0]!.coversFrom,
        coversTo: sorted[sorted.length - 1]!.coversTo,
      };

      // Reuse the first existing row when there is one; delete the rest.
      const existingRows = sorted.filter((bucket) => bucket.rowId !== null);
      const def = CONTEXT_TIERS.find((entry) => entry.tier === tier);
      const values: Record<string, ApiValue> = {
        compaction_id: text(`ice-${customerId}-${tier}-${mergedBucket.coversTo}`),
        customer_id: utf8(customerId),
        session_id: utf8(''),
        tier: i64(tier),
        label: utf8(def?.label ?? String(tier)),
        content: text(content),
        created_at: i64(now),
        covers_from: i64(mergedBucket.coversFrom),
        covers_to: i64(mergedBucket.coversTo),
      };
      if (existingRows.length > 0) {
        mergedBucket.rowId = existingRows[0]!.rowId;
        await this.client.updateRow(this.table, existingRows[0]!.rowId!, values);
        for (const stale of existingRows.slice(1)) {
          await this.client.deleteRow(this.table, stale.rowId!);
        }
      } else {
        const inserted = await this.client.insertRow(this.table, values);
        mergedBucket.rowId = inserted.row_id;
      }
      result.push(mergedBucket);
    }
    return result;
  }

  /** LLM condense with deterministic tail-truncation fallback. */
  private async condense(content: string, tier: number): Promise<string> {
    if (this.llm && this.model) {
      try {
        const def = CONTEXT_TIERS.find((entry) => entry.tier === tier);
        const result = await this.llm.complete({
          model: this.model,
          maxTokens: 250,
          tools: [],
          system:
            'Condense this conversation excerpt into a short factual recap (3-6 bullets). ' +
            'Keep names, numbers, decisions, open questions, and commitments. Drop pleasantries. ' +
            `This covers the window: ${def?.human ?? 'recently'}.`,
          messages: [{ role: 'user', content }],
          telemetry: { purpose: 'immediate_context_compaction' },
        });
        const condensed = result.text.trim();
        if (condensed.length > 0) {
          return condensed;
        }
      } catch {
        // fall through to truncation
      }
    }
    return `${content.slice(0, this.maxBucketChars)}\n…[older context trimmed]`;
  }

  private async loadHotWindow(customerId: string, now: number): Promise<HotMessage[]> {
    if (!this.hotSeeded.has(customerId)) {
      const scan = await this.client.scanRows(this.messagesTable, {
        k: SCAN_CAP,
        filters: [
          { col: 'customer_id', op: 'eq', value: utf8(customerId) },
          { col: 'created_at', op: 'ge', value: i64(now - HOT_WINDOW_MS) },
        ],
      });
      const seeded = scan.rows
        .map((row) => ({
          role: readUtf8(row.values, 'role'),
          content: readUtf8(row.values, 'content'),
          createdAt: readI64(row.values, 'created_at'),
        }))
        .filter((msg) => msg.role === 'user' || msg.role === 'assistant')
        .sort((a, b) => a.createdAt - b.createdAt);
      this.hot.set(customerId, seeded);
      this.hotSeeded.add(customerId);
    }
    const pruned = (this.hot.get(customerId) ?? []).filter(
      (msg) => now - msg.createdAt < HOT_WINDOW_MS,
    );
    this.hot.set(customerId, pruned);
    return pruned;
  }

  private render(hotMessages: HotMessage[], buckets: CompactionBucket[]): string {
    const sections: string[] = [];

    if (hotMessages.length > 0) {
      const lines = hotMessages.map(
        (msg) => `${msg.role}: ${clip(msg.content, HOT_MESSAGE_MAX_CHARS)}`,
      );
      sections.push(`[last 5 minutes — verbatim]\n${lines.join('\n')}`);
    }

    for (const bucket of buckets) {
      const def = CONTEXT_TIERS.find((entry) => entry.tier === bucket.tier);
      sections.push(`[${def?.human ?? `tier ${bucket.tier}`}]\n${bucket.content}`);
    }

    if (sections.length === 0) {
      return '';
    }

    return (
      'IMMEDIATE CONVERSATION CONTEXT (this customer, most recent first — use it to stay ' +
      'consistent with what was just discussed, across sessions and channels):\n\n' +
      sections.join('\n\n')
    );
  }
}

export function createImmediateContextEngine(
  options: ImmediateContextEngineOptions,
): ImmediateContextEngine {
  return new ImmediateContextEngine(options);
}
