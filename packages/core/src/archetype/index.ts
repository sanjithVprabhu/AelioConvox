/**
 * Archetype valence engine.
 *
 * For each conversational category (sentiment, engagement, certainty, urgency,
 * …) three valence buckets — positive / negative / neutral — live in AelioDb as
 * embedded exemplars. An incoming message vector is matched against every
 * bucket; the recomputed-cosine scores become per-category positive/negative/
 * neutral numbers. The dominant, confident buckets contribute their `guidance`
 * to the system prompt so the model knows both WHAT posture to take and HOW to
 * respond. Weak or ambiguous signals are withheld (fed = false) so the prompt
 * only carries stances the retrieval is actually confident about.
 *
 * This is deliberately advisory: it shapes tone/strategy, never safety. Hard
 * policies, confirmation, and lifecycle gates stay deterministic elsewhere.
 */

import type { LLMProvider } from '@aelio/llm';
import { embed } from '../analyst/embeddings.js';
import { discoverAspects, type AspectDiscoveryResult } from '../analyst/aspects.js';
import type {
  ArchetypeExemplar,
  ArchetypeMatch,
  ArchetypeValence,
  ConvoxArchetypeStore,
} from '../storage/archetypes.js';
import type { ConvoxAspectStore } from '../storage/aspects.js';

export type CategoryAssessment = {
  category: string;
  /** Mother-collection aspect id when known ('' for legacy unlinked buckets). */
  aspectId?: string;
  positive: number;
  negative: number;
  neutral: number;
  dominant: ArchetypeValence;
  /** Top valence score for the category. */
  strength: number;
  /** Gap between the top and second valence — the confidence signal. */
  margin: number;
  /** The winning bucket's response guidance. */
  guidance: string;
  /** Whether this category cleared the thresholds to enter the prompt. */
  fed: boolean;
  /** The message span the winning bucket matched (absent = whole message). */
  span?: string;
};

/**
 * Split a message into independently-matchable spans (sentences/clauses), so
 * "the dashboard is great, but my invoice is wrong AGAIN" reads as a positive
 * span AND a negative span instead of an averaged-away blur. Returns [] when
 * the message is effectively a single span (callers use the whole message).
 */
const MAX_SPANS = 6;
export function splitSpans(message: string): string[] {
  const spans = message
    .split(/(?<=[.!?;])\s+|\n+/)
    .map((span) => span.trim())
    .filter((span) => span.length >= 3);
  return spans.length > 1 ? spans.slice(0, MAX_SPANS) : [];
}

export type ArchetypeAssessment = {
  categories: CategoryAssessment[];
  /** Net stance across fed categories: >0 positive-leaning, <0 negative-leaning. */
  overall: { valence: ArchetypeValence; score: number };
  /** Rendered prompt block ('' when nothing cleared the feed threshold). */
  prompt: string;
  timings: { totalMs: number };
};

export type ArchetypeEngineOptions = {
  store: ConvoxArchetypeStore;
  /** Mother collection registry — enables self-learning aspect discovery. */
  aspectStore?: ConvoxAspectStore;
  /** Candidate rows pulled from AelioDb per assessment. Default 40. */
  k?: number;
  /** A bucket below this cosine is ignored entirely. Default 0.20. */
  minScore?: number;
  /** A category is fed to the prompt only when strength ≥ this. Default 0.28. */
  feedThreshold?: number;
  /** …and its winning margin over the next valence is ≥ this. Default 0.04. */
  marginThreshold?: number;
  /** Max categories fed into the prompt. Default 4. */
  maxCategories?: number;
};

export type ArchetypeAssessInput = {
  message: string;
  /** Reuse the pathway/turn message vector; embedded here only if omitted. */
  queryVector?: number[];
};

type SpanMatch = ArchetypeMatch & { span?: string };

function pickDominant(scores: Record<ArchetypeValence, number>): {
  dominant: ArchetypeValence;
  strength: number;
  margin: number;
} {
  const entries = (Object.entries(scores) as Array<[ArchetypeValence, number]>).sort(
    (a, b) => b[1] - a[1],
  );
  const top = entries[0] ?? (['neutral', 0] as [ArchetypeValence, number]);
  const second = entries[1];
  return {
    dominant: top[0],
    strength: top[1],
    margin: top[1] - (second?.[1] ?? 0),
  };
}

export class ArchetypeEngine {
  private readonly store: ConvoxArchetypeStore;
  private readonly aspectStore?: ConvoxAspectStore;
  private readonly k: number;
  private readonly minScore: number;
  private readonly feedThreshold: number;
  private readonly marginThreshold: number;
  private readonly maxCategories: number;

  constructor(options: ArchetypeEngineOptions) {
    this.store = options.store;
    this.aspectStore = options.aspectStore;
    this.k = options.k ?? 40;
    this.minScore = options.minScore ?? 0.2;
    this.feedThreshold = options.feedThreshold ?? 0.28;
    this.marginThreshold = options.marginThreshold ?? 0.04;
    this.maxCategories = options.maxCategories ?? 4;
  }

  /**
   * Self-learning pass: discover new conversational aspects from a message and
   * grow the taxonomy. No-op without an aspect store. Runs post-turn and never
   * throws — the caller fires it and forgets it.
   */
  async learn(input: {
    message: string;
    queryVector?: number[];
    llm: LLMProvider;
    model: string;
    maxTokens: number;
  }): Promise<AspectDiscoveryResult | null> {
    if (!this.aspectStore) return null;
    try {
      return await discoverAspects({
        llm: input.llm,
        model: input.model,
        maxTokens: input.maxTokens,
        message: input.message,
        queryVector: input.queryVector,
        aspectStore: this.aspectStore,
        archetypeStore: this.store,
      });
    } catch (error) {
      console.warn(
        `[aelio] aspect discovery skipped: ${error instanceof Error ? error.message : String(error)}`,
      );
      return null;
    }
  }

  async assess(input: ArchetypeAssessInput): Promise<ArchetypeAssessment> {
    const started = performance.now();
    const queryVector =
      input.queryVector ?? (await embed(input.message, { purpose: 'pathway_retrieval' }));

    const spans = splitSpans(input.message);
    let matches: SpanMatch[];
    if (spans.length === 0) {
      matches = await this.store.matchByVector(queryVector, this.k, {
        text: input.message,
      });
    } else {
      // Span decomposition: match every sentence/clause independently and keep
      // each bucket's best span, so mixed messages don't average away signals.
      // Span embeds are memoized and the AelioDb queries fan out in parallel.
      // Hybrid BM25+vector is used so exact keywords (e.g. "Pro Max") survive.
      const spanVectors = await Promise.all(
        spans.map((span) => embed(span, { purpose: 'pathway_retrieval' })),
      );
      const waves = await Promise.all([
        this.store.matchByVector(queryVector, this.k, { text: input.message }),
        ...spanVectors.map((vector, index) =>
          this.store.matchByVector(vector, this.k, { text: spans[index] }),
        ),
      ]);
      // Near-ties go to the span: when a single clause explains the signal as
      // well as the whole message (within noise), the clause is the more
      // precise attribution — that's the evidence the journal should name.
      const SPAN_ATTRIBUTION_EPSILON = 0.01;
      const best = new Map<string, SpanMatch>();
      waves.forEach((wave, index) => {
        const span = index === 0 ? undefined : spans[index - 1];
        for (const match of wave) {
          const prev = best.get(match.id);
          const wins = !prev
            ? true
            : span !== undefined && prev.span === undefined
              ? match.score >= prev.score - SPAN_ATTRIBUTION_EPSILON
              : match.score > prev.score;
          if (wins) {
            best.set(match.id, span === undefined ? match : { ...match, span });
          }
        }
      });
      matches = [...best.values()];
    }

    // Staging: buckets born from DISCOVERED aspects only feed once their aspect
    // is promoted to 'active' (hits or admin approval). Builtin aspects seed as
    // active; rows without an aspect_id (legacy seeds) are always allowed.
    const allowed = await this.activeAspectIds();
    if (allowed) {
      matches = matches.filter((match) => match.aspectId === '' || allowed.has(match.aspectId));
    }

    const assessment = this.aggregate(matches);
    return { ...assessment, timings: { totalMs: performance.now() - started } };
  }

  private aspectCache: { ids: Set<string>; at: number } | null = null;

  /** Active aspect ids, cached 30s. Null = no aspect store, no staging filter. */
  private async activeAspectIds(): Promise<Set<string> | null> {
    if (!this.aspectStore) return null;
    const now = Date.now();
    if (this.aspectCache && now - this.aspectCache.at < 30_000) {
      return this.aspectCache.ids;
    }
    try {
      const records = await this.aspectStore.list();
      const ids = new Set(
        records.filter((record) => record.status === 'active').map((record) => record.id),
      );
      this.aspectCache = { ids, at: now };
      return ids;
    } catch {
      // AelioDb hiccup: keep the last known set rather than dropping all stance.
      return this.aspectCache?.ids ?? null;
    }
  }

  /** Never fails a turn: on error, returns an empty (no-op) assessment. */
  async assessSafe(input: ArchetypeAssessInput): Promise<ArchetypeAssessment> {
    try {
      return await this.assess(input);
    } catch (error) {
      console.warn(
        `[aelio] archetype assessment unavailable: ${error instanceof Error ? error.message : String(error)}`,
      );
      return {
        categories: [],
        overall: { valence: 'neutral', score: 0 },
        prompt: '',
        timings: { totalMs: 0 },
      };
    }
  }

  private aggregate(matches: SpanMatch[]): Omit<ArchetypeAssessment, 'timings'> {
    // Best (max cosine) bucket per (category, valence). Max is more stable than
    // sum: it answers "does the message resemble this bucket?" without letting
    // exemplar count skew a category.
    const byCategory = new Map<
      string,
      {
        aspectId: string;
        scores: Record<ArchetypeValence, number>;
        guidance: Record<ArchetypeValence, string>;
        spans: Record<ArchetypeValence, string | undefined>;
      }
    >();

    for (const match of matches) {
      if (match.score < this.minScore) continue;
      const entry =
        byCategory.get(match.category) ??
        {
          aspectId: match.aspectId ?? '',
          scores: { positive: 0, negative: 0, neutral: 0 },
          guidance: { positive: '', negative: '', neutral: '' },
          spans: { positive: undefined, negative: undefined, neutral: undefined } as Record<
            ArchetypeValence,
            string | undefined
          >,
        };
      if (match.aspectId) entry.aspectId = match.aspectId;
      if (match.score > entry.scores[match.valence]) {
        entry.scores[match.valence] = match.score;
        entry.guidance[match.valence] = match.guidance;
        entry.spans[match.valence] = match.span;
      }
      byCategory.set(match.category, entry);
    }

    const categories: CategoryAssessment[] = [];
    for (const [category, entry] of byCategory) {
      const { dominant, strength, margin } = pickDominant(entry.scores);
      const fed = strength >= this.feedThreshold && margin >= this.marginThreshold;
      categories.push({
        category,
        aspectId: entry.aspectId || undefined,
        positive: entry.scores.positive,
        negative: entry.scores.negative,
        neutral: entry.scores.neutral,
        dominant,
        strength,
        margin,
        guidance: entry.guidance[dominant],
        fed,
        ...(entry.spans[dominant] !== undefined ? { span: entry.spans[dominant] } : {}),
      });
    }

    categories.sort((a, b) => b.strength - a.strength);

    const fedCategories = categories.filter((c) => c.fed).slice(0, this.maxCategories);

    let net = 0;
    for (const category of fedCategories) {
      if (category.dominant === 'positive') net += category.strength;
      else if (category.dominant === 'negative') net -= category.strength;
    }
    const overallValence: ArchetypeValence =
      net > 0.05 ? 'positive' : net < -0.05 ? 'negative' : 'neutral';

    return {
      categories,
      overall: { valence: overallValence, score: net },
      prompt: this.render(fedCategories, overallValence),
    };
  }

  private render(fed: CategoryAssessment[], overall: ArchetypeValence): string {
    if (fed.length === 0) {
      return '';
    }
    const lines = [
      'CONVERSATIONAL STANCE (inferred from this message; use it to choose tone and ' +
        'how to respond — the user still drives the goal):',
      `Overall read: ${overall}.`,
    ];
    for (const category of fed) {
      lines.push(
        `- ${category.category}: ${category.dominant} ` +
          `(pos ${category.positive.toFixed(2)}, neg ${category.negative.toFixed(2)}, ` +
          `neu ${category.neutral.toFixed(2)}). ${category.guidance}`,
      );
    }
    return lines.join('\n');
  }
}

export function createArchetypeEngine(options: ArchetypeEngineOptions): ArchetypeEngine {
  return new ArchetypeEngine(options);
}

/** Human-readable dimension descriptions for the builtin (seeded) aspects. */
const BUILTIN_ASPECT_DESCRIPTIONS: Record<string, string> = {
  sentiment: 'The emotional affect of the user — pleased, upset, or matter-of-fact.',
  engagement: 'Whether the user wants to keep going, is winding down, or is undecided.',
  certainty: 'How confident or clear the user is versus confused or still deciding.',
  urgency: 'How time-pressured the request is — urgent, relaxed, or neutral.',
};

/**
 * Seed the builtin aspects into the mother collection and their pos/neu/neg
 * buckets (linked by aspect_id) into the archetypes table. Idempotent: does
 * nothing once the registry already holds aspects. This is the bridge that
 * makes the hardcoded defaults the first citizens of the self-learning taxonomy.
 */
export async function seedBuiltinArchetypes(
  aspectStore: ConvoxAspectStore,
  archetypeStore: ConvoxArchetypeStore,
  exemplars: ArchetypeExemplar[] = DEFAULT_ARCHETYPES,
): Promise<number> {
  if ((await aspectStore.count()) > 0) return 0;

  const byCategory = new Map<string, ArchetypeExemplar[]>();
  for (const exemplar of exemplars) {
    const bucket = byCategory.get(exemplar.category) ?? [];
    bucket.push(exemplar);
    byCategory.set(exemplar.category, bucket);
  }

  let created = 0;
  for (const [category, buckets] of byCategory) {
    const aspect = await aspectStore.register({
      name: category,
      description: BUILTIN_ASPECT_DESCRIPTIONS[category] ?? `Conversational aspect: ${category}.`,
      status: 'active',
      source: 'builtin',
    });
    await archetypeStore.insertAspectBuckets(aspect.id, buckets);
    created += 1;
  }
  return created;
}

/**
 * Product-agnostic starter archetypes. A tenant can replace these with their
 * own vocabulary; they exist so the engine is useful out of the box and the
 * tests have data. Guidance is written as direct instruction to the model.
 */
export const DEFAULT_ARCHETYPES: ArchetypeExemplar[] = [
  // ---- sentiment ----
  {
    category: 'sentiment',
    valence: 'positive',
    keyword: 'happy satisfied grateful pleased',
    description: 'The user is pleased, thankful, or expresses satisfaction.',
    usage: 'thanks, this is great, perfect, exactly what I needed, love it',
    inference: 'Positive affect words, praise, gratitude, exclamation of success.',
    guidance: 'Match their warmth briefly, reinforce the good outcome, and offer a natural next step.',
  },
  {
    category: 'sentiment',
    valence: 'negative',
    keyword: 'angry frustrated upset disappointed annoyed',
    description: 'The user is unhappy, frustrated, or complaining.',
    usage: 'this is broken, terrible, still not working, fed up, unacceptable',
    inference: 'Complaint language, negative affect, repeated failed attempts, blame.',
    guidance: 'Lead with a brief, sincere acknowledgement, avoid defensiveness, and move straight to a concrete fix.',
  },
  {
    category: 'sentiment',
    valence: 'neutral',
    keyword: 'okay fine neutral informational',
    description: 'The user is matter-of-fact with no strong affect.',
    usage: 'can you tell me, what is, how do I, I need to',
    inference: 'Plain informational request without emotional markers.',
    guidance: 'Stay concise and factual; do not manufacture emotion.',
  },
  // ---- engagement ----
  {
    category: 'engagement',
    valence: 'positive',
    keyword: 'continue more tell me curious interested',
    description: 'The user wants to keep going and explore further.',
    usage: 'go on, what else, tell me more, and then, sounds good what next',
    inference: 'Follow-up questions, forward-looking phrasing, enthusiasm to proceed.',
    guidance: 'Keep momentum: answer, then proactively surface the most useful next option.',
  },
  {
    category: 'engagement',
    valence: 'negative',
    keyword: 'bye done stop leave goodbye later',
    description: 'The user wants to end or pause the conversation.',
    usage: 'that is all, goodbye, I am done, talk later, stop messaging me',
    inference: 'Closure language, dismissal, explicit stop requests.',
    guidance: 'Close warmly and briefly. Do not open new topics or push further engagement; invite them back on their terms.',
  },
  {
    category: 'engagement',
    valence: 'neutral',
    keyword: 'thinking considering maybe',
    description: 'The user is neither clearly continuing nor leaving.',
    usage: 'let me think, maybe, not sure yet, I will see',
    inference: 'Hesitation, deferral, non-committal phrasing.',
    guidance: 'Give them room; offer a small, low-pressure next step without demanding a decision.',
  },
  // ---- certainty ----
  {
    category: 'certainty',
    valence: 'positive',
    keyword: 'sure definitely decided confirm yes',
    description: 'The user is confident and has decided.',
    usage: 'yes do it, go ahead, that one, confirmed, I am sure',
    inference: 'Decisive language, explicit confirmation, clear choice.',
    guidance: 'Act decisively on their choice; do not re-litigate settled decisions.',
  },
  {
    category: 'certainty',
    valence: 'negative',
    keyword: 'confused unsure lost dont understand',
    description: 'The user is confused or uncertain.',
    usage: 'I do not understand, what do you mean, I am lost, which one, huh',
    inference: 'Clarifying questions, contradiction, signs of misunderstanding.',
    guidance: 'Slow down, clarify one thing at a time in plain language, and confirm understanding before proceeding.',
  },
  {
    category: 'certainty',
    valence: 'neutral',
    keyword: 'asking exploring comparing',
    description: 'The user is gathering information to decide.',
    usage: 'what are the options, how does this compare, what happens if',
    inference: 'Comparative or exploratory questions without a decision yet.',
    guidance: 'Lay out options clearly and neutrally; help them decide without pushing one.',
  },
  // ---- urgency ----
  {
    category: 'urgency',
    valence: 'positive',
    keyword: 'urgent now asap immediately emergency',
    description: 'The user needs something quickly.',
    usage: 'right now, asap, urgent, this is time sensitive, immediately',
    inference: 'Time pressure words, deadlines, escalation.',
    guidance: 'Prioritize the fastest path to resolution; be direct and skip non-essential detail.',
  },
  {
    category: 'urgency',
    valence: 'negative',
    keyword: 'whenever no rush relaxed',
    description: 'The user is in no hurry.',
    usage: 'no rush, whenever you can, just checking, take your time',
    inference: 'Explicitly low time pressure.',
    guidance: 'You can be thorough; no need to compress or rush the answer.',
  },
  {
    category: 'urgency',
    valence: 'neutral',
    keyword: 'normal routine',
    description: 'No particular time signal.',
    usage: 'general request with no timing cue',
    inference: 'Absence of urgency or relaxation markers.',
    guidance: 'Respond at a normal, efficient pace.',
  },
];
