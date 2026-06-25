import type { AelioDatabase } from '@aelio/db';
import { functionCalls, memory, messages, reflections } from '@aelio/db';
import type { LLMProvider } from '@aelio/llm';
import { asc, eq } from 'drizzle-orm';
import { randomUUID } from 'node:crypto';
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
 * Closed sessions that have at least `minMessages` messages and have not yet been
 * reflected on. Pure SQL so the "already reflected" filter is the daemon's cache.
 */
export function findUnreflectedSessions(
  database: AelioDatabase,
  limit: number,
  minMessages = 2,
): Array<{ sessionId: string; customerId: string }> {
  const rows = database.sqlite
    .prepare(
      `SELECT s.id AS sessionId, s.customer_id AS customerId
       FROM sessions s
       WHERE s.status = 'closed'
         AND NOT EXISTS (SELECT 1 FROM reflections r WHERE r.session_id = s.id)
         AND (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id) >= ?
       ORDER BY s.closed_at ASC
       LIMIT ?`,
    )
    .all(minMessages, limit) as Array<{ sessionId: string; customerId: string }>;
  return rows;
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
  database: AelioDatabase;
  llm: LLMProvider;
  model: string;
  maxTokens: number;
  sessionId: string;
  customerId: string;
}): Promise<Reflection | null> {
  const db = input.database.db;

  const msgRows = await db
    .select({ role: messages.role, content: messages.content })
    .from(messages)
    .where(eq(messages.sessionId, input.sessionId))
    .orderBy(asc(messages.createdAt));

  if (msgRows.length === 0) {
    return null;
  }

  const calls = await db
    .select({ name: functionCalls.functionName, status: functionCalls.status })
    .from(functionCalls)
    .where(eq(functionCalls.sessionId, input.sessionId));

  const transcript = msgRows
    .map((m) => `${m.role}: ${(m.content ?? '').trim()}`)
    .join('\n')
    .slice(0, 4000);
  const toolSummary = calls.length
    ? calls.map((c) => `${c.name}:${c.status}`).join(', ')
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
    });
    verdict = parseVerdict(result.text);
  } catch {
    return null;
  }

  const now = new Date();
  const id = randomUUID();
  await db.insert(reflections).values({
    id,
    sessionId: input.sessionId,
    customerId: input.customerId,
    outcome: verdict.outcome,
    score: verdict.score,
    summary: verdict.summary,
    issues: verdict.issues,
    createdAt: now,
  });

  // Feed a durable insight back into long-term memory so future turns benefit.
  if (verdict.insight && verdict.insight.trim().length > 0) {
    const memId = randomUUID();
    const embedding = await embed(verdict.insight);
    await db.insert(memory).values({
      id: memId,
      customerId: input.customerId,
      content: verdict.insight.trim(),
      embedding,
      sourceSessionId: input.sessionId,
      createdAt: now,
      confidence: verdict.score || 0.5,
      category: 'insight',
    });
    input.database.upsertMemoryVector({
      memoryId: memId,
      customerId: input.customerId,
      category: 'insight',
      embedding,
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
