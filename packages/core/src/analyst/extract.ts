import type { AelioDatabase } from '@aelio/db';
import { memory } from '@aelio/db';
import { randomUUID } from 'node:crypto';
import { embed } from './embeddings.js';

export type ExtractInput = {
  database: AelioDatabase;
  customerId: string;
  sessionId: string;
  userMessage: string;
  assistantReply: string;
};

function detectFacts(userMessage: string, assistantReply: string): Array<{ content: string; category: string }> {
  const facts: Array<{ content: string; category: string }> = [];
  const lower = userMessage.toLowerCase();
  const lines = userMessage
    .split(/[.!?\n]/)
    .map((part) => part.trim())
    .filter(Boolean);

  if (lower.includes('metric')) {
    facts.push({ content: 'User prefers metric units', category: 'preference' });
  }
  if (lower.includes('imperial') || lower.includes('fahrenheit')) {
    facts.push({ content: 'User prefers imperial units', category: 'preference' });
  }
  if (lower.includes('order') || lower.includes('ship') || lower.includes('status')) {
    facts.push({ content: 'User frequently asks about order status and shipping', category: 'behavior' });
  }
  if (assistantReply.toLowerCase().includes('tracking')) {
    facts.push({ content: 'User recently checked a shipped order', category: 'order' });
  }
  if (lower.includes('english') || lower.includes('hindi')) {
    facts.push({ content: `User mentioned language preference in: "${userMessage.trim()}"`, category: 'language' });
  }
  if (lower.includes('email me at ')) {
    const match = userMessage.match(/[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/i);
    if (match) {
      facts.push({ content: `User email is ${match[0]}`, category: 'profile' });
    }
  }
  if (lower.includes('my name is ') || lower.startsWith("i'm ") || lower.startsWith('i am ')) {
    const match = userMessage.match(/(?:my name is|i'm|i am)\s+([A-Za-z][A-Za-z '-]{1,40})/i);
    if (match?.[1]) {
      facts.push({ content: `User name is ${match[1].trim()}`, category: 'profile' });
    }
  }
  for (const line of lines) {
    if (/prefer|usually|always|never|only/i.test(line) && line.length <= 140) {
      facts.push({ content: `Preference signal: ${line}`, category: 'preference' });
    }
  }
  if (assistantReply.toLowerCase().includes('follow up') || assistantReply.toLowerCase().includes('i will check')) {
    facts.push({ content: 'Assistant promised follow-up or manual verification', category: 'workflow' });
  }

  return [...new Map(facts.map((fact) => [`${fact.category}:${fact.content}`, fact])).values()];
}

export async function extractMemories(input: ExtractInput): Promise<string[]> {
  const facts = detectFacts(input.userMessage, input.assistantReply);
  const stored: string[] = [];

  for (const fact of facts) {
    const id = randomUUID();
    const embedding = await embed(fact.content);
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
