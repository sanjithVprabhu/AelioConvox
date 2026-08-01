import type { LLMCompleteOptions, LLMCompleteResult, LLMProvider } from '@aelio/llm';
import type { AelioDbClient } from '@aelio/db-client';
import { AsyncLocalStorage } from 'node:async_hooks';
import { randomUUID } from 'node:crypto';
import { i64, parseJson, readI64, readUtf8, utf8 } from '../storage/helpers.js';

const TRUNCATE = 500;
const INPUT_PREVIEW = 300;

export type TurnApiCallPurpose =
  | 'chat_completion'
  | 'pathway_retrieval'
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
  | 'immediate_context_compaction'
  | 'aspect_discovery'
  // Harness passes
  | 'plan'
  | 'replan'
  | 'bind'
  | 'synthesis'
  | 'recoil_extract';

export type TurnApiCallsAelioDbConfig = {
  client: AelioDbClient;
  table: string;
};

export type TurnContext = {
  turnId: string;
  sessionId: string;
  customerId: string;
  sequence: number;
  /** When present, per-turn API call telemetry writes to AelioDb exclusively. */
  aelioDb?: TurnApiCallsAelioDbConfig;
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

/**
 * Record one per-turn API call. Writes to AelioDb exclusively — either the
 * `aelioDb` config passed explicitly or the one carried on the ambient turn
 * context (see `runWithTurnContext`). Silently no-ops (returns null) when
 * neither is available, since telemetry must never break the turn.
 */
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
  aelioDb?: TurnApiCallsAelioDbConfig;
}): Promise<string | null> {
  const ctx = getTurnContext();
  const turnId = input.turnId ?? ctx?.turnId;
  const sessionId = input.sessionId ?? ctx?.sessionId;
  const customerId = input.customerId ?? ctx?.customerId;
  const aelioDb = input.aelioDb ?? ctx?.aelioDb;

  if (!turnId || !sessionId || !customerId || !aelioDb) {
    return null;
  }

  const sequence = ctx ? nextSequence(ctx) : 1;
  const id = randomUUID();
  const now = new Date();

  await aelioDb.client.insertRow(aelioDb.table, {
    call_id: utf8(id),
    turn_id: utf8(turnId),
    session_id: utf8(sessionId),
    customer_id: utf8(customerId),
    sequence: i64(sequence),
    call_type: utf8(input.callType),
    purpose: utf8(input.purpose),
    model: utf8(input.model ?? ''),
    iteration: i64(input.iteration ?? 0),
    prompt_summary: utf8(input.promptSummary),
    input_preview: utf8(input.inputPreview ? truncate(input.inputPreview, INPUT_PREVIEW) : ''),
    message_count: i64(input.messageCount ?? 0),
    tool_count: i64(input.toolCount ?? 0),
    tool_names: utf8(JSON.stringify(input.toolNames ?? [])),
    stop_reason: utf8(input.stopReason ?? ''),
    duration_ms: i64(input.durationMs ?? 0),
    tokens_in: i64(input.tokensIn ?? 0),
    tokens_out: i64(input.tokensOut ?? 0),
    created_at: i64(now.getTime()),
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
  input: {
    turnId?: string;
    sessionId?: string;
    limit?: number;
  } = {},
  aelioDb?: TurnApiCallsAelioDbConfig,
): Promise<TurnApiCallRecord[]> {
  const limit = Math.min(Math.max(input.limit ?? 200, 1), 1000);

  if (!aelioDb) {
    return [];
  }

  const filters = [];
  if (input.turnId) {
    filters.push({ col: 'turn_id', op: 'eq' as const, value: utf8(input.turnId) });
  }
  if (input.sessionId) {
    filters.push({ col: 'session_id', op: 'eq' as const, value: utf8(input.sessionId) });
  }
  const scan = await aelioDb.client.scanRows(aelioDb.table, {
    k: Math.max(limit * 4, 500),
    filters,
  });
  const rows = scan.rows.map((row) => ({
    id: readUtf8(row.values, 'call_id'),
    turnId: readUtf8(row.values, 'turn_id'),
    sessionId: readUtf8(row.values, 'session_id'),
    customerId: readUtf8(row.values, 'customer_id'),
    sequence: readI64(row.values, 'sequence'),
    callType: readUtf8(row.values, 'call_type') as 'llm' | 'embed',
    purpose: readUtf8(row.values, 'purpose') as TurnApiCallPurpose,
    model: readUtf8(row.values, 'model') || undefined,
    iteration: readI64(row.values, 'iteration') || undefined,
    promptSummary: readUtf8(row.values, 'prompt_summary'),
    inputPreview: readUtf8(row.values, 'input_preview') || undefined,
    messageCount: readI64(row.values, 'message_count') || undefined,
    toolCount: readI64(row.values, 'tool_count') || undefined,
    toolNames: parseJson<string[]>(readUtf8(row.values, 'tool_names'), []),
    stopReason: readUtf8(row.values, 'stop_reason') || undefined,
    durationMs: readI64(row.values, 'duration_ms') || undefined,
    createdAt: readI64(row.values, 'created_at'),
  }));
  rows.sort((a, b) => (input.turnId ? a.sequence - b.sequence : b.createdAt - a.createdAt));
  return rows.slice(0, limit);
}

export async function summarizeTurnApiCalls(
  turnId: string,
  aelioDb?: TurnApiCallsAelioDbConfig,
): Promise<{
  turnId: string;
  totalCalls: number;
  llmCalls: number;
  embedCalls: number;
  calls: TurnApiCallRecord[];
}> {
  const calls = await listTurnApiCalls({ turnId, limit: 100 }, aelioDb);
  return {
    turnId,
    totalCalls: calls.length,
    llmCalls: calls.filter((call) => call.callType === 'llm').length,
    embedCalls: calls.filter((call) => call.callType === 'embed').length,
    calls,
  };
}
