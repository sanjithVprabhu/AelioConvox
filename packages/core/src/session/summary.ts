import type { AelioDatabase } from '@aelio/db';
import { messages, sessions } from '@aelio/db';
import type { LLMProvider } from '@aelio/llm';
import { asc, count, desc, eq } from 'drizzle-orm';

type SessionSummaryMeta = {
  summarizedAtCount?: number;
  [key: string]: unknown;
};

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

  const sessionRows = await input.db
    .select({ summary: sessions.summary, metadata: sessions.metadata })
    .from(sessions)
    .where(eq(sessions.id, input.sessionId))
    .limit(1);
  const existing = sessionRows[0]?.summary ?? null;
  const metadata = (sessionRows[0]?.metadata ?? {}) as SessionSummaryMeta;
  const summarizedAtCount = metadata.summarizedAtCount ?? 0;

  // Refresh, don't freeze: a summary made at turn 50 is stale by turn 120.
  // Re-summarize each time another `summarizeAfter` messages accumulate,
  // folding the previous summary in so nothing already condensed is lost.
  if (existing && messageCount < summarizedAtCount + input.summarizeAfter) {
    return existing;
  }

  const rows = await input.db
    .select({ role: messages.role, content: messages.content })
    .from(messages)
    .where(eq(messages.sessionId, input.sessionId))
    .orderBy(desc(messages.createdAt))
    .limit(Math.min(input.summarizeAfter, 50));
  rows.reverse();

  const transcript = rows
    .map((row) => `${row.role}: ${row.content ?? ''}`.trim())
    .join('\n');
  const promptBody = existing
    ? `Previous summary:\n${existing}\n\nNew messages since then:\n${transcript}`
    : transcript;

  let summary = '';
  try {
    const result = await input.llm.complete({
      model: input.model,
      maxTokens: Math.min(input.maxTokens, 300),
      tools: [],
      system:
        'Summarize this customer session in 4-6 short bullets focusing on goals, facts, preferences, unresolved issues, and any promised follow-up. When a previous summary is provided, merge it with the new messages into one updated summary.',
      messages: [{ role: 'user', content: promptBody }],
      telemetry: { purpose: 'session_summary' },
    });
    summary = result.text.trim();
  } catch {
    summary =
      existing ??
      rows
        .slice(-10)
        .map((row) => `${row.role}: ${row.content ?? ''}`.trim())
        .join('\n');
  }

  if (!summary) {
    return null;
  }

  await input.db
    .update(sessions)
    .set({ summary, metadata: { ...metadata, summarizedAtCount: messageCount } })
    .where(eq(sessions.id, input.sessionId));
  return summary;
}
