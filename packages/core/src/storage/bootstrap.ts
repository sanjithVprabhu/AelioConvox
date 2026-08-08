import type { ColumnSpec, SunjetClient } from '@aelio/sunjet-client';
import type { SunjetTableNames } from './types.js';
import {
  AelioMigrationRunner,
  migrationsTableSchema,
  type AelioMigration,
  type MigrationOutcome,
} from './migrations.js';

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
    // Every operational read is scoped by tenant AND subject. `customer_id` alone is not an
    // isolation boundary: two tenants can legitimately use the same subject id.
    { name: 'tenant_id', kind: 'utf8' },
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

// ---- Stateful runtime tables ---------------------------------------------------------------

function runtimeEventsTableSchema(): ColumnSpec[] {
  return [
    { name: 'event_id', kind: 'utf8' }, { name: 'tenant_id', kind: 'utf8' },
    { name: 'subject_id', kind: 'utf8' }, { name: 'idempotency_key', kind: 'utf8' },
    { name: 'kind', kind: 'utf8' }, { name: 'payload_json', kind: 'text' }, { name: 'received_at', kind: 'i64' },
  ];
}

function runtimeSnapshotsTableSchema(): ColumnSpec[] {
  return [
    { name: 'tenant_id', kind: 'utf8' }, { name: 'subject_id', kind: 'utf8' },
    { name: 'snapshot_key', kind: 'utf8' }, { name: 'revision', kind: 'i64' },
    { name: 'payload_json', kind: 'text' }, { name: 'updated_at', kind: 'i64' },
  ];
}

function runtimeLedgerTableSchema(): ColumnSpec[] {
  return [
    { name: 'record_id', kind: 'utf8' }, { name: 'tenant_id', kind: 'utf8' }, { name: 'subject_id', kind: 'utf8' },
    { name: 'event_id', kind: 'utf8' }, { name: 'instance_id', kind: 'utf8' }, { name: 'kind', kind: 'utf8' },
    { name: 'payload_json', kind: 'text' }, { name: 'created_at', kind: 'i64' },
  ];
}

function runtimeOutboxTableSchema(): ColumnSpec[] {
  return [
    { name: 'effect_id', kind: 'utf8' }, { name: 'tenant_id', kind: 'utf8' }, { name: 'subject_id', kind: 'utf8' },
    { name: 'event_id', kind: 'utf8' }, { name: 'idempotency_key', kind: 'utf8' }, { name: 'kind', kind: 'utf8' },
    { name: 'status', kind: 'utf8' }, { name: 'payload_json', kind: 'text' }, { name: 'attempts', kind: 'i64' },
    { name: 'available_at', kind: 'i64' }, { name: 'lease_token', kind: 'utf8' }, { name: 'lease_expires_at', kind: 'i64' },
    { name: 'last_error', kind: 'text' }, { name: 'revision', kind: 'i64' }, { name: 'created_at', kind: 'i64' }, { name: 'updated_at', kind: 'i64' },
  ];
}

function runtimeContinuationsTableSchema(): ColumnSpec[] {
  return [
    { name: 'token', kind: 'utf8' }, { name: 'tenant_id', kind: 'utf8' }, { name: 'subject_id', kind: 'utf8' },
    { name: 'instance_id', kind: 'utf8' }, { name: 'status', kind: 'utf8' }, { name: 'prompt', kind: 'text' },
    { name: 'payload_json', kind: 'text' }, { name: 'expires_at', kind: 'i64' }, { name: 'revision', kind: 'i64' },
    { name: 'updated_at', kind: 'i64' },
  ];
}

function scheduledEventsTableSchema(): ColumnSpec[] {
  return [
    { name: 'schedule_id', kind: 'utf8' }, { name: 'tenant_id', kind: 'utf8' }, { name: 'subject_id', kind: 'utf8' },
    { name: 'due_at', kind: 'i64' }, { name: 'status', kind: 'utf8' }, { name: 'payload_json', kind: 'text' },
    { name: 'lease_token', kind: 'utf8' }, { name: 'lease_expires_at', kind: 'i64' }, { name: 'revision', kind: 'i64' },
  ];
}

function workflowArtifactsTableSchema(): ColumnSpec[] {
  return [
    { name: 'artifact_id', kind: 'utf8' }, { name: 'version', kind: 'utf8' }, { name: 'digest', kind: 'utf8' },
    { name: 'status', kind: 'utf8' }, { name: 'definition_json', kind: 'text' }, { name: 'created_at', kind: 'i64' },
  ];
}

function workflowInstancesTableSchema(): ColumnSpec[] {
  return [
    { name: 'instance_id', kind: 'utf8' }, { name: 'tenant_id', kind: 'utf8' }, { name: 'subject_id', kind: 'utf8' },
    { name: 'artifact_id', kind: 'utf8' }, { name: 'artifact_version', kind: 'utf8' }, { name: 'parent_instance_id', kind: 'utf8' },
    { name: 'status', kind: 'utf8' }, { name: 'state_json', kind: 'text' }, { name: 'revision', kind: 'i64' },
    { name: 'updated_at', kind: 'i64' },
  ];
}

function promptArtifactsTableSchema(): ColumnSpec[] {
  return [
    { name: 'prompt_id', kind: 'utf8' }, { name: 'version', kind: 'utf8' }, { name: 'digest', kind: 'utf8' },
    { name: 'status', kind: 'utf8' }, { name: 'purpose', kind: 'utf8' }, { name: 'template_json', kind: 'text' },
    { name: 'created_at', kind: 'i64' },
  ];
}

function promptLedgerTableSchema(): ColumnSpec[] {
  return [
    { name: 'ledger_id', kind: 'utf8' }, { name: 'tenant_id', kind: 'utf8' }, { name: 'event_id', kind: 'utf8' },
    { name: 'prompt_id', kind: 'utf8' }, { name: 'prompt_version', kind: 'utf8' }, { name: 'input_digest', kind: 'utf8' },
    { name: 'output_digest', kind: 'utf8' }, { name: 'created_at', kind: 'i64' },
  ];
}

/**
 * Every schema change this build knows about, in order.
 *
 * Append here; never edit an entry that has shipped. The runner records each by id and refuses to
 * proceed if an applied migration's definition has changed underneath it, so an accidental edit
 * surfaces at boot rather than as corrupt rows later.
 */
export function aelioSchemaMigrations(embedDim: number): AelioMigration[] {
  return [
    {
      id: '0001-conversation-core',
      version: 1,
      description: 'Messages, conversations, memories, compactions, and runtime key/value state.',
      kind: 'additive',
      apply: async (client, tables) => {
        await client.ensureTable(tables.messages, messagesTableSchema(embedDim));
        await client.ensureTable(tables.conversations, conversationsTableSchema(embedDim));
        await client.ensureTable(tables.memories, memoriesTableSchema(embedDim));
        await client.ensureTable(tables.compactions, compactionsTableSchema(embedDim));
        await client.ensureTable(tables.runtimeState, runtimeStateTableSchema());
      },
    },
    {
      id: '0002-harness-catalog',
      version: 2,
      description: 'Lighthouse tool/capability mirrors, binding cache, suspensions, ledger, traces.',
      kind: 'additive',
      apply: async (client, tables) => {
        await client.ensureTable(tables.harnessTools, harnessToolsTableSchema(embedDim));
        await client.ensureTable(tables.harnessCapabilities, harnessCapabilitiesTableSchema(embedDim));
        await client.ensureTable(tables.harnessBindings, harnessBindingsTableSchema(embedDim));
        await client.ensureTable(tables.harnessSuspensions, harnessSuspensionsTableSchema());
        await client.ensureTable(tables.harnessLedger, harnessLedgerTableSchema());
        await client.ensureTable(tables.harnessTraces, harnessTracesTableSchema(embedDim));
      },
    },
    {
      id: '0003-stateful-runtime',
      version: 3,
      description: 'Event inbox, subject snapshots, ledger, outbox, continuations, schedules, artifacts, instances, prompts.',
      kind: 'additive',
      apply: async (client, tables) => {
        await client.ensureTable(tables.runtimeEvents, runtimeEventsTableSchema());
        await client.ensureTable(tables.runtimeSnapshots, runtimeSnapshotsTableSchema());
        await client.ensureTable(tables.runtimeLedger, runtimeLedgerTableSchema());
        await client.ensureTable(tables.runtimeOutbox, runtimeOutboxTableSchema());
        await client.ensureTable(tables.runtimeContinuations, runtimeContinuationsTableSchema());
        await client.ensureTable(tables.scheduledEvents, scheduledEventsTableSchema());
        await client.ensureTable(tables.workflowArtifacts, workflowArtifactsTableSchema());
        await client.ensureTable(tables.workflowInstances, workflowInstancesTableSchema());
        await client.ensureTable(tables.promptArtifacts, promptArtifactsTableSchema());
        await client.ensureTable(tables.promptLedger, promptLedgerTableSchema());
      },
    },
    {
      id: '0004-memory-tenant-scope',
      version: 4,
      description: 'Add tenant_id to the memories table so recall can never cross a tenant boundary.',
      kind: 'additive',
      apply: async (client, tables) => {
        // Additive and nullable. Pre-existing rows have no tenant, so a tenant-scoped read simply
        // does not match them — memory degrades rather than leaking, which is the safe direction.
        await client.ensureTable(tables.memories, [
          ...memoriesTableSchema(embedDim),
          { name: 'tenant_id', kind: 'utf8' },
        ]);
      },
    },
  ];
}

/**
 * Bring an Aelio DB up to the schema this build requires, under the migration lock.
 *
 * `ensureTable` is additive and idempotent, so re-running is safe; the ledger exists so the
 * deployment can *prove* which schema a database is on, refuse an older binary against a newer
 * schema, and serialize concurrent replicas booting together.
 */
export async function migrateAelioStorage(
  client: SunjetClient,
  tables: SunjetTableNames,
  embedDim: number,
  options: { allowDestructive?: boolean } = {},
): Promise<MigrationOutcome> {
  await client.ensureTable(tables.migrations, migrationsTableSchema());
  const runner = new AelioMigrationRunner(client, tables);
  return runner.migrate(aelioSchemaMigrations(embedDim), options);
}

/**
 * Direct table creation without the migration ledger. Retained for tests and tooling that want a
 * scratch database; production boot goes through `migrateAelioStorage`.
 */
export async function bootstrapSunjetTables(
  client: SunjetClient,
  tables: SunjetTableNames,
  embedDim: number,
): Promise<void> {
  await client.ensureTable(tables.migrations, migrationsTableSchema());
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
  await client.ensureTable(tables.runtimeEvents, runtimeEventsTableSchema());
  await client.ensureTable(tables.runtimeSnapshots, runtimeSnapshotsTableSchema());
  await client.ensureTable(tables.runtimeLedger, runtimeLedgerTableSchema());
  await client.ensureTable(tables.runtimeOutbox, runtimeOutboxTableSchema());
  await client.ensureTable(tables.runtimeContinuations, runtimeContinuationsTableSchema());
  await client.ensureTable(tables.scheduledEvents, scheduledEventsTableSchema());
  await client.ensureTable(tables.workflowArtifacts, workflowArtifactsTableSchema());
  await client.ensureTable(tables.workflowInstances, workflowInstancesTableSchema());
  await client.ensureTable(tables.promptArtifacts, promptArtifactsTableSchema());
  await client.ensureTable(tables.promptLedger, promptLedgerTableSchema());
}
