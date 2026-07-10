import {
  bootstrapSunjetTables,
  createConvoxMessageStore,
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
  };

  await bootstrapSunjetTables(client, tables, sunjet.embed_dim);

  const messageStore = createConvoxMessageStore({
    client,
    tables,
    embedDim: sunjet.embed_dim,
    dualWriteSqlite: sunjet.dual_write_sqlite,
    fallbackSqliteOnError: sunjet.fallback_sqlite_on_error,
  });

  return { client, messageStore, tables, embedDim: sunjet.embed_dim };
}