import type { FunctionDefinition } from '@aelio/protocol';
import { toolEmbeddings } from '@aelio/db';
import type { AelioDatabase } from '@aelio/db';
import { createHash } from 'node:crypto';
import { inArray, eq } from 'drizzle-orm';
import { embed, seedEmbedCache } from '../analyst/embeddings.js';
import { buildToolDescriptor } from './tool-descriptor.js';

function descriptorHash(descriptor: string): string {
  return createHash('sha256').update(descriptor).digest('hex');
}

/**
 * Compute and persist embeddings for every registered tool. Called when an SDK
 * connects so harness vector retrieval has warm vectors before the first turn.
 */
export async function syncToolEmbeddings(
  db: AelioDatabase['db'],
  functions: FunctionDefinition[],
): Promise<void> {
  const now = new Date();
  for (const fn of functions) {
    const descriptor = buildToolDescriptor(fn);
    const hash = descriptorHash(descriptor);

    const existing = await db
      .select()
      .from(toolEmbeddings)
      .where(eq(toolEmbeddings.toolName, fn.name))
      .limit(1);

    if (existing[0]?.descriptorHash === hash) {
      seedEmbedCache(descriptor, existing[0].embedding);
      continue;
    }

    const vector = await embed(descriptor, { purpose: 'tool_registration' });
    await db
      .insert(toolEmbeddings)
      .values({
        toolName: fn.name,
        descriptor,
        descriptorHash: hash,
        embedding: vector,
        updatedAt: now,
      })
      .onConflictDoUpdate({
        target: toolEmbeddings.toolName,
        set: {
          descriptor,
          descriptorHash: hash,
          embedding: vector,
          updatedAt: now,
        },
      });
    seedEmbedCache(descriptor, vector);
  }
}

/** Load persisted tool vectors keyed by tool name for similarity ranking. */
export async function loadToolEmbeddings(
  db: AelioDatabase['db'],
  toolNames: string[],
): Promise<Map<string, number[]>> {
  if (toolNames.length === 0) {
    return new Map();
  }

  const rows = await db
    .select()
    .from(toolEmbeddings)
    .where(inArray(toolEmbeddings.toolName, toolNames));

  const map = new Map<string, number[]>();
  for (const row of rows) {
    map.set(row.toolName, row.embedding);
    seedEmbedCache(row.descriptor, row.embedding);
  }
  return map;
}
