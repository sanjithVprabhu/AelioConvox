/**
 * Temporal resolver — deterministic linguistic + recency scoping.
 *
 * Produces a TemporalScope that bounds Harness Axis traversal and evidence age
 * decay. Never calls an LLM: regex + rules over the message and last-seen time.
 */

export type TemporalTier =
  | 'hot'
  | 'recent'
  | 'today'
  | 'historical'
  | 'long_term';

export type TemporalScope = {
  /** How the window was chosen. */
  mode: 'explicit' | 'recency' | 'default';
  /** Inclusive lower bound (epoch ms). */
  from: number;
  /** Inclusive upper bound (epoch ms). */
  to: number;
  /** Soft labels covering the window (for prompt rendering). */
  tiers: TemporalTier[];
  /** 0–1 confidence in the resolved window. */
  confidence: number;
  /** Human-readable reason for the journal / prompt. */
  reason: string;
};

export type TemporalResolveInput = {
  message: string;
  /** When the customer last spoke (epoch ms). Defaults to `now`. */
  lastMessageAt?: number | null;
  /** Clock override for tests. */
  now?: number;
};

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

const EXPLICIT_RULES: Array<{
  pattern: RegExp;
  windowMs: () => { fromOffset: number; toOffset: number };
  tiers: TemporalTier[];
  reason: string;
  confidence: number;
}> = [
  {
    pattern: /\b(a few minutes ago|just now|moments? ago|a minute ago)\b/i,
    windowMs: () => ({ fromOffset: -15 * MINUTE, toOffset: 0 }),
    tiers: ['hot', 'recent'],
    reason: 'explicit: a few minutes ago',
    confidence: 0.95,
  },
  {
    pattern: /\b(earlier (today|this morning|this afternoon)|this morning|this afternoon)\b/i,
    windowMs: () => ({ fromOffset: -DAY, toOffset: 0 }),
    tiers: ['today', 'recent'],
    reason: 'explicit: earlier today',
    confidence: 0.9,
  },
  {
    pattern: /\byesterday\b/i,
    windowMs: () => ({ fromOffset: -2 * DAY, toOffset: -DAY + HOUR }),
    tiers: ['historical', 'today'],
    reason: 'explicit: yesterday',
    confidence: 0.92,
  },
  {
    pattern: /\b(last week|a week ago|past week)\b/i,
    windowMs: () => ({ fromOffset: -14 * DAY, toOffset: -5 * DAY }),
    tiers: ['historical'],
    reason: 'explicit: last week',
    confidence: 0.9,
  },
  {
    pattern: /\b(last month|a month ago|past month)\b/i,
    windowMs: () => ({ fromOffset: -45 * DAY, toOffset: -20 * DAY }),
    tiers: ['historical', 'long_term'],
    reason: 'explicit: last month',
    confidence: 0.85,
  },
  {
    pattern: /\b(long ago|ages ago|months? ago|a while (ago|back))\b/i,
    windowMs: () => ({ fromOffset: -180 * DAY, toOffset: -30 * DAY }),
    tiers: ['long_term'],
    reason: 'explicit: long ago',
    confidence: 0.8,
  },
];

function tierForAge(ageMs: number): TemporalTier {
  if (ageMs < 5 * MINUTE) return 'hot';
  if (ageMs < 30 * MINUTE) return 'recent';
  if (ageMs < DAY) return 'today';
  if (ageMs < 30 * DAY) return 'historical';
  return 'long_term';
}

function windowForRecency(ageMs: number): { fromOffset: number; tiers: TemporalTier[]; reason: string } {
  if (ageMs < 5 * MINUTE) {
    return { fromOffset: -30 * MINUTE, tiers: ['hot', 'recent'], reason: 'recency: conversation is hot' };
  }
  if (ageMs < 30 * MINUTE) {
    return { fromOffset: -2 * HOUR, tiers: ['recent', 'today'], reason: 'recency: spoken within 30m' };
  }
  if (ageMs < DAY) {
    return { fromOffset: -2 * DAY, tiers: ['today', 'historical'], reason: 'recency: spoken today' };
  }
  if (ageMs < 30 * DAY) {
    return { fromOffset: -45 * DAY, tiers: ['historical'], reason: 'recency: spoken this month' };
  }
  return { fromOffset: -180 * DAY, tiers: ['long_term', 'historical'], reason: 'recency: long gap since last contact' };
}

/**
 * Resolve the temporal window for this turn.
 * Explicit linguistic cues win; otherwise fall back to conversation recency;
 * otherwise a default 7-day lookback.
 */
export function resolveTemporalScope(input: TemporalResolveInput): TemporalScope {
  const now = input.now ?? Date.now();
  const message = input.message.trim();

  for (const rule of EXPLICIT_RULES) {
    if (rule.pattern.test(message)) {
      const { fromOffset, toOffset } = rule.windowMs();
      return {
        mode: 'explicit',
        from: now + fromOffset,
        to: now + toOffset,
        tiers: rule.tiers,
        confidence: rule.confidence,
        reason: rule.reason,
      };
    }
  }

  if (input.lastMessageAt != null && Number.isFinite(input.lastMessageAt)) {
    const ageMs = Math.max(0, now - input.lastMessageAt);
    const { fromOffset, tiers, reason } = windowForRecency(ageMs);
    return {
      mode: 'recency',
      from: now + fromOffset,
      to: now,
      tiers,
      confidence: 0.75,
      reason: `${reason} (last seen ${tierForAge(ageMs)})`,
    };
  }

  return {
    mode: 'default',
    from: now - 7 * DAY,
    to: now,
    tiers: ['hot', 'recent', 'today', 'historical'],
    confidence: 0.5,
    reason: 'default: 7-day lookback',
  };
}

/** True when a timestamp falls inside the scope window. */
export function inTemporalScope(timestampMs: number, scope: TemporalScope): boolean {
  return timestampMs >= scope.from && timestampMs <= scope.to;
}

/** Age-decay factor in [0, 1] with a half-life (default 3 days). */
export function temporalRelevance(
  timestampMs: number,
  nowMs: number,
  halfLifeMs = 3 * DAY,
): number {
  const age = Math.max(0, nowMs - timestampMs);
  return Math.exp((-Math.LN2 * age) / Math.max(1, halfLifeMs));
}

export function renderTemporalScope(scope: TemporalScope): string {
  return `TEMPORAL SCOPE: ${scope.mode} — ${scope.reason} (tiers: ${scope.tiers.join(', ')}, confidence ${scope.confidence.toFixed(2)}). Prefer context inside this window.`;
}
