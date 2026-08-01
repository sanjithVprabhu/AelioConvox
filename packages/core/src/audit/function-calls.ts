import type { SafetyLevel } from '@aelio/protocol';
import type { ConvoxFunctionCallStore } from '../storage/audit.js';

export async function logFunctionCall(
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
  functionCallStore: ConvoxFunctionCallStore,
): Promise<void> {
  if (!functionCallStore) {
    throw new Error('AelioDb functionCallStore is required');
  }
  await functionCallStore.log(input);
}
