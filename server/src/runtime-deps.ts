import type {
  ConvoxMessageStore,
  ConvoxMemoryStore,
  ConvoxSessionStore,
  ConvoxCustomerStore,
  ConvoxJobStore,
  ConvoxResponseCacheStore,
  ConvoxFunctionCallStore,
  ConvoxReflectionStore,
  ConvoxProactiveStore,
  ConvoxInboundDedupStore,
  ConvoxMagicLinkStore,
  ConvoxSdkConnectionStore,
  ImmediateContextEngine,
  SemanticPathwayEngine,
  ArchetypeEngine,
  ConvoxAxisStore,
  HarnessTracer,
  LighthouseService,
  SuspensionStore,
} from '@aelio/core';
import type { LLMProvider } from '@aelio/llm';
import type { WhatsAppSender } from '@aelio/channels';
import type { SunjetClient } from '@aelio/sunjet-client';
import type { AelioConfig } from './config.js';
import type { ServerSdkBridge } from './sdk-bridge.js';

/** Sunjet-only runtime: every store is backed by Astrolobe — there is no SQLite fallback. */
export type RuntimeDeps = {
  config: AelioConfig;
  llm: LLMProvider;
  sdkBridge: ServerSdkBridge;
  lighthouse: LighthouseService;
  tracer: HarnessTracer | null;
  suspensionStore: SuspensionStore;
  whatsappSender: WhatsAppSender | null;
  sunjetClient: SunjetClient;
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
  /** Immediate Context Engine — time-bucketed short-term context per customer. */
  contextEngine: ImmediateContextEngine;
  /** One-vector semantic decision engine for every incoming message. */
  pathwayEngine: SemanticPathwayEngine;
  /** Archetype valence engine — infers conversational stance for prompt tone shaping. */
  archetypeEngine: ArchetypeEngine;
  /** Harness Axis store — user-specific occurrence chains per aspect. */
  axisStore: ConvoxAxisStore;
};
