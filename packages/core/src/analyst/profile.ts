import type { RecalledMemory } from './recall.js';

export function buildCustomerProfile(memories: RecalledMemory[]): string {
  const grouped = new Map<string, string[]>();
  for (const memory of memories) {
    const key = memory.category ?? 'general';
    const values = grouped.get(key) ?? [];
    values.push(memory.content);
    grouped.set(key, values);
  }

  return [...grouped.entries()]
    .map(([category, values]) => `${category}: ${values.slice(0, 3).join(' | ')}`)
    .join('\n');
}
