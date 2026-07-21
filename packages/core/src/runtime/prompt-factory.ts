/**
 * System prompt factory — the single place where a turn's system prompt is
 * assembled. Callers hand over raw ingredients (persona, lifecycle prompt,
 * tool cards, immediate context, summary, memories, intent) and the factory
 * decides ordering, stability, priorities, and token budgets via the
 * composer (see prompt-composer.ts for why ordering/budgets matter).
 */

import { composeSystemPrompt, type ComposeOptions } from './prompt-composer.js';

export type SystemPromptIngredients = {
  persona: string;
  toolGuidance: string;
  /** Harness capability brief ('' when the harness is off). */
  brief?: string;
  lifecyclePrompt?: string;
  /** Tool prompt cards for this turn ('' when the harness is off). */
  toolCards?: string;
  /** Semantic retrieval decision: intent, pathway, relevant flow/policies/tools. */
  pathway?: string;
  /** Archetype valence stance: inferred tone/posture guidance for this message. */
  stance?: string;
  /** Temporal scope resolved for this turn. */
  temporal?: string;
  /** Unified high-evidence context items (axis recall, memories, stance). */
  evidence?: string;
  /** Immediate Context Engine block — time-bucketed short-term context. */
  immediateContext?: string;
  /** Rolling per-session summary. */
  summary?: string;
  /** Recalled long-term memories, already rendered. */
  memories?: string;
  /** Intent stack prompt. */
  intent?: string;
};

export function buildTurnSystemPrompt(
  ingredients: SystemPromptIngredients,
  options?: ComposeOptions,
): string {
  return composeSystemPrompt(
    [
      { id: 'persona', content: ingredients.persona, stability: 'stable', priority: 100, maxTokens: 800 },
      { id: 'guidance', content: ingredients.toolGuidance, stability: 'stable', priority: 95 },
      { id: 'brief', content: ingredients.brief ?? '', stability: 'stable', priority: 92, maxTokens: 1500 },
      { id: 'lifecycle', content: ingredients.lifecyclePrompt ?? '', stability: 'stable', priority: 90, maxTokens: 1200 },
      {
        id: 'pathway',
        content: ingredients.pathway ?? '',
        stability: 'volatile',
        priority: 75,
        maxTokens: 500,
      },
      {
        id: 'stance',
        content: ingredients.stance ?? '',
        stability: 'volatile',
        priority: 72,
        maxTokens: 400,
      },
      {
        id: 'temporal',
        content: ingredients.temporal ?? '',
        stability: 'volatile',
        priority: 71,
        maxTokens: 200,
      },
      {
        id: 'evidence',
        content: ingredients.evidence ?? '',
        stability: 'volatile',
        priority: 68,
        maxTokens: 900,
      },
      {
        id: 'tools',
        content: ingredients.toolCards ? `Available tools for this turn:\n${ingredients.toolCards}` : '',
        stability: 'volatile',
        priority: 70,
        maxTokens: 1500,
      },
      // Immediate context outranks the session summary: it is fresher and
      // spans sessions/channels, whereas the summary is per-session history.
      { id: 'immediate_context', content: ingredients.immediateContext ?? '', stability: 'volatile', priority: 65, maxTokens: 900 },
      {
        id: 'summary',
        content: ingredients.summary ? `Rolling session summary:\n${ingredients.summary}` : '',
        stability: 'volatile',
        priority: 60,
        maxTokens: 600,
      },
      { id: 'memories', content: ingredients.memories ?? '', stability: 'volatile', priority: 50, maxTokens: 600 },
      { id: 'intent', content: ingredients.intent ?? '', stability: 'volatile', priority: 40, maxTokens: 400 },
    ],
    options,
  );
}
