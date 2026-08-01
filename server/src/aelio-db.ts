import {
  bootstrapAelioDbTables,
  createConvoxMessageStore,
  createConvoxMemoryStore,
  createConvoxSessionStore,
  createConvoxCustomerStore,
  createConvoxJobStore,
  createConvoxResponseCacheStore,
  createConvoxFunctionCallStore,
  createConvoxReflectionStore,
  createConvoxProactiveStore,
  createConvoxInboundDedupStore,
  createConvoxMagicLinkStore,
  createConvoxSdkConnectionStore,
  createConvoxArchetypeStore,
  createConvoxAspectStore,
  createConvoxAxisStore,
  type ConvoxMessageStore,
  type ConvoxMemoryStore,
  type ConvoxSessionStore,
  type ConvoxCustomerStore,
  type ConvoxJobStore,
  type ConvoxResponseCacheStore,
  type ConvoxFunctionCallStore,
  type ConvoxReflectionStore,
  type ConvoxProactiveStore,
  type ConvoxInboundDedupStore,
  type ConvoxMagicLinkStore,
  type ConvoxSdkConnectionStore,
  type ConvoxArchetypeStore,
  type ConvoxAspectStore,
  type ConvoxAxisStore,
  type AelioDbTableNames,
  type AelioDbStorageConfig,
} from '@aelio/core/edge';
import { AelioDbClient } from '@aelio/db-client';
import type { AelioConfig } from './config.js';

export type AelioDbStores = {
  messageStore: ConvoxMessageStore;
  memoryStore: ConvoxMemoryStore;
  sessionStore: ConvoxSessionStore;
  customerStore: ConvoxCustomerStore;
  jobStore: ConvoxJobStore;
  responseCacheStore: ConvoxResponseCacheStore;
  functionCallStore: ConvoxFunctionCallStore;
  reflectionStore: ConvoxReflectionStore;
  proactiveStore: ConvoxProactiveStore;
  inboundDedupStore: ConvoxInboundDedupStore;
  magicLinkStore: ConvoxMagicLinkStore;
  sdkConnectionStore: ConvoxSdkConnectionStore;
  archetypeStore: ConvoxArchetypeStore;
  aspectStore: ConvoxAspectStore;
  axisStore: ConvoxAxisStore;
};

export type AelioDbRuntime = AelioDbStores & {
  client: AelioDbClient;
  tables: AelioDbTableNames;
  embedDim: number;
  storageConfig: AelioDbStorageConfig;
};

/** The Rust Aelio database is required; callers must not fall back to another store. */
export async function initAelioDb(config: AelioConfig): Promise<AelioDbRuntime> {
  const aelioDb = config.aelioDb;
  if (!aelioDb.enabled) {
    throw new Error('aelioDb.enabled must be true — Aelio has no fallback store');
  }

  const client = new AelioDbClient({
    baseUrl: aelioDb.url,
    apiKey: aelioDb.api_key,
    timeoutMs: aelioDb.timeout_ms,
  });

  await client.health();

  const tables: AelioDbTableNames = {
    messages: aelioDb.tables.messages,
    conversations: aelioDb.tables.conversations,
    memories: aelioDb.tables.memories,
    compactions: aelioDb.tables.compactions,
    runtimeState: aelioDb.tables.runtime_state,
    harnessTools: aelioDb.tables.harness_tools,
    harnessCapabilities: aelioDb.tables.harness_capabilities,
    harnessBindings: aelioDb.tables.harness_bindings,
    harnessSuspensions: aelioDb.tables.harness_suspensions,
    harnessLedger: aelioDb.tables.harness_ledger,
    harnessTraces: aelioDb.tables.harness_traces,
    customers: aelioDb.tables.customers,
    channelAddresses: aelioDb.tables.channel_addresses,
    sessions: aelioDb.tables.sessions,
    jobQueue: aelioDb.tables.job_queue,
    responseCache: aelioDb.tables.response_cache,
    functionCalls: aelioDb.tables.function_calls,
    turnApiCalls: aelioDb.tables.turn_api_calls,
    reflections: aelioDb.tables.reflections,
    proactiveMessages: aelioDb.tables.proactive_messages,
    inboundDedup: aelioDb.tables.inbound_dedup,
    magicLinks: aelioDb.tables.magic_links,
    sdkConnections: aelioDb.tables.sdk_connections,
    archetypes: aelioDb.tables.archetypes,
    aspects: aelioDb.tables.aspects,
    axisNodes: aelioDb.tables.axis_nodes,
  };

  await bootstrapAelioDbTables(client, tables, aelioDb.embed_dim);

  const storageConfig: AelioDbStorageConfig = {
    client,
    tables,
    embedDim: aelioDb.embed_dim,
  };

  return {
    client,
    tables,
    embedDim: aelioDb.embed_dim,
    storageConfig,
    messageStore: createConvoxMessageStore(storageConfig),
    memoryStore: createConvoxMemoryStore(storageConfig),
    sessionStore: createConvoxSessionStore(storageConfig),
    customerStore: createConvoxCustomerStore(storageConfig),
    jobStore: createConvoxJobStore(storageConfig),
    responseCacheStore: createConvoxResponseCacheStore(storageConfig),
    functionCallStore: createConvoxFunctionCallStore(storageConfig),
    reflectionStore: createConvoxReflectionStore(storageConfig),
    proactiveStore: createConvoxProactiveStore(storageConfig),
    inboundDedupStore: createConvoxInboundDedupStore(storageConfig),
    magicLinkStore: createConvoxMagicLinkStore(storageConfig),
    sdkConnectionStore: createConvoxSdkConnectionStore(storageConfig),
    archetypeStore: createConvoxArchetypeStore(storageConfig),
    aspectStore: createConvoxAspectStore(storageConfig),
    axisStore: createConvoxAxisStore(storageConfig),
  };
}
