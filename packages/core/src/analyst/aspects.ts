import type { LLMProvider } from '@aelio/llm';
import { embed } from './embeddings.js';
import type {
  ArchetypeExemplar,
  ArchetypeValence,
} from '../storage/archetypes.js';
import type { ConvoxArchetypeStore } from '../storage/archetypes.js';
import type { ConvoxAspectStore } from '../storage/aspects.js';

/**
 * Aspect Discovery — the "self" part of the self-learning stance layer.
 *
 * On a turn whose message is NOT already covered by a known aspect, one bounded
 * LLM pass names the new conversational "thing(s)" worth tracking and drafts
 * their positive/neutral/negative buckets. New aspects are registered in the
 * mother collection and their buckets written to the archetypes table, so from
 * the very next turn the (fast, vector-only) archetype engine can detect and
 * feed them. Aspects already present in the message are simply "touched"
 * (hit count / promotion) with no LLM cost.
 *
 * This runs post-turn, fire-and-forget: it never blocks or fails a reply.
 */

const DISCOVERY_SYSTEM = `You expand a conversational-intelligence taxonomy. Given ONE user message, identify GENERAL, reusable conversational "aspects" it exhibits — dimensions of tone, attitude, concern, or intent that will recur across many future conversations (e.g. "price_sensitivity", "trust_in_brand", "technical_confidence", "decision_readiness"). Do NOT capture one-off facts, names, or order numbers — those are memories, not aspects.

Only propose an aspect if it is genuinely useful to track over time AND not already in the provided known-aspects list. It is correct and common to return an empty list.

For each new aspect, define how it appears as positive, neutral, and negative, and how the assistant should respond in each case.

Respond with ONLY a JSON object, no prose, in this exact shape:
{"aspects":[{"name":"snake_case_name","description":"what this dimension is","positive":{"keyword":"few words","description":"...","usage":"phrases that convey it","inference":"how it is inferred","guidance":"how the assistant should respond"},"neutral":{...same 5 fields...},"negative":{...same 5 fields...}}]}`;

export type AspectDiscoveryInput = {
  llm: LLMProvider;
  model: string;
  maxTokens: number;
  message: string;
  aspectStore: ConvoxAspectStore;
  archetypeStore: ConvoxArchetypeStore;
  /** Reuse the turn's message embedding; embedded here when omitted. */
  queryVector?: number[];
  /** Skip the LLM when a known aspect already matches at/above this cosine. Default 0.55. */
  coveredThreshold?: number;
  /** Treat a proposed aspect as a duplicate of an existing one at/above this cosine. Default 0.8. */
  dedupeThreshold?: number;
  /** Max new aspects to create per turn. Default 2. */
  maxNew?: number;
};

export type AspectDiscoveryResult = {
  createdAspects: string[];
  touchedAspects: string[];
  usedLLM: boolean;
};

const VALENCES: ArchetypeValence[] = ['positive', 'neutral', 'negative'];

function coerceBucket(
  raw: unknown,
  category: string,
  valence: ArchetypeValence,
): ArchetypeExemplar | null {
  if (!raw || typeof raw !== 'object') return null;
  const record = raw as Record<string, unknown>;
  const str = (key: string) => (typeof record[key] === 'string' ? (record[key] as string).trim() : '');
  const keyword = str('keyword');
  const description = str('description');
  const guidance = str('guidance');
  if (!keyword && !description) return null;
  return {
    category,
    valence,
    keyword,
    description,
    usage: str('usage'),
    inference: str('inference'),
    guidance,
  };
}

function parseAspects(text: string): Array<{
  name: string;
  description: string;
  buckets: ArchetypeExemplar[];
}> {
  const start = text.indexOf('{');
  const end = text.lastIndexOf('}');
  if (start === -1 || end <= start) return [];
  let parsed: Record<string, unknown>;
  try {
    parsed = JSON.parse(text.slice(start, end + 1)) as Record<string, unknown>;
  } catch {
    return [];
  }
  const list = Array.isArray(parsed.aspects) ? parsed.aspects : [];
  const out: Array<{ name: string; description: string; buckets: ArchetypeExemplar[] }> = [];
  for (const entry of list) {
    if (!entry || typeof entry !== 'object') continue;
    const record = entry as Record<string, unknown>;
    const name = typeof record.name === 'string' ? record.name.trim().toLowerCase().replace(/\s+/g, '_') : '';
    const description = typeof record.description === 'string' ? record.description.trim() : '';
    if (!name || !description) continue;
    const buckets: ArchetypeExemplar[] = [];
    for (const valence of VALENCES) {
      const bucket = coerceBucket(record[valence], name, valence);
      if (bucket) buckets.push(bucket);
    }
    // An aspect needs at least two contrasting valences to be useful.
    if (buckets.length >= 2) out.push({ name, description, buckets });
  }
  return out;
}

export async function discoverAspects(
  input: AspectDiscoveryInput,
): Promise<AspectDiscoveryResult> {
  const result: AspectDiscoveryResult = {
    createdAspects: [],
    touchedAspects: [],
    usedLLM: false,
  };

  const message = input.message.trim();
  if (message.length < 8) return result;

  const queryVector =
    input.queryVector ?? (await embed(message, { purpose: 'pathway_retrieval' }));
  const coveredThreshold = input.coveredThreshold ?? 0.55;
  const dedupeThreshold = input.dedupeThreshold ?? 0.8;
  const maxNew = input.maxNew ?? 2;

  // Novelty guard: if a known aspect already covers this message, just reinforce
  // it — no LLM spend.
  const matched = await input.aspectStore.matchByVector(queryVector, 10);
  for (const aspect of matched) {
    await input.aspectStore.touch(aspect.id);
    result.touchedAspects.push(aspect.name);
  }
  if (matched.some((aspect) => aspect.score >= coveredThreshold)) {
    return result;
  }

  const known = await input.aspectStore.list();
  const knownNames = known.map((aspect) => aspect.name);

  let text: string;
  try {
    const completion = await input.llm.complete({
      model: input.model,
      maxTokens: Math.min(input.maxTokens, 600),
      tools: [],
      system: DISCOVERY_SYSTEM,
      messages: [
        {
          role: 'user',
          content: `Known aspects (do not duplicate): ${
            knownNames.length ? knownNames.join(', ') : 'none yet'
          }\n\nUser message:\n${message}`,
        },
      ],
      telemetry: { purpose: 'aspect_discovery' },
    });
    text = completion.text;
    result.usedLLM = true;
  } catch {
    return result;
  }

  const proposals = parseAspects(text);
  for (const proposal of proposals) {
    if (result.createdAspects.length >= maxNew) break;
    if (knownNames.includes(proposal.name)) continue;

    const signatureVector = await embed(`${proposal.name} — ${proposal.description}`, {
      purpose: 'pathway_retrieval',
    });
    const duplicate = await input.aspectStore.findSimilar(signatureVector, dedupeThreshold);
    if (duplicate) {
      await input.aspectStore.touch(duplicate.id);
      result.touchedAspects.push(duplicate.name);
      continue;
    }

    const aspect = await input.aspectStore.register({
      name: proposal.name,
      description: proposal.description,
      status: 'candidate',
      source: 'discovered',
      embedding: signatureVector,
    });
    await input.archetypeStore.insertAspectBuckets(aspect.id, proposal.buckets);
    result.createdAspects.push(aspect.name);
  }

  return result;
}
