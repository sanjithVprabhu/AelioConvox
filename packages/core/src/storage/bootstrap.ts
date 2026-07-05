import type { ColumnSpec, SunjetClient } from '@aelio/sunjet-client';
import type { SunjetTableNames } from './types.js';

function vectorColumn(name: string, dim: number): ColumnSpec {
  return { name, kind: 'vector', dim };
}

function messagesTableSchema(embedDim: number): ColumnSpec[] {
  return [
    { name: 'message_id', kind: 'utf8' },
    { name: 'session_id', kind: 'utf8' },
    { name: 'customer_id', kind: 'utf8' },
    { name: 'role', kind: 'utf8' },
    { name: 'content', kind: 'text' },
    { name: 'channel', kind: 'utf8' },
    { name: 'tier', kind: 'i64' },
    { name: 'created_at', kind: 'i64' },
    vectorColumn('embedding', embedDim),
    { name: 'parent_ids', kind: 'edge' },
  ];
}

function memoriesTableSchema(embedDim: number): ColumnSpec[] {
  return [
    { name: 'memory_id', kind: 'utf8' },
    { name: 'customer_id', kind: 'utf8' },
    { name: 'content', kind: 'text' },
    vectorColumn('embedding', embedDim),
    { name: 'category', kind: 'utf8' },
    { name: 'source_session_id', kind: 'utf8' },
    { name: 'confidence', kind: 'f64' },
    { name: 'created_at', kind: 'i64' },
    { name: 'expires_at', kind: 'i64' },
  ];
}

function compactionsTableSchema(embedDim: number): ColumnSpec[] {
  return [
    { name: 'compaction_id', kind: 'utf8' },
    { name: 'customer_id', kind: 'utf8' },
    { name: 'session_id', kind: 'utf8' },
    { name: 'tier', kind: 'i64' },
    { name: 'label', kind: 'utf8' },
    { name: 'content', kind: 'text' },
    vectorColumn('embedding', embedDim),
    { name: 'created_at', kind: 'i64' },
    { name: 'covers_from', kind: 'i64' },
    { name: 'covers_to', kind: 'i64' },
    { name: 'child_ids', kind: 'edge' },
  ];
}

function conversationsTableSchema(embedDim: number): ColumnSpec[] {
  return [
    { name: 'event_id', kind: 'utf8' },
    { name: 'message_id', kind: 'utf8' },
    { name: 'session_id', kind: 'utf8' },
    { name: 'customer_id', kind: 'utf8' },
    { name: 'customer_external_id', kind: 'utf8' },
    { name: 'channel', kind: 'utf8' },
    { name: 'channel_address', kind: 'utf8' },
    { name: 'role', kind: 'utf8' },
    { name: 'content', kind: 'text' },
    vectorColumn('embedding', embedDim),
    { name: 'created_at', kind: 'i64' },
    { name: 'lifecycle_state', kind: 'utf8' },
    { name: 'lifecycle_state_reason', kind: 'utf8' },
    { name: 'intent_label', kind: 'utf8' },
    { name: 'intent_summary', kind: 'utf8' },
    { name: 'intent_stack', kind: 'text' },
    { name: 'flow_id', kind: 'utf8' },
    { name: 'flow_step_id', kind: 'utf8' },
    { name: 'flow_step_index', kind: 'i64' },
    { name: 'flow_step_goal', kind: 'utf8' },
    { name: 'active_policies', kind: 'text' },
    { name: 'tools_executed', kind: 'text' },
    { name: 'pending_confirmation', kind: 'bool' },
  ];
}

function runtimeStateTableSchema(): ColumnSpec[] {
  return [
    { name: 'scope', kind: 'utf8' },
    { name: 'kind', kind: 'utf8' },
    { name: 'payload', kind: 'text' },
    { name: 'expires_at', kind: 'i64' },
    { name: 'updated_at', kind: 'i64' },
  ];
}

export async function bootstrapSunjetTables(
  client: SunjetClient,
  tables: SunjetTableNames,
  embedDim: number,
): Promise<void> {
  await client.ensureTable(tables.messages, messagesTableSchema(embedDim));
  await client.ensureTable(tables.conversations, conversationsTableSchema(embedDim));
  await client.ensureTable(tables.memories, memoriesTableSchema(embedDim));
  await client.ensureTable(tables.compactions, compactionsTableSchema(embedDim));
  await client.ensureTable(tables.runtimeState, runtimeStateTableSchema());
}