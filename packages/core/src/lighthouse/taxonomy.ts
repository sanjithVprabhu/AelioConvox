import type { RegistrySnapshot } from './hash.js';

/**
 * The capability brief: a compact, hierarchical description of what this
 * product can do, generated deterministically from the registry (no LLM).
 * The planner reads this instead of a raw tool dump, so Pass 1 scales to
 * hundreds of tools without prompt blowout. A tenant-supplied `productBrief`
 * becomes the preamble; the taxonomy grounds it in concrete capabilities.
 *
 * Structure: tools group into categories by their declared `intent` (the
 * tenant's own vocabulary); each category lists capability one-liners.
 */
export function buildCapabilityBrief(snapshot: RegistrySnapshot): string {
  const sections: string[] = [];

  if (snapshot.productBrief) {
    sections.push(snapshot.productBrief.trim());
  }

  if (snapshot.functions.length > 0) {
    const categories = new Map<string, string[]>();
    for (const fn of snapshot.functions) {
      const category = fn.intent ?? 'general';
      const line = `- ${fn.name}: ${firstSentence(fn.description)}${fn.safety !== 'read' ? ` [${fn.safety}]` : ''}`;
      const bucket = categories.get(category);
      if (bucket) {
        bucket.push(line);
      } else {
        categories.set(category, [line]);
      }
    }

    const lines: string[] = ['Capabilities (grouped by intent):'];
    for (const [category, entries] of [...categories.entries()].sort(([a], [b]) =>
      a.localeCompare(b),
    )) {
      lines.push(`${category}:`);
      lines.push(...entries.sort());
    }
    sections.push(lines.join('\n'));
  }

  if (snapshot.flows.length > 0) {
    const lines = ['Guided flows:'];
    for (const flow of [...snapshot.flows].sort((a, b) => a.id.localeCompare(b.id))) {
      lines.push(
        `- ${flow.id} (state: ${flow.state}): ${firstSentence(flow.description)} — steps: ${flow.steps
          .map((step) => step.id)
          .join(' → ')}`,
      );
    }
    sections.push(lines.join('\n'));
  }

  return sections.join('\n\n').trim();
}

function firstSentence(text: string): string {
  const trimmed = text.trim();
  const period = trimmed.indexOf('. ');
  return period > 0 ? trimmed.slice(0, period + 1) : trimmed;
}
