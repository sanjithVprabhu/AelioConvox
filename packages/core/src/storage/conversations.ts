import { randomUUID } from 'node:crypto';
import type { ApiValue } from '@aelio/sunjet-client';
import { embed } from '../analyst/embeddings.js';
import type { ConversationTurnContext } from './context.js';
import type { MessageStoreAppendInput, SunjetStorageConfig } from './types.js';

function utf8(value: string): ApiValue {
  return { type: 'utf8', value };
}

function i64(value: number): ApiValue {
  return { type: 'i64', value };
}

function bool(value: boolean): ApiValue {
  return { type: 'bool', value };
}

function optionalUtf8(value: string | undefined): ApiValue {
  return value ? utf8(value) : { type: 'null' };
}

function optionalI64(value: number | undefined): ApiValue {
  return value === undefined ? { type: 'null' } : i64(value);
}

const warnedDims = new Set<string>();

function normalizeEmbedding(vector: number[], dim: number): number[] {
  if (vector.length === dim) {
    return vector;
  }
  // A mismatch means the configured embeddings.output_dimension does not match
  // sunjet.embed_dim — vectors are being silently reshaped, which degrades
  // similarity search. Surface it once instead of hiding it.
  const key = `${vector.length}->${dim}`;
  if (!warnedDims.has(key)) {
    warnedDims.add(key);
    console.warn(
      `[aelio] embedding dimension mismatch: provider returned ${vector.length}, table expects ${dim}. ` +
        'Align embeddings.output_dimension with sunjet.embed_dim to avoid degraded semantic search.',
    );
  }
  if (vector.length > dim) {
    return vector.slice(0, dim);
  }
  return [...vector, ...new Array<number>(dim - vector.length).fill(0)];
}

export async function appendConversationRecord(
  config: Pick<SunjetStorageConfig, 'client' | 'tables' | 'embedDim'>,
  input: MessageStoreAppendInput & {
    messageId: string;
    createdAt: number;
    context?: ConversationTurnContext;
  },
): Promise<void> {
  const table = config.tables.conversations;
  if (!table) {
    return;
  }

  const content = input.content.trim();
  const values: Record<string, ApiValue> = {
    event_id: utf8(randomUUID()),
    message_id: utf8(input.messageId),
    session_id: utf8(input.sessionId),
    customer_id: utf8(input.customerId),
    customer_external_id: utf8(input.context?.customerExternalId ?? ''),
    channel: utf8(input.channel),
    channel_address: utf8(input.context?.channelAddress ?? ''),
    role: utf8(input.role),
    content: utf8(content),
    created_at: i64(input.createdAt),
    lifecycle_state: optionalUtf8(input.context?.lifecycleState),
    lifecycle_state_reason: optionalUtf8(input.context?.lifecycleStateReason),
    intent_label: optionalUtf8(input.context?.intentLabel),
    intent_summary: optionalUtf8(input.context?.intentSummary),
    intent_stack: optionalUtf8(input.context?.intentStackJson),
    flow_id: optionalUtf8(input.context?.flowId),
    flow_step_id: optionalUtf8(input.context?.flowStepId),
    flow_step_index: optionalI64(input.context?.flowStepIndex),
    flow_step_goal: optionalUtf8(input.context?.flowStepGoal),
    active_policies: optionalUtf8(input.context?.activePoliciesJson),
    tools_executed: optionalUtf8(input.context?.toolsExecutedJson),
    pending_confirmation: bool(input.context?.pendingConfirmation ?? false),
  };

  if (content.length > 0) {
    try {
      const embedding = normalizeEmbedding(
        await embed(content, { purpose: 'conversation_archive' }),
        config.embedDim,
      );
      values.embedding = { type: 'vector', value: embedding };
    } catch {
      // Optional embedding for semantic search over conversation archive.
    }
  }

  await config.client.insertRow(table, values);
}