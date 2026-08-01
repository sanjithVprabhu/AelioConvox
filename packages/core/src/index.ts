export {
  processTurn,
  type ProcessTurnInput,
  type ProcessTurnResult,
} from './runtime/turn.js';
export { withSessionLock } from './runtime/session-lock.js';
export {
  buildLlmPromptSummary,
  createInstrumentedLlm,
  getTurnContext,
  listTurnApiCalls,
  recordTurnApiCall,
  runWithTurnContext,
  summarizeTurnApiCalls,
  type TurnApiCallPurpose,
  type TurnApiCallRecord,
  type TurnApiCallsAelioDbConfig,
  type TurnContext,
} from './telemetry/index.js';
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
export {
  composeSystemPrompt,
  normalizePromptText,
  approxTokens,
  type PromptSection,
  type ComposeOptions,
} from './runtime/prompt-composer.js';
export {
  buildTurnSystemPrompt,
  type SystemPromptIngredients,
} from './runtime/prompt-factory.js';
export {
  ImmediateContextEngine,
  createImmediateContextEngine,
  tierForAge,
  CONTEXT_TIERS,
  HOT_WINDOW_MS,
  type ContextTierDef,
  type ImmediateContextEngineOptions,
  type ImmediateContextSnapshot,
} from './context-engine/index.js';
export {
  ArchetypeEngine,
  createArchetypeEngine,
  seedBuiltinArchetypes,
  splitSpans,
  DEFAULT_ARCHETYPES,
  type ArchetypeEngineOptions,
  type ArchetypeAssessInput,
  type ArchetypeAssessment,
  type CategoryAssessment,
} from './archetype/index.js';
export {
  evaluateGenericGate,
  type GenericGateOptions,
  type GenericGateResult,
  type GenericKind,
} from './gate/index.js';
export {
  resolveTemporalScope,
  renderTemporalScope,
  inTemporalScope,
  temporalRelevance,
  type TemporalScope,
  type TemporalTier,
  type TemporalResolveInput,
} from './temporal/index.js';
export {
  DEFAULT_EVIDENCE_CONFIG,
  scoreEvidenceParts,
  evidenceFromAxisOccurrence,
  evidenceFromMemory,
  evidenceFromStance,
  selectEvidence,
  renderEvidenceBlock,
  type EvidenceItem,
  type EvidenceConfig,
  type EvidenceSource,
  type EvidenceWeights,
} from './evidence/index.js';
export {
  deriveResolutionState,
  scoreProactiveActivation,
  DEFAULT_ACTIVATION_CONFIG,
  type ResolutionState,
  type ResolutionSnapshot,
  type ResolutionSignals,
  type ActivationDecision,
  type ProactiveActivationConfig,
} from './resolution/index.js';
export {
  SemanticPathwayEngine,
  createSemanticPathwayEngine,
  type SemanticPathwayEngineOptions,
  type SemanticPathwayInput,
  type SemanticPathwayDecision,
  type SemanticIntent,
  type PathwayStrategy,
  type PathwayProactiveHint,
  type PathwayTimings,
} from './pathway/index.js';
export {
  rankRelevantTools,
  selectRelevantTools,
  TOOL_RETRIEVAL_THRESHOLD,
  TOOL_RETRIEVAL_TOP_K,
  type ToolRetrievalOptions,
  type RankedTool,
} from './runtime/tool-retrieval.js';
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
  touchSessionActivity,
} from './session/lifecycle.js';
export {
  appendConversationRecord,
  bootstrapAelioDbTables,
  buildConversationContext,
  ConvoxMessageStore,
  createConvoxMessageStore,
  ConvoxMemoryStore,
  createConvoxMemoryStore,
  ConvoxSessionStore,
  createConvoxSessionStore,
  ConvoxCustomerStore,
  createConvoxCustomerStore,
  ConvoxJobStore,
  createConvoxJobStore,
  ConvoxResponseCacheStore,
  createConvoxResponseCacheStore,
  ConvoxFunctionCallStore,
  createConvoxFunctionCallStore,
  ConvoxReflectionStore,
  createConvoxReflectionStore,
  ConvoxProactiveStore,
  createConvoxProactiveStore,
  ConvoxInboundDedupStore,
  createConvoxInboundDedupStore,
  ConvoxMagicLinkStore,
  createConvoxMagicLinkStore,
  ConvoxSdkConnectionStore,
  createConvoxSdkConnectionStore,
  ConvoxArchetypeStore,
  createConvoxArchetypeStore,
  type ArchetypeExemplar,
  type ArchetypeMatch,
  type ArchetypeValence,
  ConvoxAspectStore,
  createConvoxAspectStore,
  type AspectRecord,
  type AspectMatch,
  type AspectStatus,
  type AspectSource,
  type AspectRegisterInput,
  ConvoxAxisStore,
  createConvoxAxisStore,
  type AxisOccurrence,
  type AxisOccurrenceWrite,
  listConversationTelemetry,
  type ConversationTelemetryEvent,
  type ConversationTurnContext,
  type MessageStoreAppendInput,
  type MessageStoreHistoryRow,
  type MemoryStoreWriteInput,
  type MemoryRecallHit,
  type AelioDbStorageConfig,
  type AelioDbTableNames,
  type ClosedSessionRecord,
  type SessionRecord,
  type CustomerRecord,
  type ClaimedJob,
  type FunctionCallLogInput,
  type FunctionCallRecord,
  type ReflectionRecord,
  type ReflectionStoreInput,
  type ProactiveRecord,
  type ProactiveRecordInput,
  type MagicLinkCreateInput,
  type MagicLinkRecord,
  type SdkConnectionUpsertInput,
} from './storage/index.js';
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
export {
  buildIntentStackPrompt,
  expireIntentStack,
  loadIntentStack,
  saveIntentStack,
  updateIntentStack,
  type IntentFrame,
  type IntentStack,
  type IntentStackConfig,
} from './intent/index.js';
export {
  buildLifecycleSystemPrompt,
  filterFunctionsByState,
  getCustomerLifecycleMetadata,
  getCustomerPresentFields,
  readLifecycleMetadata,
  upsertCustomerFlowProgress,
  upsertCustomerLifecycleState,
  type CustomerLifecycleMetadata,
  type FlowProgressRecord,
} from './lifecycle/index.js';
export { enqueueJob, claimJob, completeJob, failJob, requeueStaleJobs, type JobRecord } from './job-queue/index.js';
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
  discoverAspects,
  type ExtractInput,
  type RecalledMemory,
  type Reflection,
  type ReflectionOutcome,
  type AspectDiscoveryInput,
  type AspectDiscoveryResult,
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
export {
  createSessionToken,
  verifySessionToken,
  type SessionTokenClaims,
} from './identity/session-token.js';
export { logFunctionCall } from './audit/function-calls.js';
export {
  LighthouseService,
  LighthouseMirror,
  registryHash,
  buildCapabilityBrief,
  type LighthouseConfig,
  type RegistrySnapshot,
  type ToolSearchHit,
} from './lighthouse/index.js';
export {
  EmitTurnSchema,
  EMIT_TURN_INPUT_SCHEMA,
  PlanInstructionSchema,
  SuspendedPlanPayloadSchema,
  DEFAULT_BUDGETS,
  DEFAULT_BINDING,
  type EmitTurn,
  type PlanInstruction,
  type ResolvedInstruction,
  type ResolvedPlan,
  type ArgSource,
  type LedgerEntry,
  type GateVerdict,
  type SuspendedPlanPayload,
  type SuspensionReason,
  type HarnessBudgets,
  type HarnessBindingConfig,
} from './harness/schema.js';
export { SuspensionStore, type SuspendedPlanRecord, type SuspensionStoreConfig } from './harness/suspension.js';
export { type LedgerAelioDbConfig } from './harness/ledger.js';
export { type BindingCacheConfig } from './harness/binder.js';
export { HarnessTracer, type TraceKind, type HarnessTracerConfig } from './harness/traces.js';
export {
  runHarness,
  runPlanner,
  runSynthesis,
  BudgetMeter,
  bindInstructions,
  resolvePlan,
  nextWave,
  executePlan,
  newExecutorState,
  evaluateGate,
  rehydrateSuspension,
  toSuspensionPayload,
  applyStateTransition,
  hashArgs,
  type HarnessRunInput,
} from './harness/index.js';
export type { BoundInstruction } from './harness/resolver.js';
export type { ExecOutcome, ExecutorState } from './harness/executor.js';
