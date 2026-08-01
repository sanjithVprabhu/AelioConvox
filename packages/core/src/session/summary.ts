import type { LLMProvider } from '@aelio/llm';
import type { ConvoxMessageStore } from '../storage/messages.js';
import type { ConvoxSessionStore } from '../storage/sessions.js';

type SessionSummaryMeta = {
  summarizedAtCount?: number;
  [key: string]: unknown;
};

export async function loadSessionSummary(
  sessionId: string,
  sessionStore: ConvoxSessionStore,
): Promise<string | null> {
  if (!sessionStore) {
    throw new Error('AelioDb sessionStore is required');
  }
  return sessionStore.getSummary(sessionId);
}

export async function maybeSummarizeSession(input: {
  sessionStore: ConvoxSessionStore;
  messageStore: ConvoxMessageStore;
  sessionId: string;
  summarizeAfter: number;
  llm: LLMProvider;
  model: string;
  maxTokens: number;
}): Promise<string | null> {
  if (!input.sessionStore) {
    throw new Error('AelioDb sessionStore is required');
  }
  if (!input.messageStore) {
    throw new Error('AelioDb messageStore is required');
  }

  const messageCount = await input.messageStore.countSessionMessages(input.sessionId);

  if (messageCount < input.summarizeAfter) {
    return null;
  }

  const session = await input.sessionStore.get(input.sessionId);
  const existing = session?.summary ?? null;
  const metadata = (session?.metadata ?? {}) as SessionSummaryMeta;

  const summarizedAtCount = metadata.summarizedAtCount ?? 0;
  if (existing && messageCount < summarizedAtCount + input.summarizeAfter) {
    return existing;
  }

  const rows = await input.messageStore.loadTranscript(
    input.sessionId,
    Math.min(input.summarizeAfter, 50),
  );

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

  const nextMeta = { ...metadata, summarizedAtCount: messageCount };
  await input.sessionStore.updateSummary(input.sessionId, summary, nextMeta);
  return summary;
}
