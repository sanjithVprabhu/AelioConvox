import type { LLMProvider } from '@aelio/llm';
import { randomUUID } from 'node:crypto';
import type { ConvoxFunctionCallStore } from '../storage/audit.js';
import type { ConvoxMemoryStore } from '../storage/memories.js';
import type { ConvoxMessageStore } from '../storage/messages.js';
import type { ConvoxReflectionStore } from '../storage/reflections.js';
import type { ConvoxSessionStore } from '../storage/sessions.js';
import { embed } from './embeddings.js';

export type ReflectionOutcome = 'resolved' | 'unresolved' | 'unclear';

export type Reflection = {
  id: string;
  sessionId: string;
  customerId: string;
  outcome: ReflectionOutcome;
  score: number;
  summary: string;
  issues: string[];
  insight?: string;
  followup?: string;
};

const REFLECT_SYSTEM = `You are Aelio's analytical agent reviewing a finished customer support session.
Judge whether the customer's intent was actually resolved and what can be learned.
Respond with ONLY a JSON object, no prose, in this exact shape:
{"outcome":"resolved|unresolved|unclear","score":0.0,"summary":"1-2 sentences","issues":["..."],"insight":"one durable fact about this customer worth remembering, or empty string","followup":"a short, friendly customer-facing message to re-engage them IF their issue seems unresolved, otherwise empty string"}`;

/**
 * Closed sessions that have at least `minMessages` messages and have not yet
 * been reflected on. AelioDb-only: `sessionStore` is required; `reflectionStore`
 * and `messageStore` refine the candidate set (session/message counts) when
 * supplied.
 */
export async function findUnreflectedSessions(
  limit: number,
  minMessages = 2,
  sessionStore?: ConvoxSessionStore,
  reflectionStore?: ConvoxReflectionStore,
  messageStore?: ConvoxMessageStore,
): Promise<Array<{ sessionId: string; customerId: string }>> {
  if (!sessionStore) {
    throw new Error('AelioDb sessionStore is required');
  }
  // Overfetch, then filter by reflection state / message count in-process —
  // AelioDb has no NOT EXISTS / correlated-subquery equivalent over HTTP.
  const closed = await sessionStore.listClosed(Math.max(limit * 4, limit));
  const out: Array<{ sessionId: string; customerId: string }> = [];
  for (const session of closed) {
    if (out.length >= limit) {
      break;
    }
    const hasReflection = reflectionStore ? await reflectionStore.hasForSession(session.sessionId) : false;
    if (hasReflection) {
      continue;
    }
    const messageCount = messageStore ? await messageStore.countSessionMessages(session.sessionId) : 0;
    if (messageCount < minMessages) {
      continue;
    }
    out.push({ sessionId: session.sessionId, customerId: session.customerId });
  }
  return out;
}

function parseVerdict(text: string): {
  outcome: ReflectionOutcome;
  score: number;
  summary: string;
  issues: string[];
  insight: string;
  followup: string;
} {
  const fallback = {
    outcome: 'unclear' as ReflectionOutcome,
    score: 0,
    summary: text.slice(0, 400).trim(),
    issues: [] as string[],
    insight: '',
    followup: '',
  };

  const start = text.indexOf('{');
  const end = text.lastIndexOf('}');
  if (start === -1 || end <= start) {
    return fallback;
  }

  try {
    const parsed = JSON.parse(text.slice(start, end + 1)) as Record<string, unknown>;
    const outcome =
      parsed.outcome === 'resolved' || parsed.outcome === 'unresolved' ? parsed.outcome : 'unclear';
    const scoreNum = typeof parsed.score === 'number' ? parsed.score : Number(parsed.score);
    return {
      outcome,
      score: Number.isFinite(scoreNum) ? Math.max(0, Math.min(1, scoreNum)) : 0,
      summary: typeof parsed.summary === 'string' ? parsed.summary : fallback.summary,
      issues: Array.isArray(parsed.issues) ? parsed.issues.map((i) => String(i)) : [],
      insight: typeof parsed.insight === 'string' ? parsed.insight : '',
      followup: typeof parsed.followup === 'string' ? parsed.followup : '',
    };
  } catch {
    return fallback;
  }
}

/** Run one bounded LLM reflection on a single session and persist the verdict. */
export async function reflectOnSession(input: {
  llm: LLMProvider;
  model: string;
  maxTokens: number;
  sessionId: string;
  customerId: string;
  /** Long-term insights go to AelioDb/VSS. */
  memoryStore?: ConvoxMemoryStore;
  messageStore: ConvoxMessageStore;
  functionCallStore?: ConvoxFunctionCallStore;
  reflectionStore: ConvoxReflectionStore;
}): Promise<Reflection | null> {
  if (!input.messageStore) {
    throw new Error('AelioDb messageStore is required');
  }
  if (!input.reflectionStore) {
    throw new Error('AelioDb reflectionStore is required');
  }

  const rows = await input.messageStore.loadTranscript(input.sessionId);
  if (rows.length === 0) {
    return null;
  }
  const transcript = rows
    .map((m) => `${m.role}: ${(m.content ?? '').trim()}`)
    .join('\n')
    .slice(0, 4000);
  const calls = input.functionCallStore ? await input.functionCallStore.listBySession(input.sessionId) : [];
  const toolSummary = calls.length
    ? calls.map((c) => `${c.functionName}:${c.status}`).join(', ')
    : 'none';

  let verdict;
  try {
    const result = await input.llm.complete({
      model: input.model,
      maxTokens: Math.min(input.maxTokens, 400),
      tools: [],
      system: REFLECT_SYSTEM,
      messages: [
        {
          role: 'user',
          content: `Tool calls in this session: ${toolSummary}\n\nTranscript:\n${transcript}`,
        },
      ],
      telemetry: { purpose: 'session_reflection' },
    });
    verdict = parseVerdict(result.text);
  } catch {
    return null;
  }

  const id = randomUUID();
  await input.reflectionStore.store({
    sessionId: input.sessionId,
    customerId: input.customerId,
    outcome: verdict.outcome,
    score: verdict.score,
    summary: verdict.summary,
    issues: verdict.issues,
    insight: verdict.insight || undefined,
    followup: verdict.followup || undefined,
  });

  // Feed a durable insight into AelioDb/VSS so future turns can recall it.
  if (input.memoryStore && verdict.insight && verdict.insight.trim().length > 0) {
    const insight = verdict.insight.trim();
    const embedding = await embed(insight, { purpose: 'reflection_insight' });
    await input.memoryStore.store({
      customerId: input.customerId,
      content: insight,
      category: 'insight',
      sourceSessionId: input.sessionId,
      confidence: verdict.score || 0.5,
      embedding,
      dedupe: true,
    });
  }

  return {
    id,
    sessionId: input.sessionId,
    customerId: input.customerId,
    outcome: verdict.outcome,
    score: verdict.score,
    summary: verdict.summary,
    issues: verdict.issues,
    insight: verdict.insight || undefined,
    followup: verdict.followup || undefined,
  };
}
