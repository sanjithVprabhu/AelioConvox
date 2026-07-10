import type { FunctionDefinition } from '@aelio/protocol';

export type PendingConfirmation = {
  functionName: string;
  args: Record<string, unknown>;
  description: string;
  safetyLevel: 'write' | 'destructive';
  createdAt: number;
  /** When set, confirmation resumes a multi-step harness plan. */
  harnessPlanId?: string;
  harnessStepId?: string;
};

const CONFIRM_PATTERN = /^(yes|y|yeah|yep|confirm|proceed|sure|go ahead|ok|okay)$/i;
const DENY_PATTERN = /^(no|n|nope|cancel|stop|deny|don't|do not)$/i;

/** Pending write confirmations expire after this window. */
export const PENDING_CONFIRMATION_TTL_MS = 10 * 60_000;

export function isPendingConfirmationExpired(pending: PendingConfirmation): boolean {
  return Date.now() - pending.createdAt > PENDING_CONFIRMATION_TTL_MS;
}

export function isConfirmationMessage(message: string): boolean {
  const trimmed = message.trim();
  return CONFIRM_PATTERN.test(trimmed);
}

export function isDenialMessage(message: string): boolean {
  const trimmed = message.trim();
  return DENY_PATTERN.test(trimmed);
}

export function buildConfirmationPrompt(
  fn: FunctionDefinition,
  args: Record<string, unknown>,
): string {
  const argSummary = Object.entries(args)
    .map(([key, value]) => `${key}: ${String(value)}`)
    .join(', ');
  const details = argSummary ? ` (${argSummary})` : '';
  return `I'm about to run **${fn.name}** — ${fn.description}${details}. Reply **yes** to confirm or **no** to cancel.`;
}

export function buildCancellationReply(): string {
  return "Okay — I won't make that change.";
}

export function buildWriteSuccessReply(fn: FunctionDefinition, data: unknown): string {
  if (fn.name === 'cancelOrder') {
    return 'Your order has been cancelled successfully.';
  }
  return `Done — ${fn.description} completed successfully.`;
}