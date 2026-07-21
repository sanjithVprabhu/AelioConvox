import { embed } from './embeddings.js';
import type { ConvoxMemoryStore } from '../storage/memories.js';

export type ExtractInput = {
  memoryStore?: ConvoxMemoryStore;
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
  if (!input.memoryStore) {
    return [];
  }

  const facts = detectFacts(input.userMessage);
  const stored: string[] = [];

  for (const fact of facts) {
    const embedding = await embed(fact.content, {
      purpose: 'memory_extract',
      turnId: input.turnId,
      sessionId: input.sessionId,
      customerId: input.customerId,
    });

    const result = await input.memoryStore.store({
      customerId: input.customerId,
      content: fact.content,
      category: fact.category,
      sourceSessionId: input.sessionId,
      confidence: 1,
      embedding,
      dedupe: true,
    });

    if (result) {
      stored.push(fact.content);
    }
  }

  return stored;
}
