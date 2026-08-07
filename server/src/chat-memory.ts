import type { RuntimeDeps } from './runtime-deps.js';

export type ChatSessionContext = {
  customerId: string;
  sessionId: string;
};

export type ChatHistoryMessage = {
  role: 'user' | 'assistant' | 'system';
  content: string;
};

/** Resolve the internal customer id and an active chat session (create if needed). */
export async function ensureChatSession(
  deps: RuntimeDeps,
  input: {
    customerExternalId: string;
    channel: string;
    channelAddress: string;
    resumeSessionId?: string;
  },
): Promise<ChatSessionContext> {
  const customerId = await deps.customerStore.ensureCustomer(
    input.customerExternalId,
    input.channel,
    input.channelAddress,
  );

  if (input.resumeSessionId) {
    const existing = await deps.sessionStore.get(input.resumeSessionId);
    if (
      existing &&
      existing.customerId === customerId &&
      existing.status === 'active'
    ) {
      return { customerId, sessionId: existing.id };
    }
  }

  const session = await deps.sessionStore.findOrCreate(
    customerId,
    input.channel,
    deps.config.session.idle_timeout_minutes,
  );
  return { customerId, sessionId: session.id };
}

/** Load recent transcript rows for a session (L0 tier, user/assistant/system). */
export async function loadChatHistory(
  deps: RuntimeDeps,
  sessionId: string,
): Promise<ChatHistoryMessage[]> {
  return deps.messageStore.loadHistory(sessionId, deps.config.session.history_window);
}

export async function persistChatMessage(
  deps: RuntimeDeps,
  input: {
    sessionId: string;
    customerId: string;
    channel: string;
    role: 'user' | 'assistant' | 'system';
    content: string;
  },
): Promise<void> {
  await deps.messageStore.appendMessage(input);
  await deps.sessionStore.touchActivity(input.sessionId);
}
