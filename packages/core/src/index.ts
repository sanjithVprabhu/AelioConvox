export { processTurn, type ProcessTurnInput } from './runtime/turn.js';
export { runToolLoop, type ToolLoopResult } from './runtime/tool-loop.js';
export {
  buildInputSchema,
  requiredParamNames,
  findMissingRequiredArgs,
  coerceArgs,
  type InputJsonSchema,
  type ParamJsonSchema,
  type JsonSchemaType,
  type ArgCoercionResult,
} from './runtime/tool-schema.js';
export { assertWithinRateLimit, type RateLimitConfig } from './runtime/rate-limit.js';
export {
  sendProactiveMessage,
  setProactiveOptIn,
  type ProactiveConfig,
  type ProactiveInput,
  type ProactiveResult,
} from './runtime/proactive.js';
export {
  lookupCachedResponse,
  storeCachedResponse,
  type ResponseCacheConfig,
} from './runtime/response-cache.js';
export {
  appendMessage,
  ensureCustomer,
  findOrCreateSession,
  loadHistory,
} from './session/lifecycle.js';
export { loadSessionSummary, maybeSummarizeSession } from './session/summary.js';
export {
  getPendingConfirmation,
  setPendingConfirmation,
  clearPendingConfirmation,
} from './session/confirmations.js';
export {
  evaluateSafety,
  type SafetyConfig,
  type SafetyOverride,
} from './safety/policy.js';
export {
  buildConfirmationPrompt,
  buildCancellationReply,
  buildWriteSuccessReply,
  isConfirmationMessage,
  isDenialMessage,
  type PendingConfirmation,
} from './safety/confirmations.js';
export type { SdkBridge, SdkInvokeResult } from './sdk-bridge/types.js';
export { enqueueJob, claimJob, completeJob, failJob, type JobRecord } from './job-queue/index.js';
export {
  embedText,
  embed,
  configureEmbedder,
  hasConfiguredEmbedder,
  cosineSimilarity,
  extractMemories,
  recallMemories,
  buildCustomerProfile,
  summarizeMemories,
  findUnreflectedSessions,
  reflectOnSession,
  type ExtractInput,
  type RecalledMemory,
  type Reflection,
  type ReflectionOutcome,
} from './analyst/index.js';
export {
  normalizePhone,
  normalizeEmail,
  resolveCustomerExternalId,
  resolveWhatsAppIdentity,
  type IdentityConfig,
} from './identity/resolve.js';
export {
  createMagicLink,
  verifyMagicLink,
  type MagicLinkResult,
  type VerifiedMagicLink,
} from './identity/magic-link.js';
export { logFunctionCall } from './audit/function-calls.js';
