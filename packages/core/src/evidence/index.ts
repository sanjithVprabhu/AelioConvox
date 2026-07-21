/**
 * Unified evidence scorer — arithmetic over typed ContextItems.
 *
 * Weights and thresholds live in a versioned config object so replay can retune
 * without code changes. Hard policies never go through this filter.
 */

import { temporalRelevance } from '../temporal/index.js';
import type { AxisOccurrence } from '../storage/axis.js';
import type { MemoryRecallHit } from '../storage/memories.js';
import type { CategoryAssessment } from '../archetype/index.js';

export type EvidenceSource =
  | 'axis_occurrence'
  | 'memory'
  | 'stance'
  | 'pathway_tool'
  | 'pathway_flow';

export type EvidenceItem = {
  id: string;
  source: EvidenceSource;
  /** Human-readable content for the prompt. */
  content: string;
  /** Why this was included (for journal / debugging). */
  why: string;
  score: number;
  timestampMs?: number;
  meta?: Record<string, unknown>;
};

export type EvidenceWeights = {
  semantic: number;
  atomConfidence: number;
  temporal: number;
  sameAxisContinuity: number;
  activeFlow: number;
  lexical: number;
};

export type EvidenceConfig = {
  version: string;
  weights: EvidenceWeights;
  /** Minimum score to enter the prompt. */
  threshold: number;
  /** Age half-life for temporalRelevance (ms). Default 3 days. */
  halfLifeMs: number;
  /** Max items fed into the prompt. */
  maxItems: number;
};

export const DEFAULT_EVIDENCE_CONFIG: EvidenceConfig = {
  version: 'evidence-v1',
  weights: {
    semantic: 0.35,
    atomConfidence: 0.2,
    temporal: 0.15,
    sameAxisContinuity: 0.15,
    activeFlow: 0.1,
    lexical: 0.05,
  },
  threshold: 0.35,
  halfLifeMs: 3 * 24 * 60 * 60 * 1000,
  maxItems: 8,
};

function clamp01(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(1, value));
}

export function scoreEvidenceParts(input: {
  semantic?: number;
  atomConfidence?: number;
  temporal?: number;
  sameAxisContinuity?: number;
  activeFlow?: number;
  lexical?: number;
  config?: EvidenceConfig;
}): number {
  const cfg = input.config ?? DEFAULT_EVIDENCE_CONFIG;
  const w = cfg.weights;
  return clamp01(
    w.semantic * clamp01(input.semantic ?? 0) +
      w.atomConfidence * clamp01(input.atomConfidence ?? 0) +
      w.temporal * clamp01(input.temporal ?? 0) +
      w.sameAxisContinuity * clamp01(input.sameAxisContinuity ?? 0) +
      w.activeFlow * clamp01(input.activeFlow ?? 0) +
      w.lexical * clamp01(input.lexical ?? 0),
  );
}

export function evidenceFromAxisOccurrence(
  occ: AxisOccurrence,
  options: {
    now: number;
    activeFlowId?: string | null;
    config?: EvidenceConfig;
  },
): EvidenceItem {
  const cfg = options.config ?? DEFAULT_EVIDENCE_CONFIG;
  const temporal = temporalRelevance(occ.createdAt, options.now, cfg.halfLifeMs);
  const activeFlow =
    options.activeFlowId && occ.flowId && options.activeFlowId === occ.flowId ? 1 : 0;
  const score = scoreEvidenceParts({
    semantic: occ.score,
    atomConfidence: occ.strength,
    temporal,
    sameAxisContinuity: 1,
    activeFlow,
    config: cfg,
  });
  const spanBit = occ.span ? ` span="${occ.span}"` : '';
  return {
    id: `axis:${occ.occurrenceId}`,
    source: 'axis_occurrence',
    content: `${occ.aspectName}:${occ.valence} (${occ.strength.toFixed(2)})${spanBit} — prior at ${new Date(occ.createdAt).toISOString()}${
      occ.intentLabel ? `; intent ${occ.intentLabel}` : ''
    }`,
    why: `same-axis continuity score=${score.toFixed(2)} temporal=${temporal.toFixed(2)}`,
    score,
    timestampMs: occ.createdAt,
    meta: {
      aspectId: occ.aspectId,
      valence: occ.valence,
      turnId: occ.turnId,
    },
  };
}

export function evidenceFromMemory(
  memory: MemoryRecallHit,
  options: { now: number; config?: EvidenceConfig },
): EvidenceItem {
  const cfg = options.config ?? DEFAULT_EVIDENCE_CONFIG;
  const score = scoreEvidenceParts({
    semantic: memory.score,
    atomConfidence: memory.score,
    temporal: 0.7,
    config: cfg,
  });
  return {
    id: `memory:${memory.id}`,
    source: 'memory',
    content: memory.content,
    why: `memory recall cosine=${memory.score.toFixed(3)} evidence=${score.toFixed(2)}`,
    score,
  };
}

export function evidenceFromStance(
  category: CategoryAssessment,
  options: { config?: EvidenceConfig } = {},
): EvidenceItem | null {
  if (!category.fed) return null;
  const cfg = options.config ?? DEFAULT_EVIDENCE_CONFIG;
  const score = scoreEvidenceParts({
    semantic: category.strength,
    atomConfidence: category.margin,
    temporal: 1,
    sameAxisContinuity: 0.5,
    config: cfg,
  });
  const spanBit = category.span ? ` [from: ${category.span}]` : '';
  return {
    id: `stance:${category.category}`,
    source: 'stance',
    content: `${category.category}: ${category.dominant} — ${category.guidance}${spanBit}`,
    why: `stance strength=${category.strength.toFixed(2)} margin=${category.margin.toFixed(2)}`,
    score,
    meta: {
      positive: category.positive,
      negative: category.negative,
      neutral: category.neutral,
    },
  };
}

/** Filter, rank, and cap evidence items using the configured threshold. */
export function selectEvidence(
  items: EvidenceItem[],
  config: EvidenceConfig = DEFAULT_EVIDENCE_CONFIG,
): EvidenceItem[] {
  return items
    .filter((item) => item.score >= config.threshold)
    .sort((a, b) => b.score - a.score)
    .slice(0, config.maxItems);
}

export function renderEvidenceBlock(items: EvidenceItem[]): string {
  if (items.length === 0) return '';
  const lines = [
    'RELEVANT EVIDENCE (high-confidence only; each item carries its why):',
    ...items.map(
      (item) =>
        `- [${item.source} ${item.score.toFixed(2)}] ${item.content} (${item.why})`,
    ),
  ];
  return lines.join('\n');
}
