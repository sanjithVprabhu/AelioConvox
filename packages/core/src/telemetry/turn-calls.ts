import type { AelioDatabase } from '@aelio/db';
import { turnApiCalls } from '@aelio/db';
import type { LLMCompleteOptions, LLMCompleteResult, LLMProvider } from '@aelio/llm';
import { AsyncLocalStorage } from 'node:async_hooks';
import { randomUUID } from 'node:crypto';
import { and, asc, desc, eq } from 'drizzle-orm';

const TRUNCATE = 500;
const INPUT_PREVIEW = 300;

export type TurnApiCallPurpose =
  | 'chat_completion'
  | 'tool_synthesis'
  | 'session_summary'
  | 'session_reflection'
  | 'memory_recall'
  | 'memory_extract'
  | 'response_cache_lookup'
  | 'response_cache_store'
  | 'message_storage'
  | 'conversation_archive'
  | 'reflection_insight'
  // Harness passes
  | 'plan'
  | 'replan'
  | 'bind'
  | 'synthesis'
  | 'recoil_extract';

export type TurnContext = {
  turnId: string;
  sessionId: string;
  customerId: string;
  sequence: number;
  database: AelioDatabase;
};

export type TurnApiCallRecord = {
  id: string;
  turnId: string;
  sessionId: string;
  customerId: string;
  sequence: number;
  callType: 'llm' | 'embed';
  purpose: TurnApiCallPurpose;
  model?: string;
  iteration?: number;
  promptSummary: string;
  inputPreview?: string;
  messageCount?: number;
  toolCount?: number;
  toolNames?: string[];
  stopReason?: string;
  durationMs?: number;
  createdAt: number;
};

const turnContextStorage = new AsyncLocalStorage<TurnContext>();

function truncate(value: string, max = TRUNCATE): string {
  const trimmed = value.replace(/\s+/g, ' ').trim();
  if (trimmed.length <= max) {
    return trimmed;
  }
  return `${trimmed.slice(0, max)}…`;
}

export function getTurnContext(): TurnContext | undefined {
  return turnContextStorage.getStore();
}

export async function runWithTurnContext<T>(
  context: Omit<TurnContext, 'sequence'>,
  fn: () => Promise<T>,
): Promise<T> {
  return turnContextStorage.run({ ...context, sequence: 0 }, fn);
}

function nextSequence(ctx: TurnContext): number {
  ctx.sequence += 1;
  return ctx.sequence;
}

export function buildLlmPromptSummary(
  opts: LLMCompleteOptions,
  input: { purpose: TurnApiCallPurpose; iteration?: number },
): string {
  const parts = [`purpose=${input.purpose}`];
  if (input.iteration !== undefined) {
    parts.push(`iteration=${input.iteration}`);
  }
  if (opts.system?.trim()) {
    parts.push(`system=${truncate(opts.system, 280)}`);
  }
  parts.push(`messages=${opts.messages.length}`);
  if (opts.tools.length > 0) {
    parts.push(`tools=[${opts.tools.map((tool) => tool.name).join(', ')}]`);
  }
  const lastUser = [...opts.messages].reverse().find((message) => message.role === 'user');
  if (lastUser?.content.trim()) {
    parts.push(`last_user=${truncate(lastUser.content, 160)}`);
  }
  return parts.join(' | ');
}

export async function recordTurnApiCall(input: {
  callType: 'llm' | 'embed';
  purpose: TurnApiCallPurpose;
  promptSummary: string;
  inputPreview?: string;
  model?: string;
  iteration?: number;
  messageCount?: number;
  toolCount?: number;
  toolNames?: string[];
  stopReason?: string;
  durationMs?: number;
  tokensIn?: number;
  tokensOut?: number;
  turnId?: string;
  sessionId?: string;
  customerId?: string;
  database?: AelioDatabase;
}): Promise<string | null> {
  const ctx = getTurnContext();
  const turnId = input.turnId ?? ctx?.turnId;
  const sessionId = input.sessionId ?? ctx?.sessionId;
  const customerId = input.customerId ?? ctx?.customerId;
  const database = input.database ?? ctx?.database;

  if (!turnId || !sessionId || !customerId || !database) {
    return null;
  }

  const sequence = ctx ? nextSequence(ctx) : 1;
  const id = randomUUID();
  const now = new Date();

  await database.db.insert(turnApiCalls).values({
    id,
    turnId,
    sessionId,
    customerId,
    sequence,
    callType: input.callType,
    purpose: input.purpose,
    model: input.model,
    iteration: input.iteration,
    promptSummary: input.promptSummary,
    inputPreview: input.inputPreview ? truncate(input.inputPreview, INPUT_PREVIEW) : undefined,
    messageCount: input.messageCount,
    toolCount: input.toolCount,
    toolNames: input.toolNames,
    stopReason: input.stopReason,
    durationMs: input.durationMs,
    tokensIn: input.tokensIn,
    tokensOut: input.tokensOut,
    createdAt: now,
  });

  return id;
}

export function createInstrumentedLlm(provider: LLMProvider): LLMProvider {
  return {
    async complete(opts) {
      const telemetry = opts.telemetry;
      const purpose = telemetry?.purpose ?? 'chat_completion';
      const started = Date.now();
      let result: LLMCompleteResult;
      try {
        result = await provider.complete(opts);
      } catch (error) {
        await recordTurnApiCall({
          callType: 'llm',
          purpose,
          model: opts.model,
          iteration: telemetry?.iteration,
          promptSummary: buildLlmPromptSummary(opts, {
            purpose,
            iteration: telemetry?.iteration,
          }),
          messageCount: opts.messages.length,
          toolCount: opts.tools.length,
          toolNames: opts.tools.map((tool) => tool.name),
          stopReason: 'error',
          durationMs: Date.now() - started,
        });
        throw error;
      }

      await recordTurnApiCall({
        callType: 'llm',
        purpose,
        model: opts.model,
        iteration: telemetry?.iteration,
        promptSummary: buildLlmPromptSummary(opts, {
          purpose,
          iteration: telemetry?.iteration,
        }),
        messageCount: opts.messages.length,
        toolCount: opts.tools.length,
        toolNames: opts.tools.map((tool) => tool.name),
        stopReason: result.stopReason,
        durationMs: Date.now() - started,
        tokensIn: result.usage?.inputTokens,
        tokensOut: result.usage?.outputTokens,
      });

      return result;
    },
  };
}

export async function listTurnApiCalls(
  database: AelioDatabase,
  input: {
    turnId?: string;
    sessionId?: string;
    limit?: number;
  } = {},
): Promise<TurnApiCallRecord[]> {
  const limit = Math.min(Math.max(input.limit ?? 200, 1), 1000);
  const conditions = [];
  if (input.turnId) {
    conditions.push(eq(turnApiCalls.turnId, input.turnId));
  }
  if (input.sessionId) {
    conditions.push(eq(turnApiCalls.sessionId, input.sessionId));
  }

  const query = database.db
    .select()
    .from(turnApiCalls)
    .orderBy(
      input.turnId ? asc(turnApiCalls.sequence) : desc(turnApiCalls.createdAt),
    )
    .limit(limit);

  const rows =
    conditions.length > 0 ? await query.where(and(...conditions)) : await query;

  return rows.map((row) => ({
    id: row.id,
    turnId: row.turnId,
    sessionId: row.sessionId,
    customerId: row.customerId,
    sequence: row.sequence,
    callType: row.callType as 'llm' | 'embed',
    purpose: row.purpose as TurnApiCallPurpose,
    model: row.model ?? undefined,
    iteration: row.iteration ?? undefined,
    promptSummary: row.promptSummary,
    inputPreview: row.inputPreview ?? undefined,
    messageCount: row.messageCount ?? undefined,
    toolCount: row.toolCount ?? undefined,
    toolNames: row.toolNames ?? undefined,
    stopReason: row.stopReason ?? undefined,
    durationMs: row.durationMs ?? undefined,
    createdAt: row.createdAt.getTime(),
  }));
}

export async function summarizeTurnApiCalls(
  database: AelioDatabase,
  turnId: string,
): Promise<{
  turnId: string;
  totalCalls: number;
  llmCalls: number;
  embedCalls: number;
  calls: TurnApiCallRecord[];
}> {
  const calls = await listTurnApiCalls(database, { turnId, limit: 100 });
  return {
    turnId,
    totalCalls: calls.length,
    llmCalls: calls.filter((call) => call.callType === 'llm').length,
    embedCalls: calls.filter((call) => call.callType === 'embed').length,
    calls,
  };
}