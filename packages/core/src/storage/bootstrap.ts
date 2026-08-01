import type { ColumnSpec, AelioDbClient } from '@aelio/db-client';
import type { AelioDbTableNames } from './types.js';

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

// ---- Harness tables ----

/** Lighthouse tool mirror: one row per registered tool, re-synced per registry hash. */
function harnessToolsTableSchema(embedDim: number): ColumnSpec[] {
  return [
    { name: 'tenant', kind: 'utf8' },
    { name: 'registry_hash', kind: 'utf8' },
    { name: 'name', kind: 'utf8' },
    { name: 'description', kind: 'text' },
    { name: 'intent', kind: 'utf8' },
    { name: 'safety', kind: 'utf8' },
    { name: 'params_json', kind: 'text' },
    vectorColumn('embedding', embedDim),
    // Retrieval-expansion edges: consumer tool → likely prerequisite tools.
    { name: 'requires', kind: 'edge' },
  ];
}

/** Capability taxonomy nodes (categories) for planner grounding + feasibility probes. */
function harnessCapabilitiesTableSchema(embedDim: number): ColumnSpec[] {
  return [
    { name: 'tenant', kind: 'utf8' },
    { name: 'registry_hash', kind: 'utf8' },
    { name: 'category', kind: 'utf8' },
    { name: 'content', kind: 'text' },
    vectorColumn('embedding', embedDim),
  ];
}

/** Instruction→tool binding cache. Keyed by content hash; stores selection only, never args. */
function harnessBindingsTableSchema(embedDim: number): ColumnSpec[] {
  return [
    { name: 'binding_key', kind: 'utf8' },
    { name: 'tenant', kind: 'utf8' },
    { name: 'registry_hash', kind: 'utf8' },
    { name: 'instruction', kind: 'text' },
    vectorColumn('embedding', embedDim),
    { name: 'tool_name', kind: 'utf8' },
    { name: 'created_at', kind: 'i64' },
    { name: 'expires_at', kind: 'i64' },
  ];
}

/** Suspended plans awaiting user input (recoil) or confirmation. */
function harnessSuspensionsTableSchema(): ColumnSpec[] {
  return [
    { name: 'session_id', kind: 'utf8' },
    { name: 'tenant', kind: 'utf8' },
    { name: 'reason', kind: 'utf8' },
    { name: 'payload', kind: 'text' },
    { name: 'created_at', kind: 'i64' },
    { name: 'expires_at', kind: 'i64' },
  ];
}

/** Per-instruction execution ledger — the idempotency/replay guard. */
function harnessLedgerTableSchema(): ColumnSpec[] {
  return [
    { name: 'session_id', kind: 'utf8' },
    { name: 'turn_id', kind: 'utf8' },
    { name: 'instruction_id', kind: 'utf8' },
    { name: 'args_hash', kind: 'utf8' },
    { name: 'status', kind: 'utf8' },
    { name: 'result_json', kind: 'text' },
    { name: 'created_at', kind: 'i64' },
  ];
}

/** Append-only harness trace firehose (plans, waves, gates, repairs) for audit/compaction. */
function harnessTracesTableSchema(embedDim: number): ColumnSpec[] {
  return [
    { name: 'turn_id', kind: 'utf8' },
    { name: 'session_id', kind: 'utf8' },
    { name: 'tenant', kind: 'utf8' },
    { name: 'kind', kind: 'utf8' },
    { name: 'payload', kind: 'text' },
    vectorColumn('embedding', embedDim),
    { name: 'tier', kind: 'i64' },
    { name: 'created_at', kind: 'i64' },
  ];
}

// ---- Identity / session / lifecycle tables ----

function customersTableSchema(): ColumnSpec[] {
  return [
    { name: 'customer_id', kind: 'utf8' },
    { name: 'external_id', kind: 'utf8' },
    { name: 'display_name', kind: 'utf8' },
    { name: 'metadata', kind: 'utf8' },
    { name: 'created_at', kind: 'i64' },
    { name: 'updated_at', kind: 'i64' },
  ];
}

function channelAddressesTableSchema(): ColumnSpec[] {
  return [
    { name: 'address_id', kind: 'utf8' },
    { name: 'customer_id', kind: 'utf8' },
    { name: 'channel', kind: 'utf8' },
    { name: 'address', kind: 'utf8' },
    { name: 'verified_at', kind: 'i64' },
    { name: 'created_at', kind: 'i64' },
  ];
}

function sessionsTableSchema(): ColumnSpec[] {
  return [
    { name: 'session_id', kind: 'utf8' },
    { name: 'customer_id', kind: 'utf8' },
    { name: 'channel', kind: 'utf8' },
    { name: 'status', kind: 'utf8' },
    { name: 'started_at', kind: 'i64' },
    { name: 'last_activity_at', kind: 'i64' },
    { name: 'closed_at', kind: 'i64' },
    { name: 'summary', kind: 'text' },
    { name: 'metadata', kind: 'utf8' },
  ];
}

function jobQueueTableSchema(): ColumnSpec[] {
  return [
    { name: 'job_id', kind: 'utf8' },
    { name: 'queue', kind: 'utf8' },
    { name: 'payload', kind: 'text' },
    { name: 'status', kind: 'utf8' },
    { name: 'attempts', kind: 'i64' },
    { name: 'max_attempts', kind: 'i64' },
    { name: 'next_run_at', kind: 'i64' },
    { name: 'locked_by', kind: 'utf8' },
    { name: 'locked_at', kind: 'i64' },
    { name: 'error_message', kind: 'text' },
    { name: 'created_at', kind: 'i64' },
    { name: 'completed_at', kind: 'i64' },
  ];
}

function responseCacheTableSchema(embedDim: number): ColumnSpec[] {
  return [
    { name: 'cache_id', kind: 'utf8' },
    { name: 'customer_id', kind: 'utf8' },
    { name: 'query', kind: 'text' },
    vectorColumn('embedding', embedDim),
    { name: 'reply', kind: 'text' },
    { name: 'hits', kind: 'i64' },
    { name: 'created_at', kind: 'i64' },
    { name: 'expires_at', kind: 'i64' },
  ];
}

function functionCallsTableSchema(): ColumnSpec[] {
  return [
    { name: 'call_id', kind: 'utf8' },
    { name: 'session_id', kind: 'utf8' },
    { name: 'customer_id', kind: 'utf8' },
    { name: 'function_name', kind: 'utf8' },
    { name: 'args_json', kind: 'text' },
    { name: 'result_json', kind: 'text' },
    { name: 'status', kind: 'utf8' },
    { name: 'safety_level', kind: 'utf8' },
    { name: 'required_confirmation', kind: 'bool' },
    { name: 'confirmed', kind: 'bool' },
    { name: 'duration_ms', kind: 'i64' },
    { name: 'error_message', kind: 'text' },
    { name: 'created_at', kind: 'i64' },
  ];
}

function turnApiCallsTableSchema(): ColumnSpec[] {
  return [
    { name: 'call_id', kind: 'utf8' },
    { name: 'turn_id', kind: 'utf8' },
    { name: 'session_id', kind: 'utf8' },
    { name: 'customer_id', kind: 'utf8' },
    { name: 'sequence', kind: 'i64' },
    { name: 'call_type', kind: 'utf8' },
    { name: 'purpose', kind: 'utf8' },
    { name: 'model', kind: 'utf8' },
    { name: 'iteration', kind: 'i64' },
    { name: 'prompt_summary', kind: 'text' },
    { name: 'input_preview', kind: 'text' },
    { name: 'message_count', kind: 'i64' },
    { name: 'tool_count', kind: 'i64' },
    { name: 'tool_names', kind: 'text' },
    { name: 'stop_reason', kind: 'utf8' },
    { name: 'duration_ms', kind: 'i64' },
    { name: 'tokens_in', kind: 'i64' },
    { name: 'tokens_out', kind: 'i64' },
    { name: 'created_at', kind: 'i64' },
  ];
}

function reflectionsTableSchema(): ColumnSpec[] {
  return [
    { name: 'reflection_id', kind: 'utf8' },
    { name: 'session_id', kind: 'utf8' },
    { name: 'customer_id', kind: 'utf8' },
    { name: 'outcome', kind: 'utf8' },
    { name: 'score', kind: 'f64' },
    { name: 'summary', kind: 'text' },
    { name: 'issues_json', kind: 'text' },
    { name: 'insight', kind: 'text' },
    { name: 'followup', kind: 'text' },
    { name: 'created_at', kind: 'i64' },
  ];
}

function proactiveMessagesTableSchema(): ColumnSpec[] {
  return [
    { name: 'proactive_id', kind: 'utf8' },
    { name: 'customer_id', kind: 'utf8' },
    { name: 'channel', kind: 'utf8' },
    { name: 'to_address', kind: 'utf8' },
    { name: 'content', kind: 'text' },
    { name: 'dedup_key', kind: 'utf8' },
    { name: 'status', kind: 'utf8' },
    { name: 'reason', kind: 'text' },
    { name: 'created_at', kind: 'i64' },
  ];
}

function inboundDedupTableSchema(): ColumnSpec[] {
  return [
    { name: 'message_id', kind: 'utf8' },
    { name: 'created_at', kind: 'i64' },
  ];
}

function magicLinksTableSchema(): ColumnSpec[] {
  return [
    { name: 'link_id', kind: 'utf8' },
    { name: 'token_hash', kind: 'utf8' },
    { name: 'email', kind: 'utf8' },
    { name: 'customer_id', kind: 'utf8' },
    { name: 'external_id', kind: 'utf8' },
    { name: 'expires_at', kind: 'i64' },
    { name: 'consumed_at', kind: 'i64' },
    { name: 'created_at', kind: 'i64' },
  ];
}

/**
 * Archetype exemplars for the valence engine: one row per (category, valence)
 * bucket. The embedding is the semantic signature of the bucket (keyword +
 * description + how it is conveyed/used/inferred); `guidance` is the response
 * instruction fed into the system prompt when this bucket wins.
 */
function archetypesTableSchema(embedDim: number): ColumnSpec[] {
  return [
    { name: 'archetype_id', kind: 'utf8' },
    { name: 'tenant', kind: 'utf8' },
    { name: 'aspect_id', kind: 'utf8' },
    { name: 'category', kind: 'utf8' },
    { name: 'valence', kind: 'utf8' },
    { name: 'keyword', kind: 'utf8' },
    { name: 'description', kind: 'text' },
    { name: 'usage', kind: 'text' },
    { name: 'inference', kind: 'text' },
    { name: 'guidance', kind: 'text' },
    vectorColumn('embedding', embedDim),
    { name: 'created_at', kind: 'i64' },
  ];
}

/**
 * The "mother" collection for the self-learning stance layer: a registry of the
 * conversational "things" (aspects) the system has identified. Each aspect owns
 * a pos/neu/neg bucket set in the archetypes table (linked by aspect_id). Rows
 * start as `candidate` (source=discovered) and promote to `active` once seen
 * enough times; the seeded defaults are `active` (source=builtin).
 */
function aspectsTableSchema(embedDim: number): ColumnSpec[] {
  return [
    { name: 'aspect_id', kind: 'utf8' },
    { name: 'tenant', kind: 'utf8' },
    { name: 'name', kind: 'utf8' },
    { name: 'description', kind: 'text' },
    vectorColumn('embedding', embedDim),
    { name: 'status', kind: 'utf8' },
    { name: 'source', kind: 'utf8' },
    { name: 'hits', kind: 'i64' },
    { name: 'created_at', kind: 'i64' },
    { name: 'last_seen_at', kind: 'i64' },
  ];
}

/**
 * Harness Axis graph: roots + occurrences in one table so `previous`/`head`
 * edges stay intra-table (AelioDb graph queries are per-table).
 */
function axisNodesTableSchema(embedDim: number): ColumnSpec[] {
  return [
    { name: 'node_id', kind: 'utf8' },
    { name: 'tenant', kind: 'utf8' },
    { name: 'node_type', kind: 'utf8' },
    { name: 'customer_id', kind: 'utf8' },
    { name: 'aspect_id', kind: 'utf8' },
    { name: 'aspect_name', kind: 'utf8' },
    { name: 'occurrence_id', kind: 'utf8' },
    { name: 'turn_id', kind: 'utf8' },
    { name: 'message_id', kind: 'utf8' },
    { name: 'session_id', kind: 'utf8' },
    { name: 'span', kind: 'text' },
    { name: 'valence', kind: 'utf8' },
    { name: 'positive', kind: 'f64' },
    { name: 'negative', kind: 'f64' },
    { name: 'neutral', kind: 'f64' },
    { name: 'strength', kind: 'f64' },
    { name: 'intent_label', kind: 'utf8' },
    { name: 'flow_id', kind: 'utf8' },
    vectorColumn('embedding', embedDim),
    { name: 'created_at', kind: 'i64' },
    { name: 'previous', kind: 'edge' },
    { name: 'head', kind: 'edge' },
  ];
}

function sdkConnectionsTableSchema(): ColumnSpec[] {
  return [
    { name: 'connection_id', kind: 'utf8' },
    { name: 'sdk_version', kind: 'utf8' },
    { name: 'language', kind: 'utf8' },
    { name: 'connected_at', kind: 'i64' },
    { name: 'last_heartbeat_at', kind: 'i64' },
    { name: 'functions_json', kind: 'text' },
  ];
}

export async function bootstrapAelioDbTables(
  client: AelioDbClient,
  tables: AelioDbTableNames,
  embedDim: number,
): Promise<void> {
  await client.ensureTable(tables.messages, messagesTableSchema(embedDim));
  await client.ensureTable(tables.conversations, conversationsTableSchema(embedDim));
  await client.ensureTable(tables.memories, memoriesTableSchema(embedDim));
  await client.ensureTable(tables.compactions, compactionsTableSchema(embedDim));
  await client.ensureTable(tables.runtimeState, runtimeStateTableSchema());
  await client.ensureTable(tables.harnessTools, harnessToolsTableSchema(embedDim));
  await client.ensureTable(tables.harnessCapabilities, harnessCapabilitiesTableSchema(embedDim));
  await client.ensureTable(tables.harnessBindings, harnessBindingsTableSchema(embedDim));
  await client.ensureTable(tables.harnessSuspensions, harnessSuspensionsTableSchema());
  await client.ensureTable(tables.harnessLedger, harnessLedgerTableSchema());
  await client.ensureTable(tables.harnessTraces, harnessTracesTableSchema(embedDim));

  await client.ensureTable(tables.customers, customersTableSchema());
  await client.ensureTable(tables.channelAddresses, channelAddressesTableSchema());
  await client.ensureTable(tables.sessions, sessionsTableSchema());
  await client.ensureTable(tables.jobQueue, jobQueueTableSchema());
  await client.ensureTable(tables.responseCache, responseCacheTableSchema(embedDim));
  await client.ensureTable(tables.functionCalls, functionCallsTableSchema());
  await client.ensureTable(tables.turnApiCalls, turnApiCallsTableSchema());
  await client.ensureTable(tables.reflections, reflectionsTableSchema());
  await client.ensureTable(tables.proactiveMessages, proactiveMessagesTableSchema());
  await client.ensureTable(tables.inboundDedup, inboundDedupTableSchema());
  await client.ensureTable(tables.magicLinks, magicLinksTableSchema());
  await client.ensureTable(tables.sdkConnections, sdkConnectionsTableSchema());
  await client.ensureTable(tables.archetypes, archetypesTableSchema(embedDim));
  await client.ensureTable(tables.aspects, aspectsTableSchema(embedDim));
  await client.ensureTable(tables.axisNodes, axisNodesTableSchema(embedDim));
}