import type { FunctionDefinition } from '@aelio/protocol';

export type PendingConfirmation = {
  functionName: string;
  args: Record<string, unknown>;
  description: string;
  safetyLevel: 'write' | 'destructive';
  createdAt: number;
};

const CONFIRM_PATTERN = /^(yes|y|yeah|yep|confirm|proceed|ok|okay|sure|go ahead)$/i;
const DENY_PATTERN = /^(no|n|nope|cancel|stop|deny|don't|do not)$/i;

export function isConfirmationMessage(message: string): boolean {
  const trimmed = message.trim();
  return CONFIRM_PATTERN.test(trimmed);
}

export function isDenialMessage(message: string): boolean {
  const trimmed = message.trim();
  return DENY_PATTERN.test(trimmed);
}

/** "createMethod" / "job_apply_id" → "create method" / "job apply id". */
function humanizeKey(key: string): string {
  return key
    .replace(/[_-]+/g, ' ')
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .toLowerCase()
    .trim();
}

/**
 * Tenant tool descriptions often carry parameter documentation after the first
 * sentence ("Create a new resume. OPTIONAL: jobApplyingFor (…)"). Only the
 * first sentence belongs in a customer-facing confirmation.
 */
function actionPhrase(fn: FunctionDefinition): string {
  const source = (fn.description ?? '').trim() || humanizeKey(fn.name);
  const sentence = (source.match(/^[^.!?\n]+/) ?? [source])[0].trim();
  return sentence.charAt(0).toLowerCase() + sentence.slice(1);
}

export function buildConfirmationPrompt(
  fn: FunctionDefinition,
  args: Record<string, unknown>,
): string {
  const argSummary = Object.entries(args)
    .filter(([, value]) => value !== undefined && value !== null && String(value).trim() !== '')
    .map(([key, value]) => `${humanizeKey(key)}: ${String(value).slice(0, 80)}`)
    .join(', ');
  const details = argSummary ? ` (${argSummary})` : '';
  return `Just to confirm — I'll ${actionPhrase(fn)}${details}. Reply **yes** to confirm or **no** to cancel.`;
}

export function buildCancellationReply(): string {
  return "Okay — I won't make that change.";
}

export function buildWriteSuccessReply(fn: FunctionDefinition, _data: unknown): string {
  if (fn.name === 'cancelOrder') {
    return 'Your order has been cancelled successfully.';
  }
  // Keep success replies human — first sentence of the description only, never
  // the raw tool name or the OPTIONAL:/param schema trailer.
  const source = (fn.description ?? '').trim() || humanizeKey(fn.name);
  const sentence = (source.match(/^[^.!?\n]+/) ?? [source])[0].trim();
  return `Done — ${sentence}.`;
}