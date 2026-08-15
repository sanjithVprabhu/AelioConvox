import type { AelioDatabase } from '@aelio/db';
import type { Channel } from '@aelio/protocol';
import type { LLMProvider } from '@aelio/llm';
import { randomUUID } from 'node:crypto';
import { extractMemories, recallMemories, summarizeMemories } from '../analyst/index.js';
import { logFunctionCall } from '../audit/function-calls.js';
import { resolveCustomerExternalId, type IdentityConfig } from '../identity/resolve.js';
import { assertWithinRateLimit, type RateLimitConfig } from './rate-limit.js';
import type { ConvoxMessageStore } from '../storage/messages.js';
import {
  buildConversationContext,
  type ConversationTurnContext,
} from '../storage/context.js';
import {
  appendMessage,
  ensureCustomer,
  findOrCreateSession,
  loadHistory,
  touchSessionActivity,
} from '../session/lifecycle.js';
import { maybeSummarizeSession } from '../session/summary.js';
import {
  lookupCachedResponse,
  storeCachedResponse,
  type ResponseCacheConfig,
} from './response-cache.js';
import {
  clearPendingConfirmation,
  getPendingConfirmation,
  setPendingConfirmation,
} from '../session/confirmations.js';
import type { SdkBridge } from '../sdk-bridge/types.js';
import {
  buildCancellationReply,
  buildWriteSuccessReply,
  isConfirmationMessage,
  isDenialMessage,
} from '../safety/confirmations.js';
import type { SafetyConfig } from '../safety/policy.js';
import {
  buildIntentStackPrompt,
  loadIntentStack,
  saveIntentStack,
  updateIntentStack,
  type IntentStack,
  type IntentStackConfig,
} from '../intent/index.js';
import {
  buildLifecycleSystemPrompt,
  filterFunctionsByState,
  getCustomerLifecycleMetadata,
  getCustomerPresentFields,
  type CustomerLifecycleMetadata,
} from '../lifecycle/index.js';
import { composeSystemPrompt } from './prompt-composer.js';
import { selectRelevantTools } from './tool-retrieval.js';
import { withSessionLock } from './session-lock.js';
import { runToolLoop } from './tool-loop.js';
import { runHarness } from '../harness/index.js';
import type { HarnessBindingConfig, HarnessBudgets } from '../harness/schema.js';
import type { HarnessTracer } from '../harness/traces.js';
import type { SuspensionStore } from '../harness/suspension.js';
import type { LighthouseService } from '../lighthouse/index.js';
import {
  evaluatePipeline,
  handlePipelinePostTurn,
  patchPipelineContext,
  resolvePipelineRoute,
  runPipelineTurn,
} from '../pipeline/index.js';
import { runWithTurnContext } from '../telemetry/turn-calls.js';

export type ProcessTurnResult = {
  reply: string;
  turnId: string;
  awaitingConfirmation?: boolean;
  uiDirective?: import('@aelio/protocol').UiDirective;
};

export type ProcessTurnInput = {
  database: AelioDatabase;
  llm: LLMProvider;
  sdk: SdkBridge;
  model: string;
  maxTokens: number;
  historyWindow: number;
  safety: SafetyConfig;
  identity: IdentityConfig;
  rateLimit: RateLimitConfig;
  idleTimeoutMinutes: number;
  summarizeAfter: number;
  memoryRecallLimit: number;
  customerExternalId: string;
  channel: Channel;
  channelAddress: string;
  message: string;
  memoryEnabled?: boolean;
  cache?: ResponseCacheConfig;
  intent?: IntentStackConfig;
  messageStore?: ConvoxMessageStore;
  /** Config-level persona override (SDK-registered persona wins when present). */
  persona?: string | null;
  /** When true the customer authenticated via magic link / session token. */
  authenticated?: boolean;
  /** Harness engine (plan-execute-replan). When absent/disabled, the legacy tool loop runs. */
  harness?: {
    enabled: boolean;
    budgets: HarnessBudgets;
    binding: HarnessBindingConfig;
  };
  lighthouse?: LighthouseService;
  tracer?: HarnessTracer;
  suspensionStore?: SuspensionStore;
};

type PersistMessageInput = {
  sessionId: string;
  customerId: string;
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: string;
  channel: Channel;
  context?: ConversationTurnContext;
};

type TurnRuntimeSnapshot = {
  intentStack: IntentStack;
  lifecycle: CustomerLifecycleMetadata;
  /** Customer-profile fields present, for state/transition `requires_fields` guards. */
  presentFields: Set<string>;
};

function buildTurnContext(
  input: ProcessTurnInput,
  snapshot: TurnRuntimeSnapshot,
  extras?: {
    toolsExecuted?: string[];
    pendingConfirmation?: boolean;
  },
): ConversationTurnContext {
  return buildConversationContext({
    customerExternalId: resolveCustomerExternalId(
      input.channel,
      input.customerExternalId,
      input.identity,
    ),
    channelAddress: input.channelAddress,
    lifecycle: snapshot.lifecycle,
    intentStack: snapshot.intentStack,
    flows: input.sdk.getFlows(),
    policies: input.sdk.getPolicies(),
    toolsExecuted: extras?.toolsExecuted,
    pendingConfirmation: extras?.pendingConfirmation,
  });
}

async function loadTurnSnapshot(
  input: ProcessTurnInput,
  db: AelioDatabase['db'],
  customerId: string,
  sessionId: string,
): Promise<TurnRuntimeSnapshot> {
  const intentStack = input.intent?.enabled ? await loadIntentStack(db, sessionId) : [];
  const lifecycle = await getCustomerLifecycleMetadata(db, customerId);
  const presentFields = await getCustomerPresentFields(db, customerId);
  return { intentStack, lifecycle, presentFields };
}

async function persistMessage(
  input: ProcessTurnInput,
  db: AelioDatabase['db'],
  message: PersistMessageInput,
): Promise<void> {
  if (input.messageStore) {
    try {
      await input.messageStore.appendMessage({
        ...message,
        channel: message.channel,
      });
    } catch (error) {
      // A Sunjet outage must degrade archival, never conversations. With the
      // fallback enabled the SQLite write below keeps the turn fully durable;
      // archive rows can be reconciled later.
      if (!input.messageStore.fallbackSqliteOnError) {
        throw error;
      }
      console.error(
        '[aelio] Sunjet append failed — continuing on SQLite:',
        error instanceof Error ? error.message : String(error),
      );
      await appendMessage(db, message);
      return;
    }
    if (input.messageStore.dualWriteSqlite) {
      await appendMessage(db, message);
      return;
    }
    await touchSessionActivity(db, message.sessionId);
    return;
  }

  await appendMessage(db, message);
}

async function loadTurnHistory(
  input: ProcessTurnInput,
  db: AelioDatabase['db'],
  sessionId: string,
): Promise<Awaited<ReturnType<typeof loadHistory>>> {
  if (!input.messageStore) {
    return loadHistory(db, sessionId, input.historyWindow);
  }

  // With dual-write on, SQLite holds identical history behind an indexed
  // ORDER BY … LIMIT and no vector payloads — the Sunjet scan returns full rows
  // (embeddings included) with no projection, which is megabytes per turn.
  // Sunjet stays the read path only when it is the sole store.
  if (input.messageStore.dualWriteSqlite) {
    return loadHistory(db, sessionId, input.historyWindow);
  }

  try {
    return await input.messageStore.loadHistory(sessionId, input.historyWindow);
  } catch (error) {
    if (!input.messageStore.fallbackSqliteOnError) {
      throw error;
    }
    return loadHistory(db, sessionId, input.historyWindow);
  }
}

const DEFAULT_PERSONA = `You are Aelio, a helpful conversational assistant for a SaaS product.
Answer clearly and concisely. Use available tools when you need account-specific data.
Never invent account details — always use tools for factual lookups.`;

const TOOL_GUIDANCE = `For write actions, do NOT ask the user to confirm yourself — once you have the required
arguments, call the tool directly. The platform automatically asks the user to confirm
before any write executes, so a second confirmation question from you is redundant.`;

export async function processTurn(input: ProcessTurnInput): Promise<ProcessTurnResult> {
  const externalId = resolveCustomerExternalId(
    input.channel,
    input.customerExternalId,
    input.identity,
  );
  // Serialize turns for this customer+channel: a second message that arrives
  // mid-turn queues behind the first, so intent stack / suspension / lifecycle
  // never mutate concurrently.
  return withSessionLock(`${input.channel}:${externalId}`, () => processTurnLocked(input, externalId));
}

async function processTurnLocked(
  input: ProcessTurnInput,
  externalId: string,
): Promise<ProcessTurnResult> {
  const db = input.database.db;

  const customerId = await ensureCustomer(
    db,
    externalId,
    input.channel,
    input.channelAddress,
  );

  await assertWithinRateLimit(db, customerId, input.rateLimit);

  const session = await findOrCreateSession(
    db,
    customerId,
    input.channel,
    input.idleTimeoutMinutes,
  );
  const turnId = randomUUID();
  const preTurnSnapshot = await loadTurnSnapshot(input, db, customerId, session.id);
  await persistMessage(input, db, {
    sessionId: session.id,
    customerId,
    role: 'user',
    content: input.message,
    channel: input.channel,
    context: buildTurnContext(input, preTurnSnapshot),
  });

  return runWithTurnContext(
    {
      turnId,
      sessionId: session.id,
      customerId,
      database: input.database,
    },
    async () => executeTurn(input, {
      db,
      externalId,
      customerId,
      session,
      turnId,
      preTurnSnapshot,
    }),
  );
}

async function executeTurn(
  input: ProcessTurnInput,
  state: {
    db: AelioDatabase['db'];
    externalId: string;
    customerId: string;
    session: { id: string; customerId: string; channel: Channel };
    turnId: string;
    preTurnSnapshot: TurnRuntimeSnapshot;
  },
): Promise<ProcessTurnResult> {
  const { db, externalId, customerId, session, turnId, preTurnSnapshot } = state;

  const context = {
    customerId: externalId,
    sessionId: session.id,
    channel: input.channel,
    channelAddress: input.channelAddress,
  };

  const pending = await getPendingConfirmation(db, session.id);
  if (pending) {
    if (isConfirmationMessage(input.message)) {
      const fn = input.sdk.getFunctions().find((entry) => entry.name === pending.functionName);
      const invokeResult = await input.sdk.invoke(
        pending.functionName,
        pending.args,
        context,
      );

      await logFunctionCall(db, {
        sessionId: session.id,
        customerId,
        functionName: pending.functionName,
        args: pending.args,
        result: invokeResult.data,
        status: invokeResult.ok ? 'success' : 'error',
        safetyLevel: pending.safetyLevel,
        requiredConfirmation: true,
        confirmed: true,
        durationMs: invokeResult.durationMs,
        errorMessage: invokeResult.error,
      });

      await clearPendingConfirmation(db, session.id);

      const reply = invokeResult.ok
        ? fn
          ? buildWriteSuccessReply(fn, invokeResult.data)
          : 'The action completed successfully.'
        : `I couldn't complete that action: ${invokeResult.error ?? 'unknown error'}`;

      await persistMessage(input, db, {
        sessionId: session.id,
        customerId,
        role: 'assistant',
        content: reply,
        channel: input.channel,
        context: buildTurnContext(input, preTurnSnapshot, {
          toolsExecuted: [pending.functionName],
        }),
      });

      return { reply, turnId };
    }

    if (isDenialMessage(input.message)) {
      await clearPendingConfirmation(db, session.id);
      const reply = buildCancellationReply();
      await persistMessage(input, db, {
        sessionId: session.id,
        customerId,
        role: 'assistant',
        content: reply,
        channel: input.channel,
        context: buildTurnContext(input, preTurnSnapshot),
      });
      return { reply, turnId };
    }
  }

  // Semantic response cache: serve a near-identical, recent, no-tool reply for
  // this customer without calling the LLM. Only no-tool replies are cached, so a
  // hit never returns stale account data. Skipped when a plan is parked awaiting
  // this customer's input — that message is an answer to resume the plan, not a
  // fresh question, and must reach the harness even if it resembles a cached one.
  // (The parked-plan probe only runs when the cache is on, so the default path
  // pays no extra read.)
  // Skip response cache when a pipeline manifest is registered — pipeline steps
  // are stage-sensitive and a stale cached bind-failure must not replay.
  const pipelineManifest = input.sdk.getPipelineManifest?.() ?? null;
  if (input.cache?.enabled && !pipelineManifest) {
    const hasParkedPlan =
      input.harness?.enabled && input.suspensionStore
        ? (await input.suspensionStore.get(session.id)) !== null
        : false;
    const cached = hasParkedPlan
      ? null
      : await lookupCachedResponse({
          database: input.database,
          customerId,
          message: input.message,
          threshold: input.cache.similarityThreshold,
        });
    if (cached) {
      await persistMessage(input, db, {
        sessionId: session.id,
        customerId,
        role: 'assistant',
        content: cached,
        channel: input.channel,
        context: buildTurnContext(input, preTurnSnapshot),
      });
      return { reply: cached, turnId };
    }
  }

  const history = await loadTurnHistory(input, db, session.id);
  const summary = await maybeSummarizeSession({
    db,
    sessionId: session.id,
    summarizeAfter: input.summarizeAfter,
    llm: input.llm,
    model: input.model,
    maxTokens: input.maxTokens,
  });

  const flows = input.sdk.getFlows();
  const pipelineContext = patchPipelineContext(
    await evaluatePipeline({
      database: input.database,
      internalCustomerId: customerId,
      externalId,
      userMessage: input.message,
      manifest: input.sdk.getPipelineManifest?.() ?? null,
      flows,
      attributes: input.sdk.getAttributes?.() ?? [],
      authenticated: input.authenticated ?? false,
    }),
    flows,
  );

  const intentStack = preTurnSnapshot.intentStack;
  const intentPrompt = input.intent?.enabled ? buildIntentStackPrompt(intentStack) : '';

  const lifecycle = preTurnSnapshot.lifecycle;
  const effectiveLifecycleState = pipelineContext.enabled
    ? pipelineContext.globalStage
    : lifecycle.lifecycleState;
  const stateDef = effectiveLifecycleState
    ? input.sdk.getStates().find((entry) => entry.id === effectiveLifecycleState) ??
      (pipelineContext.stage
        ? {
            id: pipelineContext.globalStage,
            description: pipelineContext.stage.description,
            allowedTools: pipelineContext.stage.allowedTools,
            blockedTools: pipelineContext.stage.blockedTools,
            allowedIntents: pipelineContext.stage.allowedIntents,
            blockedIntents: pipelineContext.stage.blockedIntents,
            allowedSafety: pipelineContext.stage.allowedSafety,
            blockedSafety: pipelineContext.stage.blockedSafety,
            guards: pipelineContext.stage.guards,
          }
        : undefined)
    : undefined;
  const lifecyclePrompt = buildLifecycleSystemPrompt({
    stateId: effectiveLifecycleState,
    state: stateDef,
    policies: input.sdk.getPolicies(),
    flows: input.sdk.getFlows(),
    flowProgress: lifecycle.flowProgress,
    omitActiveFlows: pipelineContext.enabled && Boolean(pipelineContext.currentStep),
  });
  // Lifecycle scoping first (allowed/blocked per state), then relevance
  // retrieval: above the registry-size threshold only the top-K tools for THIS
  // message ship to the model. The active flow step's tool always survives.
  const stateScopedFunctions = filterFunctionsByState(input.sdk.getFunctions(), stateDef);
  const activeFlowTools: string[] = [...pipelineContext.forceIncludeTools];
  for (const flow of input.sdk.getFlows()) {
    const progress = lifecycle.flowProgress?.[flow.id];
    if (!progress) continue;
    const step = flow.steps[Math.min(progress.currentStepIndex, flow.steps.length - 1)];
    if (step?.tool) {
      activeFlowTools.push(step.tool);
    }
  }
  const scopedFunctions = await selectRelevantTools(stateScopedFunctions, input.message, {
    forceInclude: activeFlowTools,
  });

  let memoriesPrompt = '';
  if (input.memoryEnabled !== false) {
    const recalled = await recallMemories(
      input.database,
      customerId,
      input.message,
      input.memoryRecallLimit,
    );
    if (recalled.length > 0) {
      memoriesPrompt = summarizeMemories(recalled);
    }
  }

  // Stable sections form a cache-friendly prefix; volatile context renders last
  // and is trimmed first under budget pressure (see prompt-composer.ts).
  const persona = input.sdk.getPersona?.() ?? input.persona ?? DEFAULT_PERSONA;
  const harnessEnabled = input.harness?.enabled ?? false;
  // The harness planner sees tools as prompt cards (name — intent — description),
  // not as native tool definitions: Pass 1 plans at capability level and never
  // needs full schemas. The capability brief grounds what the product can do.
  const brief = harnessEnabled ? (input.lighthouse?.getBrief() ?? '') : '';
  const toolCards = harnessEnabled
    ? scopedFunctions
        .map((fn) => `- ${fn.name} (${fn.intent ?? fn.name})${fn.safety !== 'read' ? ` [${fn.safety}]` : ''}: ${fn.description}`)
        .join('\n')
    : '';
  const attributes = input.sdk.getAttributes?.() ?? [];
  const system = composeSystemPrompt([
    { id: 'persona', content: persona, stability: 'stable', priority: 100, maxTokens: 800 },
    { id: 'guidance', content: TOOL_GUIDANCE, stability: 'stable', priority: 95 },
    { id: 'brief', content: brief, stability: 'stable', priority: 92, maxTokens: 1500 },
    {
      id: 'pipeline',
      content: pipelineContext.pipelinePrompt,
      stability: 'volatile',
      priority: 91,
      maxTokens: 1200,
    },
    {
      id: 'profile_memory',
      content: pipelineContext.memoryPrompt,
      stability: 'volatile',
      priority: 88,
      maxTokens: 800,
    },
    { id: 'lifecycle', content: lifecyclePrompt ?? '', stability: 'stable', priority: 90, maxTokens: 1200 },
    { id: 'tools', content: toolCards ? `Available tools for this turn:\n${toolCards}` : '', stability: 'volatile', priority: 70, maxTokens: 1500 },
    { id: 'summary', content: summary ? `Rolling session summary:\n${summary}` : '', stability: 'volatile', priority: 60, maxTokens: 600 },
    { id: 'memories', content: memoriesPrompt, stability: 'volatile', priority: 50, maxTokens: 600 },
    { id: 'intent', content: intentPrompt, stability: 'volatile', priority: 40, maxTokens: 400 },
  ]);

  const engineInput = {
    database: input.database,
    internalCustomerId: customerId,
    llm: input.llm,
    sdk: input.sdk,
    functions: scopedFunctions,
    model: input.model,
    maxTokens: input.maxTokens,
    system,
    history,
    userMessage: input.message,
    context,
    safety: input.safety,
  };
  const evaluateBase = {
    database: input.database,
    internalCustomerId: customerId,
    externalId,
    userMessage: input.message,
    manifest: input.sdk.getPipelineManifest?.() ?? null,
    flows,
    attributes,
    authenticated: input.authenticated ?? false,
  };

  const pipelineRoute = resolvePipelineRoute(pipelineContext, input.message);
  const loopResult =
    pipelineRoute != null
      ? await runPipelineTurn({
          database: input.database,
          externalId,
          internalCustomerId: customerId,
          flows,
          attributes,
          evaluateBase,
          pipeline: pipelineContext,
          llm: input.llm,
          sdk: input.sdk,
          model: input.model,
          maxTokens: input.maxTokens,
          system,
          history,
          userMessage: input.message,
          context,
          safety: input.safety,
        })
      : input.harness?.enabled
      ? await runHarness({
          ...engineInput,
          ...(stateDef ? { state: stateDef } : {}),
          presentFields: preTurnSnapshot.presentFields,
          lighthouse: input.lighthouse,
          tracer: input.tracer,
          suspensionStore: input.suspensionStore,
          budgets: input.harness.budgets,
          binding: input.harness.binding,
          turnId,
        })
      : await runToolLoop(engineInput);

  if (loopResult.pendingConfirmation) {
    await setPendingConfirmation(db, session.id, loopResult.pendingConfirmation);
  }

  if (pipelineRoute == null) {
    await handlePipelinePostTurn({
      database: input.database,
      externalId,
      internalCustomerId: customerId,
      pipeline: pipelineContext,
      executedToolNames: loopResult.executedToolNames,
      userMessage: input.message,
    });
  }

  let postTurnSnapshot = preTurnSnapshot;
  if (input.intent?.enabled) {
    const nextStack = updateIntentStack({
      stack: intentStack,
      userMessage: input.message,
      assistantReply: loopResult.reply,
      toolNames: loopResult.executedToolNames,
      registeredTools: input.sdk.getFunctions(),
      config: input.intent,
    });
    await saveIntentStack(db, session.id, nextStack);
    postTurnSnapshot = { ...preTurnSnapshot, intentStack: nextStack };
  }

  await persistMessage(input, db, {
    sessionId: session.id,
    customerId,
    role: 'assistant',
    content: loopResult.reply,
    channel: input.channel,
    context: buildTurnContext(input, postTurnSnapshot, {
      toolsExecuted: loopResult.executedToolNames,
      pendingConfirmation: Boolean(loopResult.pendingConfirmation),
    }),
  });

  if (input.memoryEnabled !== false) {
    void extractMemories({
      database: input.database,
      customerId,
      sessionId: session.id,
      userMessage: input.message,
      assistantReply: loopResult.reply,
      turnId,
    });
  }

  // Cache only no-tool, non-pending, real replies — never tool-backed answers.
  if (
    input.cache?.enabled &&
    loopResult.toolCallsExecuted === 0 &&
    !loopResult.pendingConfirmation &&
    loopResult.reply.trim().length > 0 &&
    !/^I don't have a way to "/.test(loopResult.reply)
  ) {
    await storeCachedResponse({
      database: input.database,
      customerId,
      message: input.message,
      reply: loopResult.reply,
      ttlMinutes: input.cache.ttlMinutes,
    });
  }

  return {
    reply: loopResult.reply,
    turnId,
    ...(loopResult.pendingConfirmation ? { awaitingConfirmation: true } : {}),
    ...(pipelineContext.uiDirective ? { uiDirective: pipelineContext.uiDirective } : {}),
  };
}
