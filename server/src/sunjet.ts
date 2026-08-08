import {
  AelioMemoryStore,
  AelioRuntimeStore,
  AelioSuspensionStore,
  bootstrapSunjetTables,
  createConvoxMessageStore,
  DEFAULT_RUNTIME_ARTIFACTS,
  type ConvoxMessageStore,
} from '@aelio/core';
import type { SunjetTableNames } from '@aelio/core';
import { SunjetClient } from '@aelio/sunjet-client';
import type { AelioConfig } from './config.js';

export type SunjetRuntime = {
  client: SunjetClient;
  messageStore: ConvoxMessageStore;
  tables: SunjetTableNames;
  embedDim: number;
  runtimeStore: AelioRuntimeStore;
  runtimeSuspensionStore: AelioSuspensionStore;
  runtimeMemory: AelioMemoryStore;
};

export async function initSunjet(config: AelioConfig): Promise<SunjetRuntime | null> {
  const sunjet = config.sunjet;
  if (!sunjet.enabled) {
    return null;
  }

  const client = new SunjetClient({
    baseUrl: sunjet.url,
    apiKey: sunjet.api_key,
    timeoutMs: sunjet.timeout_ms,
  });

  await client.health();

  const tables = {
    messages: sunjet.tables.messages,
    conversations: sunjet.tables.conversations,
    memories: sunjet.tables.memories,
    compactions: sunjet.tables.compactions,
    runtimeState: sunjet.tables.runtime_state,
    harnessTools: sunjet.tables.harness_tools,
    harnessCapabilities: sunjet.tables.harness_capabilities,
    harnessBindings: sunjet.tables.harness_bindings,
    harnessSuspensions: sunjet.tables.harness_suspensions,
    harnessLedger: sunjet.tables.harness_ledger,
    harnessTraces: sunjet.tables.harness_traces,
    runtimeEvents: sunjet.tables.runtime_events,
    runtimeSnapshots: sunjet.tables.runtime_snapshots,
    runtimeLedger: sunjet.tables.runtime_ledger,
    runtimeOutbox: sunjet.tables.runtime_outbox,
    runtimeContinuations: sunjet.tables.runtime_continuations,
    scheduledEvents: sunjet.tables.scheduled_events,
    workflowArtifacts: sunjet.tables.workflow_artifacts,
    workflowInstances: sunjet.tables.workflow_instances,
    promptArtifacts: sunjet.tables.prompt_artifacts,
    promptLedger: sunjet.tables.prompt_ledger,
  };

  await bootstrapSunjetTables(client, tables, sunjet.embed_dim);

  // Built-ins are ordinary approved runtime artifacts. Reinstalling is idempotent and lets a
  // fresh Aelio DB boot with a useful, auditable baseline catalog.
  const runtimeStore = new AelioRuntimeStore(client, tables);
  for (const artifact of DEFAULT_RUNTIME_ARTIFACTS) {
    await runtimeStore.installArtifact(artifact);
  }

  const messageStore = createConvoxMessageStore({
    client,
    tables,
    embedDim: sunjet.embed_dim,
    dualWriteSqlite: sunjet.dual_write_sqlite,
    fallbackSqliteOnError: sunjet.fallback_sqlite_on_error,
  });

  return {
    client,
    messageStore,
    tables,
    embedDim: sunjet.embed_dim,
    runtimeStore,
    runtimeSuspensionStore: new AelioSuspensionStore(client, tables, config.name),
    runtimeMemory: new AelioMemoryStore(client, tables, sunjet.embed_dim),
  };
}
