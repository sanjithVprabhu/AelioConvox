/**
 * TypeScript edge-only surface.
 *
 * The public server may import storage adapters, authentication, telemetry, provider modems, and
 * transport bookkeeping from here. Adaptive turn selection, flow execution, promotion,
 * reflection/extraction, and effect execution are deliberately absent; those are Rust authorities
 * in the unified architecture.
 */
export { withSessionLock } from './runtime/session-lock.js';
export {
  createInstrumentedLlm,
  listTurnApiCalls,
  summarizeTurnApiCalls,
} from './telemetry/index.js';
export {
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
  listConversationTelemetry,
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
} from './storage/index.js';
export type { SdkBridge, SdkInvokeResult } from './sdk-bridge/types.js';
export { upsertCustomerLifecycleState } from './lifecycle/index.js';
export { enqueueJob, claimJob, completeJob, failJob, requeueStaleJobs } from './job-queue/index.js';
export { configureEmbedder, embed } from './analyst/embeddings.js';
export { resolveWhatsAppIdentity } from './identity/resolve.js';
export { createMagicLink, verifyMagicLink } from './identity/magic-link.js';
export { createSessionToken, verifySessionToken } from './identity/session-token.js';
export { setProactiveOptIn } from './runtime/proactive.js';
