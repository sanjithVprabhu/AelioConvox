import type { AelioDatabase } from '@aelio/db';
import type { Channel } from '@aelio/protocol';
import type { LLMProvider } from '@aelio/llm';
import { extractMemories, recallMemories, summarizeMemories } from '../analyst/index.js';
import { logFunctionCall } from '../audit/function-calls.js';
import { resolveCustomerExternalId, type IdentityConfig } from '../identity/resolve.js';
import { assertWithinRateLimit, type RateLimitConfig } from './rate-limit.js';
import {
  appendMessage,
  ensureCustomer,
  findOrCreateSession,
  loadHistory,
} from '../session/lifecycle.js';
import { maybeSummarizeSession } from '../session/summary.js';
import {
  lookupCachedResponse,
  storeCachedResponse,
  type ResponseCacheConfig,
} from './response-cache.js';
import {
  clearPendingConfirmation,
  getPendingConfirmation,
  setPendingConfirmation,
} from '../session/confirmations.js';
import type { SdkBridge } from '../sdk-bridge/types.js';
import {
  buildCancellationReply,
  buildWriteSuccessReply,
  isConfirmationMessage,
  isDenialMessage,
} from '../safety/confirmations.js';
import type { SafetyConfig } from '../safety/policy.js';
import { runToolLoop } from './tool-loop.js';

export type ProcessTurnInput = {
  database: AelioDatabase;
  llm: LLMProvider;
  sdk: SdkBridge;
  model: string;
  maxTokens: number;
  historyWindow: number;
  safety: SafetyConfig;
  identity: IdentityConfig;
  rateLimit: RateLimitConfig;
  idleTimeoutMinutes: number;
  summarizeAfter: number;
  memoryRecallLimit: number;
  customerExternalId: string;
  channel: Channel;
  channelAddress: string;
  message: string;
  memoryEnabled?: boolean;
  cache?: ResponseCacheConfig;
};

const DEFAULT_SYSTEM = `You are Aelio, a helpful conversational assistant for a SaaS product.
Answer clearly and concisely. Use available tools when you need account-specific data.
Never invent order details — always use tools for factual lookups.
For write actions, do NOT ask the user to confirm yourself — once you have the required
arguments, call the tool directly. The platform automatically asks the user to confirm
before any write executes, so a second confirmation question from you is redundant.`;

export async function processTurn(input: ProcessTurnInput): Promise<string> {
  const db = input.database.db;
  const externalId = resolveCustomerExternalId(
    input.channel,
    input.customerExternalId,
    input.identity,
  );

  const customerId = await ensureCustomer(
    db,
    externalId,
    input.channel,
    input.channelAddress,
  );

  await assertWithinRateLimit(db, customerId, input.rateLimit);

  const session = await findOrCreateSession(
    db,
    customerId,
    input.channel,
    input.idleTimeoutMinutes,
  );
  await appendMessage(db, {
    sessionId: session.id,
    customerId,
    role: 'user',
    content: input.message,
    channel: input.channel,
  });

  const context = {
    customerId: externalId,
    sessionId: session.id,
    channel: input.channel,
    channelAddress: input.channelAddress,
  };

  const pending = await getPendingConfirmation(db, session.id);
  if (pending) {
    if (isConfirmationMessage(input.message)) {
      const fn = input.sdk.getFunctions().find((entry) => entry.name === pending.functionName);
      const invokeResult = await input.sdk.invoke(
        pending.functionName,
        pending.args,
        context,
      );

      await logFunctionCall(db, {
        sessionId: session.id,
        customerId,
        functionName: pending.functionName,
        args: pending.args,
        result: invokeResult.data,
        status: invokeResult.ok ? 'success' : 'error',
        safetyLevel: pending.safetyLevel,
        requiredConfirmation: true,
        confirmed: true,
        durationMs: invokeResult.durationMs,
        errorMessage: invokeResult.error,
      });

      await clearPendingConfirmation(db, session.id);

      const reply = invokeResult.ok
        ? fn
          ? buildWriteSuccessReply(fn, invokeResult.data)
          : 'The action completed successfully.'
        : `I couldn't complete that action: ${invokeResult.error ?? 'unknown error'}`;

      await appendMessage(db, {
        sessionId: session.id,
        customerId,
        role: 'assistant',
        content: reply,
        channel: input.channel,
      });

      return reply;
    }

    if (isDenialMessage(input.message)) {
      await clearPendingConfirmation(db, session.id);
      const reply = buildCancellationReply();
      await appendMessage(db, {
        sessionId: session.id,
        customerId,
        role: 'assistant',
        content: reply,
        channel: input.channel,
      });
      return reply;
    }
  }

  // Semantic response cache: serve a near-identical, recent, no-tool reply for
  // this customer without calling the LLM. Only no-tool replies are cached, so a
  // hit never returns stale account data.
  if (input.cache?.enabled) {
    const cached = await lookupCachedResponse({
      database: input.database,
      customerId,
      message: input.message,
      threshold: input.cache.similarityThreshold,
    });
    if (cached) {
      await appendMessage(db, {
        sessionId: session.id,
        customerId,
        role: 'assistant',
        content: cached,
        channel: input.channel,
      });
      return cached;
    }
  }

  const history = await loadHistory(db, session.id, input.historyWindow);
  const summary = await maybeSummarizeSession({
    db,
    sessionId: session.id,
    summarizeAfter: input.summarizeAfter,
    llm: input.llm,
    model: input.model,
    maxTokens: input.maxTokens,
  });

  let system = DEFAULT_SYSTEM;
  if (summary) {
    system += `\n\nRolling session summary:\n${summary}`;
  }
  if (input.memoryEnabled !== false) {
    const recalled = await recallMemories(
      input.database,
      customerId,
      input.message,
      input.memoryRecallLimit,
    );
    if (recalled.length > 0) {
      system += `\n\n${summarizeMemories(recalled)}`;
    }
  }

  const loopResult = await runToolLoop({
    database: input.database,
    internalCustomerId: customerId,
    llm: input.llm,
    sdk: input.sdk,
    model: input.model,
    maxTokens: input.maxTokens,
    system,
    history,
    userMessage: input.message,
    context,
    safety: input.safety,
  });

  if (loopResult.pendingConfirmation) {
    await setPendingConfirmation(db, session.id, loopResult.pendingConfirmation);
  }

  await appendMessage(db, {
    sessionId: session.id,
    customerId,
    role: 'assistant',
    content: loopResult.reply,
    channel: input.channel,
  });

  if (input.memoryEnabled !== false) {
    void extractMemories({
      database: input.database,
      customerId,
      sessionId: session.id,
      userMessage: input.message,
      assistantReply: loopResult.reply,
    });
  }

  // Cache only no-tool, non-pending, real replies — never tool-backed answers.
  if (
    input.cache?.enabled &&
    loopResult.toolCallsExecuted === 0 &&
    !loopResult.pendingConfirmation &&
    loopResult.reply.trim().length > 0
  ) {
    await storeCachedResponse({
      database: input.database,
      customerId,
      message: input.message,
      reply: loopResult.reply,
      ttlMinutes: input.cache.ttlMinutes,
    });
  }

  return loopResult.reply;
}
