/**
 * Deterministic system-prompt assembly with token budgets and cache-friendly
 * ordering.
 *
 * Why ordering matters: providers cache prompt PREFIXES (OpenAI automatically for
 * stable prefixes ≥1024 tokens, Anthropic via cache_control). Interleaving
 * volatile content (per-message memories, intent) with stable content (persona,
 * policies, tool guidance) changes the prefix every turn and defeats caching —
 * full price for the stable majority on every call, including every tool-loop
 * iteration. The composer renders all `stable` sections first, `volatile` last.
 *
 * Why budgets matter: unbounded concatenation grows the prompt with session age.
 * Each section carries a priority; when the total exceeds the budget, volatile
 * sections are trimmed lowest-priority-first. Stable sections are never silently
 * dropped — they are the contract (persona, policies, lifecycle boundaries).
 */

export type PromptSection = {
  id: string;
  content: string;
  stability: 'stable' | 'volatile';
  /** Higher survives longer under budget pressure. */
  priority: number;
  /** Optional per-section cap (approx tokens). */
  maxTokens?: number;
};

export type ComposeOptions = {
  /** Total approx-token budget for the whole system prompt. Default 6000. */
  totalBudgetTokens?: number;
};

const DEFAULT_TOTAL_BUDGET = 6000;
const CHARS_PER_TOKEN = 4;

export function approxTokens(text: string): number {
  return Math.ceil(text.length / CHARS_PER_TOKEN);
}

/**
 * Dedent + tidy a section body so indentation from template literals is never
 * shipped to the model as tokens.
 */
export function normalizePromptText(text: string): string {
  const lines = text.replace(/\r\n/g, '\n').split('\n');

  // Find the common leading indentation across non-empty lines.
  let minIndent = Infinity;
  for (const line of lines) {
    if (line.trim().length === 0) continue;
    const indent = line.length - line.trimStart().length;
    minIndent = Math.min(minIndent, indent);
  }
  const dedented = lines.map((line) =>
    line.trim().length === 0 ? '' : line.slice(Number.isFinite(minIndent) ? minIndent : 0),
  );

  return dedented
    .join('\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

function truncateToTokens(text: string, maxTokens: number): string {
  const maxChars = maxTokens * CHARS_PER_TOKEN;
  if (text.length <= maxChars) {
    return text;
  }
  return `${text.slice(0, maxChars)}\n…[trimmed to fit prompt budget]`;
}

export function composeSystemPrompt(
  sections: PromptSection[],
  options?: ComposeOptions,
): string {
  const budget = options?.totalBudgetTokens ?? DEFAULT_TOTAL_BUDGET;

  const prepared = sections
    .map((section) => ({
      ...section,
      content: normalizePromptText(section.content),
    }))
    .filter((section) => section.content.length > 0)
    .map((section) =>
      section.maxTokens
        ? { ...section, content: truncateToTokens(section.content, section.maxTokens) }
        : section,
    );

  // Stable prefix first (cacheable), volatile tail last — original order
  // preserved within each group.
  const ordered = [
    ...prepared.filter((section) => section.stability === 'stable'),
    ...prepared.filter((section) => section.stability === 'volatile'),
  ];

  // Budget pass: drop volatile sections lowest-priority-first until we fit.
  let total = ordered.reduce((sum, section) => sum + approxTokens(section.content), 0);
  if (total > budget) {
    const dropOrder = [...ordered]
      .filter((section) => section.stability === 'volatile')
      .sort((a, b) => a.priority - b.priority);
    const dropped = new Set<string>();
    for (const candidate of dropOrder) {
      if (total <= budget) break;
      dropped.add(candidate.id);
      total -= approxTokens(candidate.content);
    }
    return ordered
      .filter((section) => !dropped.has(section.id))
      .map((section) => section.content)
      .join('\n\n');
  }

  return ordered.map((section) => section.content).join('\n\n');
}
