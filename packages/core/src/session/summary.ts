import type { AelioDatabase } from '@aelio/db';
import { messages, sessions } from '@aelio/db';
import type { LLMProvider } from '@aelio/llm';
import { and, asc, count, eq } from 'drizzle-orm';

export async function loadSessionSummary(
  db: AelioDatabase['db'],
  sessionId: string,
): Promise<string | null> {
  const rows = await db.select({ summary: sessions.summary }).from(sessions).where(eq(sessions.id, sessionId)).limit(1);
  return rows[0]?.summary ?? null;
}

export async function maybeSummarizeSession(input: {
  db: AelioDatabase['db'];
  sessionId: string;
  summarizeAfter: number;
  llm: LLMProvider;
  model: string;
  maxTokens: number;
}): Promise<string | null> {
  const countRows = await input.db
    .select({ value: count() })
    .from(messages)
    .where(eq(messages.sessionId, input.sessionId));
  const messageCount = countRows[0]?.value ?? 0;

  if (messageCount < input.summarizeAfter) {
    return null;
  }

  const existing = await loadSessionSummary(input.db, input.sessionId);
  if (existing) {
    return existing;
  }

  const rows = await input.db
    .select({ role: messages.role, content: messages.content })
    .from(messages)
    .where(
      and(
        eq(messages.sessionId, input.sessionId),
      ),
    )
    .orderBy(asc(messages.createdAt))
    .limit(Math.min(input.summarizeAfter, 50));

  const transcript = rows
    .map((row) => `${row.role}: ${row.content ?? ''}`.trim())
    .join('\n');

  let summary = '';
  try {
    const result = await input.llm.complete({
      model: input.model,
      maxTokens: Math.min(input.maxTokens, 300),
      tools: [],
      system:
        'Summarize this customer session in 4-6 short bullets focusing on goals, facts, preferences, unresolved issues, and any promised follow-up.',
      messages: [{ role: 'user', content: transcript }],
      telemetry: { purpose: 'session_summary' },
    });
    summary = result.text.trim();
  } catch {
    summary = rows
      .slice(-10)
      .map((row) => `${row.role}: ${row.content ?? ''}`.trim())
      .join('\n');
  }

  if (!summary) {
    return null;
  }

  await input.db.update(sessions).set({ summary }).where(eq(sessions.id, input.sessionId));
  return summary;
}
