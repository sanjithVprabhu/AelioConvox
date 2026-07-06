import type { AelioDatabase } from '@aelio/db';
import { memory } from '@aelio/db';
import { and, eq } from 'drizzle-orm';
import { randomUUID } from 'node:crypto';
import { embed } from './embeddings.js';

export type ExtractInput = {
  database: AelioDatabase;
  customerId: string;
  sessionId: string;
  userMessage: string;
  assistantReply: string;
  turnId?: string;
};

/**
 * Per-turn extraction captures only DOMAIN-NEUTRAL, explicit self-declarations
 * (identity + stated preferences) — signals that are true regardless of which
 * company's tools are attached. Deep domain understanding is the reflection
 * daemon's job (one LLM pass per closed session, in analyst/reflect.ts): richer
 * and far cheaper than per-turn keyword guessing, and never tenant-specific.
 */
function detectFacts(userMessage: string): Array<{ content: string; category: string }> {
  const facts: Array<{ content: string; category: string }> = [];
  const message = userMessage.trim();

  const email = message.match(/[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/i);
  if (email) {
    facts.push({ content: `User email is ${email[0]}`, category: 'profile' });
  }

  const name = message.match(/\bmy name is\s+([A-Za-z][A-Za-z '-]{1,40})/i);
  if (name?.[1]) {
    facts.push({ content: `User name is ${name[1].trim()}`, category: 'profile' });
  }

  const phone = message.match(/\bmy (?:phone|number) is\s*([+()\d][\d\s()-]{6,18}\d)/i);
  if (phone?.[1]) {
    facts.push({ content: `User phone is ${phone[1].trim()}`, category: 'profile' });
  }

  // Explicit first-person preference statements ("I prefer…", "I always…",
  // "please never…") — kept verbatim so no tenant vocabulary is assumed.
  for (const line of message.split(/[.!?\n]/)) {
    const part = line.trim();
    if (part.length === 0 || part.length > 140) {
      continue;
    }
    if (/\b(?:i|we)\s+(?:prefer|always|usually|never|only|like to|want to|don'?t want)\b/i.test(part)) {
      facts.push({ content: `Stated preference: ${part}`, category: 'preference' });
    } else if (/\bplease\s+(?:always|never|don'?t)\b/i.test(part)) {
      facts.push({ content: `Stated preference: ${part}`, category: 'preference' });
    }
  }

  return [...new Map(facts.map((fact) => [`${fact.category}:${fact.content}`, fact])).values()];
}

export async function extractMemories(input: ExtractInput): Promise<string[]> {
  const facts = detectFacts(input.userMessage);
  const stored: string[] = [];

  for (const fact of facts) {
    // Dedup: the same durable fact (e.g. "frequently asks about order status")
    // re-triggers on many turns — one row per customer per fact is enough.
    const existing = await input.database.db
      .select({ id: memory.id })
      .from(memory)
      .where(and(eq(memory.customerId, input.customerId), eq(memory.content, fact.content)))
      .limit(1);
    if (existing[0]) {
      continue;
    }

    const id = randomUUID();
    const embedding = await embed(fact.content, {
      purpose: 'memory_extract',
      turnId: input.turnId,
      sessionId: input.sessionId,
      customerId: input.customerId,
      database: input.database,
    });
    await input.database.db.insert(memory).values({
      id,
      customerId: input.customerId,
      content: fact.content,
      embedding,
      sourceSessionId: input.sessionId,
      createdAt: new Date(),
      confidence: 1,
      category: fact.category,
    });
    input.database.upsertMemoryVector({
      memoryId: id,
      customerId: input.customerId,
      category: fact.category,
      embedding,
    });
    stored.push(fact.content);
  }

  return stored;
}
