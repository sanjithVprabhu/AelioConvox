import { createHash } from 'node:crypto';
import type { FunctionDefinition } from '@aelio/protocol';
import type { AelioDbClient } from '@aelio/db-client';
import type { LighthouseService } from '../lighthouse/index.js';
import { embed } from '../analyst/embeddings.js';
import { i64, readI64, readUtf8, utf8 } from '../storage/helpers.js';
import { normalizeEmbedding } from '../storage/messages.js';
import type { HarnessBindingConfig, PlanInstruction } from './schema.js';
import type { BoundInstruction } from './resolver.js';

export type BindResult =
  | { ok: true; bound: BoundInstruction[] }
  | { ok: false; unbound: PlanInstruction[]; bound: BoundInstruction[] };

export type BindingCacheConfig = {
  client: AelioDbClient;
  table: string;
  tenant: string;
  registryHash: string;
  embedDim: number;
  ttlMinutes: number;
};

function bindingKey(tenant: string, registryHash: string, instruction: string): string {
  return createHash('sha256')
    .update(`${tenant}|${registryHash}|${instruction.trim().toLowerCase()}`)
    .digest('hex')
    .slice(0, 32);
}

async function lookupCachedTool(
  cache: BindingCacheConfig,
  capability: string,
): Promise<string | null> {
  const key = bindingKey(cache.tenant, cache.registryHash, capability);
  const now = Date.now();
  const scan = await cache.client.scanRows(cache.table, {
    k: 1,
    filters: [
      { col: 'binding_key', op: 'eq', value: utf8(key) },
      { col: 'tenant', op: 'eq', value: utf8(cache.tenant) },
    ],
  });
  const row = scan.rows[0];
  if (!row) {
    return null;
  }
  if (readI64(row.values, 'expires_at') <= now) {
    return null;
  }
  const tool = readUtf8(row.values, 'tool_name');
  return tool.length > 0 ? tool : null;
}

async function storeCachedTool(
  cache: BindingCacheConfig,
  capability: string,
  toolName: string,
): Promise<void> {
  try {
    const now = Date.now();
    const embedding = normalizeEmbedding(await embed(capability), cache.embedDim);
    await cache.client.insertRow(cache.table, {
      binding_key: utf8(bindingKey(cache.tenant, cache.registryHash, capability)),
      tenant: utf8(cache.tenant),
      registry_hash: utf8(cache.registryHash),
      instruction: utf8(capability),
      embedding: { type: 'vector', value: embedding },
      tool_name: utf8(toolName),
      created_at: i64(now),
      expires_at: i64(now + cache.ttlMinutes * 60_000),
    });
  } catch {
    // Cache write is best-effort.
  }
}

/**
 * Bind capability-level instructions to concrete tools. Sources, in order:
 *   1. Planner `tool` suggestion (trusted if in registry)
 *   2. AelioDb binding cache (when configured)
 *   3. Lighthouse semantic search with score/ambiguity gates
 */
export async function bindInstructions(
  instructions: PlanInstruction[],
  registry: FunctionDefinition[],
  lighthouse: LighthouseService | undefined,
  binding: HarnessBindingConfig,
  cache?: BindingCacheConfig,
): Promise<BindResult> {
  const byName = new Map(registry.map((fn) => [fn.name, fn]));
  const normalize = (name: string) => name.toLowerCase().replace(/[^a-z0-9]/g, '');
  const byNormalizedName = new Map(registry.map((fn) => [normalize(fn.name), fn]));
  const bound: BoundInstruction[] = [];
  const unbound: PlanInstruction[] = [];

  for (const instruction of instructions) {
    // 1. Planner suggestion — exact, then case/punctuation-insensitive. The
    // planner often writes the tool name into `capability` instead of `tool`,
    // so check both fields before paying for cache/semantic lookups.
    const suggested =
      (instruction.tool ? byName.get(instruction.tool) : undefined) ??
      (instruction.tool ? byNormalizedName.get(normalize(instruction.tool)) : undefined) ??
      byNormalizedName.get(normalize(instruction.capability));
    if (suggested) {
      bound.push({ instruction, tool: suggested });
      continue;
    }

    // 2. Persistent binding cache.
    if (cache) {
      try {
        const cachedName = await lookupCachedTool(cache, instruction.capability);
        const cached = cachedName ? byName.get(cachedName) : undefined;
        if (cached) {
          bound.push({ instruction, tool: cached });
          continue;
        }
      } catch {
        // Fall through to semantic search.
      }
    }

    // 3. Semantic search.
    if (lighthouse) {
      const hits = await lighthouse.searchTools(instruction.capability, 3);
      const top = hits[0];
      const runner = hits[1];
      if (
        top &&
        top.score >= binding.scoreMin &&
        (!runner || top.score - runner.score >= binding.ambiguityGap)
      ) {
        const fn = byName.get(top.fn.name);
        if (fn) {
          bound.push({ instruction, tool: fn });
          if (cache) {
            void storeCachedTool(cache, instruction.capability, fn.name);
          }
          continue;
        }
      }
    }

    unbound.push(instruction);
  }

  if (unbound.length > 0) {
    return { ok: false, unbound, bound };
  }
  return { ok: true, bound };
}
