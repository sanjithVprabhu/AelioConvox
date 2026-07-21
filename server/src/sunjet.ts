import {
  bootstrapSunjetTables,
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
  type SunjetTableNames,
  type SunjetStorageConfig,
} from '@aelio/core';
import { SunjetClient } from '@aelio/sunjet-client';
import type { AelioConfig } from './config.js';

export type SunjetStores = {
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

export type SunjetRuntime = SunjetStores & {
  client: SunjetClient;
  tables: SunjetTableNames;
  embedDim: number;
  storageConfig: SunjetStorageConfig;
};

/** Sunjet-only server: this is required to succeed — callers must not fall back to any other store. */
export async function initSunjet(config: AelioConfig): Promise<SunjetRuntime> {
  const sunjet = config.sunjet;
  if (!sunjet.enabled) {
    throw new Error('sunjet.enabled must be true — Aelio is Sunjet-only and has no fallback store');
  }

  const client = new SunjetClient({
    baseUrl: sunjet.url,
    apiKey: sunjet.api_key,
    timeoutMs: sunjet.timeout_ms,
  });

  await client.health();

  const tables: SunjetTableNames = {
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
    customers: sunjet.tables.customers,
    channelAddresses: sunjet.tables.channel_addresses,
    sessions: sunjet.tables.sessions,
    jobQueue: sunjet.tables.job_queue,
    responseCache: sunjet.tables.response_cache,
    functionCalls: sunjet.tables.function_calls,
    turnApiCalls: sunjet.tables.turn_api_calls,
    reflections: sunjet.tables.reflections,
    proactiveMessages: sunjet.tables.proactive_messages,
    inboundDedup: sunjet.tables.inbound_dedup,
    magicLinks: sunjet.tables.magic_links,
    sdkConnections: sunjet.tables.sdk_connections,
    archetypes: sunjet.tables.archetypes,
    aspects: sunjet.tables.aspects,
    axisNodes: sunjet.tables.axis_nodes,
  };

  await bootstrapSunjetTables(client, tables, sunjet.embed_dim);

  const storageConfig: SunjetStorageConfig = {
    client,
    tables,
    embedDim: sunjet.embed_dim,
  };

  return {
    client,
    tables,
    embedDim: sunjet.embed_dim,
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
