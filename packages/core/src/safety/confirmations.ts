import type { FunctionDefinition } from '@aelio/protocol';

export type PendingConfirmation = {
  functionName: string;
  args: Record<string, unknown>;
  description: string;
  safetyLevel: 'write' | 'destructive';
  createdAt: number;
};

const CONFIRM_PATTERN = /^(yes|y|yeah|yep|confirm|proceed|ok|okay|sure|go ahead)\b/i;
const DENY_PATTERN = /^(no|n|nope|cancel|stop|deny|don't|do not)\b/i;

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