import type {
  ConvoxMessageStore,
  ConvoxMemoryStore,
  ConvoxSessionStore,
  ConvoxCustomerStore,
  ConvoxJobStore,
  ConvoxInboundDedupStore,
  ConvoxMagicLinkStore,
  ConvoxSdkConnectionStore,
  ConvoxCatalogEntityStore,
} from '@aelio/core/edge';
import type { LLMProvider } from '@aelio/llm';
import type { WhatsAppSender } from '@aelio/channels';
import type { AelioDbClient } from '@aelio/db-client';
import type { AelioConfig } from './config.js';
import type { ServerSdkBridge } from './sdk-bridge.js';
import type { AelioRuntimeClient } from './aelio-runtime-client.js';

/** All persistent stores are backed by the Rust Aelio database; there is no SQLite fallback. */
export type RuntimeDeps = {
  config: AelioConfig;
  llm: LLMProvider;
  sdkBridge: ServerSdkBridge;
  /** Authoritative Rust execution service; TypeScript has no turn-execution fallback. */
  aelioRuntime: AelioRuntimeClient;
  whatsappSender: WhatsAppSender | null;
  aelioDbClient: AelioDbClient;
  messageStore: ConvoxMessageStore;
  sessionStore: ConvoxSessionStore;
  memoryStore: ConvoxMemoryStore;
  customerStore: ConvoxCustomerStore;
  jobStore: ConvoxJobStore;
  inboundDedupStore: ConvoxInboundDedupStore;
  magicLinkStore: ConvoxMagicLinkStore;
  sdkConnectionStore: ConvoxSdkConnectionStore;
  /** Soft-delete durable catalog (tools/states/policies/flows); reads are active-only. */
  catalogEntityStore: ConvoxCatalogEntityStore;
};
