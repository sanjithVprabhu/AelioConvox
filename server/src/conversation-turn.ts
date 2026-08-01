import { createHash } from 'node:crypto';
import { randomUUID } from 'node:crypto';
import type { RuntimeDeps } from './runtime-deps.js';

export type ConversationTurnInput = {
  customerExternalId: string;
  channel: string;
  channelAddress: string;
  message: string;
  /** Stable identifier supplied by the ingress provider (for example a WhatsApp message id). */
  sourceTurnId?: string;
};

export type ConversationTurnResult = {
  reply: string;
  turnId: string;
  awaitingConfirmation: boolean;
};

/**
 * One ingress for every conversational channel. When the Rust runtime is configured it is the
 * sole decision executor.
 */
export async function executeConversationTurn(
  deps: RuntimeDeps,
  input: ConversationTurnInput,
): Promise<ConversationTurnResult> {
  const instanceId = stableAgentUserId(input.customerExternalId);
  const turnId = input.sourceTurnId
    ? stableTurnId(input.channel, input.customerExternalId, input.sourceTurnId)
    : randomUUID();
  const result = await deps.aelioRuntime.submitAgentTurn({
    turn_id: turnId,
    user_id: instanceId,
    utterance: input.message,
    channel: input.channel,
  });
  const reply = result.reply.text.trim();
  if (!reply) {
    throw new Error('Aelio Rust agent returned an empty customer-facing reply');
  }
  return {
    reply,
    turnId,
    awaitingConfirmation: result.suspended,
  };
}

/** Stable, channel-independent identity used by turns and SDK lifecycle commands. */
export function stableAgentUserId(customerExternalId: string): string {
  return createHash('sha256')
    .update(`aelio-user\u001f${customerExternalId.trim()}`)
    .digest('hex');
}

function stableTurnId(channel: string, customerId: string, sourceTurnId: string): string {
  return `ingress:${createHash('sha256')
    .update(`${channel}\u001f${customerId}\u001f${sourceTurnId}`)
    .digest('hex')}`;
}
