import type { ProcessTurnInput } from '@aelio/core';
import type { Channel } from '@aelio/protocol';
import type { RuntimeDeps } from './runtime-deps.js';

export function baseTurnOptions(
  deps: RuntimeDeps,
): Pick<
  ProcessTurnInput,
  | 'llm'
  | 'sdk'
  | 'model'
  | 'maxTokens'
  | 'historyWindow'
  | 'safety'
  | 'identity'
  | 'memoryEnabled'
  | 'rateLimit'
  | 'idleTimeoutMinutes'
  | 'summarizeAfter'
  | 'memoryRecallLimit'
  | 'cache'
  | 'intent'
  | 'messageStore'
  | 'memoryStore'
  | 'sessionStore'
  | 'customerStore'
  | 'responseCacheStore'
  | 'functionCallStore'
  | 'persona'
  | 'harness'
  | 'lighthouse'
  | 'tracer'
  | 'suspensionStore'
  | 'ledgerSunjet'
  | 'bindingCache'
  | 'turnApiCallsSunjet'
  | 'contextEngine'
  | 'pathwayEngine'
  | 'archetypeEngine'
  | 'axisStore'
  | 'tracePrompt'
> {
  return {
    llm: deps.llm,
    sdk: deps.sdkBridge,
    model: deps.config.llm.model,
    maxTokens: deps.config.llm.max_tokens,
    historyWindow: deps.config.session.history_window,
    safety: {
      defaultMode: deps.config.safety.default_mode,
      requireConfirmationFor: deps.config.safety.require_confirmation_for,
      overrides: deps.config.safety.overrides,
    },
    identity: {
      mappingFunction: deps.config.identity.mapping_function,
      allowAnonymous: deps.config.identity.allow_anonymous,
    },
    memoryEnabled: deps.config.memory.enabled,
    rateLimit: {
      perCustomerPerMinute: deps.config.safety.rate_limit.per_customer_per_minute,
      perCustomerPerDay: deps.config.safety.rate_limit.per_customer_per_day,
    },
    idleTimeoutMinutes: deps.config.session.idle_timeout_minutes,
    summarizeAfter: deps.config.session.summarize_after,
    memoryRecallLimit: deps.config.memory.recall_limit,
    cache: {
      enabled: deps.config.cache.enabled,
      similarityThreshold: deps.config.cache.similarity_threshold,
      ttlMinutes: deps.config.cache.ttl_minutes,
    },
    intent: {
      enabled: deps.config.intent.enabled,
      ttlMinutes: deps.config.intent.ttl_minutes,
      maxDepth: deps.config.intent.max_depth,
    },
    messageStore: deps.messageStore,
    memoryStore: deps.memoryStore,
    sessionStore: deps.sessionStore,
    customerStore: deps.customerStore,
    responseCacheStore: deps.responseCacheStore,
    functionCallStore: deps.functionCallStore,
    persona: deps.config.llm.system_prompt ?? null,
    harness: {
      enabled: deps.config.harness.enabled,
      budgets: {
        maxInstructions: deps.config.harness.budgets.max_instructions,
        maxReplans: deps.config.harness.budgets.max_replans,
        maxRecoilsPerIntent: deps.config.harness.budgets.max_recoils_per_intent,
        maxToolCalls: deps.config.harness.budgets.max_tool_calls,
        wallClockMs: deps.config.harness.budgets.wall_clock_ms,
        maxTurnTokens: deps.config.harness.budgets.max_turn_tokens,
      },
      binding: {
        scoreMin: deps.config.harness.binding.score_min,
        ambiguityGap: deps.config.harness.binding.ambiguity_gap,
        cacheTtlMinutes: deps.config.harness.binding.cache_ttl_minutes,
      },
    },
    lighthouse: deps.lighthouse,
    tracer: deps.tracer ?? undefined,
    suspensionStore: deps.suspensionStore,
    ledgerSunjet: { client: deps.sunjetClient, table: deps.config.sunjet.tables.harness_ledger },
    bindingCache: {
      client: deps.sunjetClient,
      table: deps.config.sunjet.tables.harness_bindings,
      tenant: deps.config.name,
      registryHash: deps.lighthouse.getHash(),
      embedDim: deps.config.sunjet.embed_dim,
      ttlMinutes: deps.config.harness.binding.cache_ttl_minutes,
    },
    turnApiCallsSunjet: { client: deps.sunjetClient, table: deps.config.sunjet.tables.turn_api_calls },
    contextEngine: deps.contextEngine,
    pathwayEngine: deps.pathwayEngine,
    archetypeEngine: deps.archetypeEngine,
    axisStore: deps.axisStore,
    tracePrompt: deps.config.logging.trace_prompt,
  };
}

export function buildTurnInput(
  deps: RuntimeDeps,
  input: {
    customerExternalId: string;
    channel: Channel;
    channelAddress: string;
    message: string;
  },
): ProcessTurnInput {
  return {
    ...baseTurnOptions(deps),
    ...input,
  };
}
