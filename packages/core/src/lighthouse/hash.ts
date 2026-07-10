import { createHash } from 'node:crypto';
import type {
  FlowDefinition,
  FunctionDefinition,
  PolicyDefinition,
  StateDefinition,
} from '@aelio/protocol';

export type RegistrySnapshot = {
  functions: FunctionDefinition[];
  states: StateDefinition[];
  policies: PolicyDefinition[];
  flows: FlowDefinition[];
  persona: string | null;
  productBrief: string | null;
};

/** Recursively sort object keys so hashing is insensitive to property order. */
function canonicalize(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(canonicalize);
  }
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
        .map(([key, entry]) => [key, canonicalize(entry)]),
    );
  }
  return value;
}

/**
 * Content hash of the merged registry. Changes iff the registry meaningfully
 * changes — the invalidation key for tool embeddings, the capability taxonomy,
 * and every instruction→tool binding cache entry. Entities are sorted by their
 * identity so connection ordering never affects the hash.
 */
export function registryHash(snapshot: RegistrySnapshot): string {
  const byName = <T>(key: (entry: T) => string) => (a: T, b: T) =>
    key(a) < key(b) ? -1 : key(a) > key(b) ? 1 : 0;

  const canonical = canonicalize({
    functions: [...snapshot.functions].sort(byName((f) => f.name)),
    states: [...snapshot.states].sort(byName((s) => s.id)),
    policies: [...snapshot.policies].sort(byName((p) => p.id)),
    flows: [...snapshot.flows].sort(byName((f) => f.id)),
    persona: snapshot.persona,
    productBrief: snapshot.productBrief,
  });

  return createHash('sha256').update(JSON.stringify(canonical)).digest('hex');
}
