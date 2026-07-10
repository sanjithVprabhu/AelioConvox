import type { ProcessTurnInput } from '@aelio/core';
import type { Channel } from '@aelio/protocol';
import type { RuntimeDeps } from './runtime-deps.js';

export function baseTurnOptions(
  deps: RuntimeDeps,
): Pick<
  ProcessTurnInput,
  | 'database'
  | 'llm'
  | 'sdk'
  | 'model'
  | 'backgroundModel'
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
  | 'persona'
  | 'harness'
> {
  return {
    database: deps.database,
    llm: deps.llm,
    sdk: deps.sdkBridge,
    model: deps.config.llm.model,
    backgroundModel: deps.config.llm.background_model ?? deps.config.llm.model,
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
    messageStore: deps.messageStore ?? undefined,
    persona: deps.config.llm.system_prompt ?? null,
    harness: {
      enabled: deps.config.harness.enabled,
      routerModel: deps.config.harness.router_model,
      plannerModel: deps.config.harness.planner_model,
      synthesisModel: deps.config.harness.synthesis_model,
      planStepCap: deps.config.harness.plan_step_cap,
      replanCap: deps.config.harness.replan_cap,
      tokenBudget: deps.config.harness.token_budget,
      wallClockMs: deps.config.harness.wall_clock_ms,
      flowConfidenceThreshold: deps.config.harness.flow_confidence_threshold,
      toolRetrievalK: deps.config.harness.tool_retrieval_k,
      forceCategories: deps.config.harness.force_categories,
      forceOnActiveFlow: deps.config.harness.force_on_active_flow,
    },
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
