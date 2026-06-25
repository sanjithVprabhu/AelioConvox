import type { RecalledMemory } from './recall.js';
import { buildCustomerProfile } from './profile.js';

export function summarizeMemories(memories: RecalledMemory[]): string {
  if (memories.length === 0) {
    return '';
  }

  return `Known customer context:\n${buildCustomerProfile(memories)}`;
}
