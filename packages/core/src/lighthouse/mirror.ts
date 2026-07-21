import type { ApiValue, SunjetClient } from '@aelio/sunjet-client';
import type { FunctionDefinition } from '@aelio/protocol';
import { cosineSimilarity, embed } from '../analyst/embeddings.js';
import { normalizeEmbedding } from '../storage/messages.js';
import type { RegistrySnapshot } from './hash.js';

export type LighthouseMirrorConfig = {
  client: SunjetClient;
  toolsTable: string;
  capabilitiesTable: string;
  embedDim: number;
  tenant: string;
};

export type ToolSearchHit = {
  fn: FunctionDefinition;
  score: number;
};

function utf8(value: string): ApiValue {
  return { type: 'utf8', value };
}

function readUtf8(values: Record<string, ApiValue> | undefined, key: string): string {
  const entry = values?.[key];
  return entry && (entry.type === 'utf8') ? entry.value : '';
}

function readVector(values: Record<string, ApiValue> | undefined, key: string): number[] | null {
  const entry = values?.[key];
  return entry && entry.type === 'vector' ? entry.value : null;
}

/** The same descriptor tool-retrieval ranks against — keep the two aligned. */
export function toolDescriptor(fn: FunctionDefinition): string {
  return `${fn.name} — ${fn.intent ?? fn.name} — ${fn.description}`;
}

/**
 * Retrieval-expansion edges (recall booster only — never used for execution-time
 * dependency wiring). Heuristic: a required param named `<token>_id`/`<token>Id`
 * links the consumer to any tool whose name contains that token — so a search
 * that finds `apply_coupon` (requires cart_id) also pulls `add_to_cart`.
 */
function deriveRequiresEdges(
  fn: FunctionDefinition,
  rowIdByName: Map<string, number>,
  all: FunctionDefinition[],
): number[] {
  const paramNames = Object.keys(fn.params ?? {});
  const tokens = new Set<string>();
  for (const param of paramNames) {
    const match = /^(.+?)_?[iI]d$/.exec(param);
    if (match?.[1]) {
      tokens.add(match[1].toLowerCase().replace(/_/g, ''));
    }
  }
  if (tokens.size === 0) {
    return [];
  }
  const edges: number[] = [];
  for (const candidate of all) {
    if (candidate.name === fn.name) continue;
    const nameTokens = candidate.name.toLowerCase().replace(/[^a-z0-9]/g, '');
    for (const token of tokens) {
      if (nameTokens.includes(token)) {
        const rowId = rowIdByName.get(candidate.name);
        if (rowId !== undefined) {
          edges.push(rowId);
        }
        break;
      }
    }
  }
  return edges;
}

export class LighthouseMirror {
  constructor(private readonly config: LighthouseMirrorConfig) {}

  /**
   * Replace this tenant's mirror rows with the given registry snapshot.
   * Old rows are removed first (scan by tenant), then tools are inserted, then
   * `requires` edges are wired in a second pass (edges need target row ids).
   */
  async sync(snapshot: RegistrySnapshot, hash: string, brief: string): Promise<void> {
    const { client, toolsTable, capabilitiesTable, embedDim, tenant } = this.config;

    await this.deleteTenantRows(toolsTable);
    await this.deleteTenantRows(capabilitiesTable);

    const rowIdByName = new Map<string, number>();
    for (const fn of snapshot.functions) {
      const vector = normalizeEmbedding(await embed(toolDescriptor(fn)), embedDim);
      const inserted = await client.insertRow(toolsTable, {
        tenant: utf8(tenant),
        registry_hash: utf8(hash),
        name: utf8(fn.name),
        description: utf8(fn.description),
        intent: utf8(fn.intent ?? fn.name),
        safety: utf8(fn.safety),
        params_json: utf8(JSON.stringify(fn.params ?? {})),
        embedding: { type: 'vector', value: vector },
      });
      rowIdByName.set(fn.name, inserted.row_id);
    }

    // Second pass: edges (consumer → likely producer).
    for (const fn of snapshot.functions) {
      const edges = deriveRequiresEdges(fn, rowIdByName, snapshot.functions);
      const rowId = rowIdByName.get(fn.name);
      if (rowId !== undefined && edges.length > 0) {
        await client.updateRow(toolsTable, rowId, {
          requires: { type: 'edges', value: edges },
        });
      }
    }

    // Capability nodes: one row per intent category plus the whole brief, so the
    // feasibility probe can match either granular capabilities or the product story.
    const categories = new Map<string, string[]>();
    for (const fn of snapshot.functions) {
      const category = fn.intent ?? 'general';
      const line = `${fn.name}: ${fn.description}`;
      const bucket = categories.get(category);
      if (bucket) {
        bucket.push(line);
      } else {
        categories.set(category, [line]);
      }
    }
    const capabilityRows = [...categories.entries()].map(([category, lines]) => ({
      category,
      content: `${category}\n${lines.join('\n')}`,
    }));
    if (brief) {
      capabilityRows.push({ category: '__brief__', content: brief });
    }
    for (const { category, content } of capabilityRows) {
      const vector = normalizeEmbedding(await embed(content), embedDim);
      await client.insertRow(capabilitiesTable, {
        tenant: utf8(tenant),
        registry_hash: utf8(hash),
        category: utf8(category),
        content: utf8(content),
        embedding: { type: 'vector', value: vector },
      });
    }
  }

  /**
   * Semantic tool search with optional 1-hop `requires` expansion. Expanded
   * prerequisites inherit a slightly lower score than their seed so ranking
   * stays stable. Returns tool definitions resolved from the live registry
   * (the mirror stores enough to reconstruct, but the live registry is
   * authoritative for schemas).
   */
  async searchTools(
    queryText: string,
    k: number,
    registry: FunctionDefinition[],
    options?: { expandGraph?: boolean },
  ): Promise<ToolSearchHit[]> {
    const { client, toolsTable, embedDim, tenant } = this.config;
    const vector = normalizeEmbedding(await embed(queryText), embedDim);

    const response = await client.query(toolsTable, {
      k,
      vector: { col: 'embedding', query: vector },
      filters: [{ col: 'tenant', op: 'eq', value: utf8(tenant) }],
    });

    const rowIds = new Set<number>();
    for (const hit of response.results) {
      rowIds.add(hit.row_id);
    }

    if (options?.expandGraph !== false && response.results.length > 0) {
      const seeds = response.results.map((hit) => hit.row_id);
      const expansion = await client.query(toolsTable, {
        k: k * 2,
        graph: { col: 'requires', seeds, depth: 1 },
        filters: [{ col: 'tenant', op: 'eq', value: utf8(tenant) }],
      });
      for (const hit of expansion.results) {
        rowIds.add(hit.row_id);
      }
    }

    const registryByName = new Map(registry.map((fn) => [fn.name, fn]));
    const hits: ToolSearchHit[] = [];
    for (const rowId of rowIds) {
      const row = await client.getRow(toolsTable, rowId);
      const name = readUtf8(row?.values, 'name');
      const fn = registryByName.get(name);
      if (fn) {
        // Sunjet query scores are RRF ranks (~0.016 ceiling), not similarities.
        // Binder gates (scoreMin/ambiguityGap) are calibrated on cosine, so
        // recompute cosine against the stored embedding — same convention as
        // the archetype store and the in-process fallback ranker.
        const stored = readVector(row?.values, 'embedding');
        const score = stored ? cosineSimilarity(vector, stored) : 0;
        hits.push({ fn, score });
      }
    }
    hits.sort((a, b) => b.score - a.score);
    return hits.slice(0, k);
  }

  /** Feasibility probe: does anything in the capability index resemble the ask? */
  async probeCapabilities(queryText: string, k = 3): Promise<number> {
    const { client, capabilitiesTable, embedDim, tenant } = this.config;
    const vector = normalizeEmbedding(await embed(queryText), embedDim);
    const response = await client.query(capabilitiesTable, {
      k,
      vector: { col: 'embedding', query: vector },
      filters: [{ col: 'tenant', op: 'eq', value: utf8(tenant) }],
    });
    // Recompute cosine per hit: Sunjet query scores are RRF ranks, but the
    // planner's feasibility challenge threshold is calibrated on cosine.
    let best = 0;
    for (const hit of response.results) {
      const row = await client.getRow(capabilitiesTable, hit.row_id);
      const stored = readVector(row?.values, 'embedding');
      if (stored) {
        best = Math.max(best, cosineSimilarity(vector, stored));
      }
    }
    return best;
  }

  private async deleteTenantRows(table: string): Promise<void> {
    const { client, tenant } = this.config;
    // Scan is bounded: registries are dozens-to-hundreds of rows per tenant.
    const scan = await client.scanRows(table, {
      k: 10_000,
      filters: [{ col: 'tenant', op: 'eq', value: utf8(tenant) }],
    });
    for (const row of scan.rows) {
      await client.deleteRow(table, row.row_id);
    }
  }
}
