import type { AelioDatabase } from '@aelio/db';
import { functionCalls } from '@aelio/db';
import type { SafetyLevel } from '@aelio/protocol';
import { randomUUID } from 'node:crypto';

export async function logFunctionCall(
  db: AelioDatabase['db'],
  input: {
    sessionId: string;
    customerId: string;
    functionName: string;
    args: Record<string, unknown>;
    result?: unknown;
    status: 'success' | 'error' | 'timeout' | 'blocked' | 'pending';
    safetyLevel: SafetyLevel;
    requiredConfirmation?: boolean;
    confirmed?: boolean;
    durationMs?: number;
    errorMessage?: string;
  },
): Promise<void> {
  await db.insert(functionCalls).values({
    id: randomUUID(),
    sessionId: input.sessionId,
    customerId: input.customerId,
    functionName: input.functionName,
    args: input.args,
    result: input.result as Record<string, unknown> | undefined,
    status: input.status,
    safetyLevel: input.safetyLevel,
    requiredConfirmation: input.requiredConfirmation ?? false,
    confirmed: input.confirmed,
    durationMs: input.durationMs,
    errorMessage: input.errorMessage,
    createdAt: new Date(),
  });
}