import type { Channel } from '@aelio/protocol';
import type { LLMProvider } from '@aelio/llm';
import { randomUUID } from 'node:crypto';
import { extractMemories, recallMemories, summarizeMemories } from '../analyst/index.js';
import { logFunctionCall } from '../audit/function-calls.js';
import type { ConvoxFunctionCallStore } from '../storage/audit.js';
import { resolveCustomerExternalId, type IdentityConfig } from '../identity/resolve.js';
import { assertWithinRateLimit, type RateLimitConfig } from './rate-limit.js';
import type { ConvoxMessageStore } from '../storage/messages.js';
import type { ConvoxMemoryStore } from '../storage/memories.js';
import type { ConvoxSessionStore } from '../storage/sessions.js';
import type { ConvoxCustomerStore } from '../storage/customers.js';
import type { ConvoxResponseCacheStore } from '../storage/cache.js';
import {
  buildConversationContext,
  type ConversationTurnContext,
} from '../storage/context.js';
import {
  ensureCustomer,
  findOrCreateSession,
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
import { buildTurnSystemPrompt } from './prompt-factory.js';
import type { ImmediateContextEngine } from '../context-engine/index.js';
import { selectRelevantTools } from './tool-retrieval.js';
import type { SemanticPathwayEngine } from '../pathway/index.js';
import type { ArchetypeEngine } from '../archetype/index.js';
import { embed } from '../analyst/embeddings.js';
import {
  resolveTemporalScope,
  renderTemporalScope,
  type TemporalScope,
} from '../temporal/index.js';
import {
  DEFAULT_EVIDENCE_CONFIG,
  evidenceFromAxisOccurrence,
  evidenceFromMemory,
  evidenceFromStance,
  renderEvidenceBlock,
  selectEvidence,
  type EvidenceItem,
} from '../evidence/index.js';
import { deriveResolutionState } from '../resolution/index.js';
import type { ConvoxAxisStore } from '../storage/axis.js';
import { withSessionLock } from './session-lock.js';
import { evaluateGenericGate, type GenericGateOptions } from '../gate/index.js';
import { runToolLoop } from './tool-loop.js';
import { runHarness } from '../harness/index.js';
import type { HarnessBindingConfig, HarnessBudgets } from '../harness/schema.js';
import type { HarnessTracer } from '../harness/traces.js';
import type { LedgerAelioDbConfig } from '../harness/ledger.js';
import type { BindingCacheConfig } from '../harness/binder.js';
import type { SuspensionStore } from '../harness/suspension.js';
import type { LighthouseService } from '../lighthouse/index.js';
import { runWithTurnContext, type TurnApiCallsAelioDbConfig } from '../telemetry/turn-calls.js';

export type ProcessTurnResult = {
  reply: string;
  turnId: string;
  awaitingConfirmation?: boolean;
};

export type ProcessTurnInput = {
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
  /** AelioDb message store — required. */
  messageStore: ConvoxMessageStore;
  /** Long-term semantic memory (Aelio DB VSS). Required when `memoryEnabled` is true. */
  memoryStore?: ConvoxMemoryStore;
  /** AelioDb session store — required. */
  sessionStore: ConvoxSessionStore;
  /** AelioDb customer store — required. */
  customerStore: ConvoxCustomerStore;
  /** AelioDb response cache store — required when `cache.enabled` is true. */
  responseCacheStore?: ConvoxResponseCacheStore;
  /** AelioDb function-call audit store — required for tool-call logging. */
  functionCallStore: ConvoxFunctionCallStore;
  /** Config-level persona override (SDK-registered persona wins when present). */
  persona?: string | null;
  /** Harness engine (plan-execute-replan). When absent/disabled, the legacy tool loop runs. */
  harness?: {
    enabled: boolean;
    budgets: HarnessBudgets;
    binding: HarnessBindingConfig;
  };
  lighthouse?: LighthouseService;
  tracer?: HarnessTracer;
  suspensionStore?: SuspensionStore;
  /** The harness idempotency ledger reads/writes AelioDb exclusively. Required when the harness is enabled. */
  ledgerAelioDb?: LedgerAelioDbConfig;
  /** When present, instruction→tool bindings are cached in AelioDb. */
  bindingCache?: BindingCacheConfig;
  /** When present, per-turn API call telemetry writes to AelioDb exclusively. */
  turnApiCallsAelioDb?: TurnApiCallsAelioDbConfig;
  /** Immediate Context Engine — time-bucketed short-term context fed into the system prompt. */
  contextEngine?: ImmediateContextEngine;
  /** One-embedding semantic router for intent, memory, flow, policy and tool selection. */
  pathwayEngine?: SemanticPathwayEngine;
  /** Archetype valence engine — infers conversational stance for prompt tone shaping. */
  archetypeEngine?: ArchetypeEngine;
  /** Harness Axis store — user-specific occurrence chains per aspect. */
  axisStore?: ConvoxAxisStore;
  /** Generic hot-path gate (default on). Bare greetings/thanks/farewells get template replies pre-embedding. */
  genericGate?: GenericGateOptions & { enabled?: boolean };
  /** 'redacted' stores prompt length + section flags in traces instead of the full prompt (PII hygiene). */
  tracePrompt?: 'full' | 'redacted';
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
  customerId: string,
  sessionId: string,
): Promise<TurnRuntimeSnapshot> {
  const intentStack = input.intent?.enabled
    ? await loadIntentStack(sessionId, input.sessionStore)
    : [];
  const lifecycle = await getCustomerLifecycleMetadata(customerId, input.customerStore);
  const presentFields = await getCustomerPresentFields(customerId, input.customerStore);
  return { intentStack, lifecycle, presentFields };
}

/** Persist a turn message. `messageStore` (AelioDb) is the sole store for conversation content. */
async function persistMessage(input: ProcessTurnInput, message: PersistMessageInput): Promise<void> {
  await input.messageStore.appendMessage({
    ...message,
    channel: message.channel,
  });
  input.contextEngine?.record(message.customerId, {
    role: message.role,
    content: message.content,
  });
  await touchSessionActivity(message.sessionId, new Date(), input.sessionStore);
}

async function loadTurnHistory(
  input: ProcessTurnInput,
  sessionId: string,
): Promise<Awaited<ReturnType<ConvoxMessageStore['loadHistory']>>> {
  return input.messageStore.loadHistory(sessionId, input.historyWindow);
}

const DEFAULT_PERSONA = `You are Aelio, a helpful conversational assistant for a SaaS product.
Answer clearly and concisely. Use available tools when you need account-specific data.
Never invent account details — always use tools for factual lookups.`;

const TOOL_GUIDANCE = `For write actions, do NOT ask the user to confirm yourself — once you have the required
arguments, call the tool directly. The platform automatically asks the user to confirm
before any write executes, so a second confirmation question from you is redundant.`;

function assertRequiredStores(input: ProcessTurnInput): void {
  if (!input.messageStore) {
    throw new Error('AelioDb messageStore is required');
  }
  if (!input.sessionStore) {
    throw new Error('AelioDb sessionStore is required');
  }
  if (!input.customerStore) {
    throw new Error('AelioDb customerStore is required');
  }
  if (!input.functionCallStore) {
    throw new Error('AelioDb functionCallStore is required');
  }
  if (input.memoryEnabled !== false && !input.memoryStore) {
    throw new Error('AelioDb memoryStore is required when memoryEnabled is true');
  }
  if (input.cache?.enabled && !input.responseCacheStore) {
    throw new Error('AelioDb responseCacheStore is required when cache.enabled is true');
  }
  if (input.harness?.enabled && !input.ledgerAelioDb) {
    throw new Error('AelioDb ledgerAelioDb config is required when harness.enabled is true');
  }
}

export async function processTurn(input: ProcessTurnInput): Promise<ProcessTurnResult> {
  assertRequiredStores(input);
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
  const customerId = await ensureCustomer(
    externalId,
    input.channel,
    input.channelAddress,
    input.customerStore,
  );

  await assertWithinRateLimit(input.messageStore, customerId, input.rateLimit);

  const session = await findOrCreateSession(
    customerId,
    input.channel,
    input.idleTimeoutMinutes,
    input.sessionStore,
  );
  const turnId = randomUUID();
  const preTurnSnapshot = await loadTurnSnapshot(input, customerId, session.id);
  await persistMessage(input, {
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
      aelioDb: input.turnApiCallsAelioDb,
    },
    async () => executeTurn(input, {
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
    externalId: string;
    customerId: string;
    session: { id: string; customerId: string; channel: Channel };
    turnId: string;
    preTurnSnapshot: TurnRuntimeSnapshot;
  },
): Promise<ProcessTurnResult> {
  const { externalId, customerId, session, turnId, preTurnSnapshot } = state;

  const context = {
    customerId: externalId,
    sessionId: session.id,
    channel: input.channel,
    channelAddress: input.channelAddress,
  };

  const pending = await getPendingConfirmation(session.id, input.sessionStore);
  if (pending) {
    if (isConfirmationMessage(input.message)) {
      const fn = input.sdk.getFunctions().find((entry) => entry.name === pending.functionName);
      const invokeResult = await input.sdk.invoke(
        pending.functionName,
        pending.args,
        context,
      );

      await logFunctionCall(
        {
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
        },
        input.functionCallStore,
      );

      await clearPendingConfirmation(session.id, input.sessionStore);

      const reply = invokeResult.ok
        ? fn
          ? buildWriteSuccessReply(fn, invokeResult.data)
          : 'The action completed successfully.'
        : `I couldn't complete that action: ${invokeResult.error ?? 'unknown error'}`;
      input.tracer?.trace({
        turnId,
        sessionId: session.id,
        kind: 'confirmation',
        payload: {
          action: 'confirmed_pending_write',
          functionName: pending.functionName,
          status: invokeResult.ok ? 'success' : 'error',
          safetyLevel: pending.safetyLevel,
          reply,
        },
      });

      await persistMessage(input, {
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
      await clearPendingConfirmation(session.id, input.sessionStore);
      const reply = buildCancellationReply();
      input.tracer?.trace({
        turnId,
        sessionId: session.id,
        kind: 'confirmation',
        payload: {
          action: 'denied_pending_write',
          functionName: pending.functionName,
          safetyLevel: pending.safetyLevel,
          reply,
        },
      });
      await persistMessage(input, {
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

  // A plan parked awaiting this customer's input (or a recoil intent collecting
  // a value) means even a contentless-looking message is an answer — both the
  // generic gate and the response cache must stand aside for it. The AelioDb
  // probe is lazy and shared, so the default path (no gate match, cache off)
  // pays no extra read.
  let parkedPlanProbe: Promise<boolean> | undefined;
  const hasParkedPlanLazy = () =>
    (parkedPlanProbe ??=
      input.harness?.enabled && input.suspensionStore
        ? input.suspensionStore.get(session.id).then((plan) => plan !== null)
        : Promise.resolve(false));
  const recoilActive = preTurnSnapshot.intentStack[0]?.kind === 'recoil';

  // Generic hot-path gate: bare greetings/thanks/farewells get a deterministic
  // template reply before any embedding or LLM work. Cheaper than the cache
  // (no vector lookup) and journaled like every other decision.
  if (input.genericGate?.enabled !== false && !recoilActive) {
    const gated = evaluateGenericGate(input.message, input.genericGate);
    if (gated.hit && !(await hasParkedPlanLazy())) {
      input.tracer?.trace({
        turnId,
        sessionId: session.id,
        kind: 'generic',
        payload: {
          action: 'served_template_reply',
          kind: gated.kind,
          normalized: gated.normalized,
          userMessage: input.message,
          reply: gated.reply,
        },
      });
      await persistMessage(input, {
        sessionId: session.id,
        customerId,
        role: 'assistant',
        content: gated.reply,
        channel: input.channel,
        context: buildTurnContext(input, preTurnSnapshot),
      });
      return { reply: gated.reply, turnId };
    }
  }

  // Semantic response cache: serve a near-identical, recent, no-tool reply for
  // this customer without calling the LLM. Only no-tool replies are cached, so a
  // hit never returns stale account data. Skipped when a plan is parked awaiting
  // this customer's input — that message is an answer to resume the plan, not a
  // fresh question, and must reach the harness even if it resembles a cached one.
  if (input.cache?.enabled) {
    const cached = (await hasParkedPlanLazy())
      ? null
      : await lookupCachedResponse({
          customerId,
          message: input.message,
          threshold: input.cache.similarityThreshold,
          responseCacheStore: input.responseCacheStore!,
        });
    if (cached) {
      input.tracer?.trace({
        turnId,
        sessionId: session.id,
        kind: 'cache',
        payload: {
          action: 'served_cached_response',
          userMessage: input.message,
          reply: cached,
        },
      });
      await persistMessage(input, {
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

  const history = await loadTurnHistory(input, session.id);
  const summary = await maybeSummarizeSession({
    sessionStore: input.sessionStore,
    messageStore: input.messageStore,
    sessionId: session.id,
    summarizeAfter: input.summarizeAfter,
    llm: input.llm,
    model: input.model,
    maxTokens: input.maxTokens,
  });

  const intentStack = preTurnSnapshot.intentStack;
  const intentPrompt = input.intent?.enabled ? buildIntentStackPrompt(intentStack) : '';

  const lifecycle = preTurnSnapshot.lifecycle;
  const stateDef = lifecycle.lifecycleState
    ? input.sdk.getStates().find((entry) => entry.id === lifecycle.lifecycleState)
    : undefined;
  // Lifecycle scoping first (allowed/blocked per state), then relevance
  // retrieval. Hard policy enforcement and state tool boundaries remain
  // deterministic; semantic ranking only chooses among already-allowed paths.
  const allFlows = input.sdk.getFlows();
  const allPolicies = input.sdk.getPolicies();
  const stateScopedFunctions = filterFunctionsByState(input.sdk.getFunctions(), stateDef);
  const activeFlowTools: string[] = [];
  for (const flow of allFlows) {
    const progress = lifecycle.flowProgress?.[flow.id];
    if (!progress) continue;
    const step = flow.steps[Math.min(progress.currentStepIndex, flow.steps.length - 1)];
    if (step?.tool) {
      activeFlowTools.push(step.tool);
    }
  }

  // Independent context work fans out in parallel. The incoming message is
  // embedded exactly once here, then that single vector is reused by the pathway
  // router (memories, intent, policy, flow, tools) and the archetype engine
  // (conversational stance) — no source pays for its own embedding call.
  const temporalScope: TemporalScope = resolveTemporalScope({
    message: input.message,
    lastMessageAt: (await input.sessionStore.get(session.id))?.lastActivityAt ?? null,
  });
  input.tracer?.trace({
    turnId,
    sessionId: session.id,
    kind: 'temporal',
    payload: temporalScope,
  });

  const messageVector =
    input.pathwayEngine || input.archetypeEngine || input.axisStore
      ? await embed(input.message, {
          purpose: 'pathway_retrieval',
          customerId,
          turnId,
          sessionId: session.id,
        })
      : undefined;
  const [pathway, immediateContext, stance] = await Promise.all([
    input.pathwayEngine
      ? input.pathwayEngine.decide({
          customerId,
          message: input.message,
          functions: stateScopedFunctions,
          policies: allPolicies,
          flows: allFlows,
          activeStateId: lifecycle.lifecycleState,
          flowProgress: lifecycle.flowProgress,
          intentStack,
          forceIncludeTools: activeFlowTools,
          memoryLimit: input.memoryRecallLimit,
          queryVector: messageVector,
        })
      : Promise.resolve(null),
    input.contextEngine
      ? input.contextEngine.getPromptBlock(customerId)
      : Promise.resolve(''),
    input.archetypeEngine
      ? input.archetypeEngine.assessSafe({
          message: input.message,
          queryVector: messageVector,
        })
      : Promise.resolve(null),
  ]);

  // Axis recall: for each fed stance aspect, walk this customer's occurrence
  // chain inside the temporal window. Failures degrade to empty (never fail turn).
  const axisOccurrences =
    input.axisStore && messageVector && stance
      ? (
          await Promise.all(
            stance.categories
              .filter((category) => category.fed && category.aspectId)
              .slice(0, 4)
              .map((category) =>
                input.axisStore!
                  .traverseAxis({
                    customerId,
                    aspectId: category.aspectId!,
                    scope: temporalScope,
                    queryVector: messageVector,
                  })
                  .catch(() => []),
              ),
          )
        ).flat()
      : [];

  const evidenceItems: EvidenceItem[] = [];
  for (const occ of axisOccurrences) {
    evidenceItems.push(
      evidenceFromAxisOccurrence(occ, {
        now: Date.now(),
        activeFlowId: pathway?.flow?.definition.id ?? null,
      }),
    );
  }
  if (stance) {
    for (const category of stance.categories) {
      const item = evidenceFromStance(category);
      if (item) evidenceItems.push(item);
    }
  }
  if (pathway) {
    input.tracer?.trace({
      turnId,
      sessionId: session.id,
      kind: 'pathway',
      payload: {
        intent: pathway.intent,
        strategy: pathway.strategy,
        tools: pathway.tools.map((entry) => ({
          name: entry.fn.name,
          score: entry.score,
        })),
        policies: pathway.policies.map((policy) => ({
          id: policy.id,
          severity: policy.severity,
          relevance: policy.relevance,
        })),
        flow: pathway.flow
          ? {
              id: pathway.flow.definition.id,
              stepId: pathway.flow.stepId,
              score: pathway.flow.score,
            }
          : null,
        proactive: pathway.proactive,
        timings: pathway.timings,
        degradedSources: pathway.degradedSources,
      },
    });
  }
  if (stance && stance.categories.length > 0) {
    input.tracer?.trace({
      turnId,
      sessionId: session.id,
      kind: 'stance',
      payload: {
        overall: stance.overall,
        categories: stance.categories.map((category) => ({
          category: category.category,
          dominant: category.dominant,
          positive: category.positive,
          negative: category.negative,
          neutral: category.neutral,
          strength: category.strength,
          margin: category.margin,
          fed: category.fed,
          ...(category.span !== undefined ? { span: category.span } : {}),
        })),
        timings: stance.timings,
      },
    });
  }

  const scopedFunctions =
    pathway?.selectedFunctions ??
    (await selectRelevantTools(stateScopedFunctions, input.message, {
      forceInclude: activeFlowTools,
    }));

  const relevantPolicies = pathway
    ? pathway.policies.map(({ relevance: _relevance, ...policy }) => policy)
    : allPolicies;
  const relevantFlows = pathway?.flow ? [pathway.flow.definition] : allFlows;
  const lifecyclePrompt = buildLifecycleSystemPrompt({
    stateId: lifecycle.lifecycleState,
    state: stateDef,
    policies: relevantPolicies,
    flows: relevantFlows,
    flowProgress: lifecycle.flowProgress,
  });

  let memoriesPrompt = '';
  if (input.memoryEnabled !== false) {
    const recalled =
      pathway?.memories ??
      (await recallMemories(
        input.memoryStore,
        customerId,
        input.message,
        input.memoryRecallLimit,
      ));
    if (recalled.length > 0) {
      memoriesPrompt = summarizeMemories(recalled);
      for (const memory of recalled) {
        evidenceItems.push(evidenceFromMemory(memory, { now: Date.now() }));
      }
    }
  }

  const selectedEvidence = selectEvidence(evidenceItems, DEFAULT_EVIDENCE_CONFIG);
  const evidencePrompt = renderEvidenceBlock(selectedEvidence);
  if (selectedEvidence.length > 0) {
    input.tracer?.trace({
      turnId,
      sessionId: session.id,
      kind: 'evidence',
      payload: {
        configVersion: DEFAULT_EVIDENCE_CONFIG.version,
        items: selectedEvidence.map((item) => ({
          id: item.id,
          source: item.source,
          score: item.score,
          why: item.why,
        })),
      },
    });
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
  const system = buildTurnSystemPrompt({
    persona,
    toolGuidance: TOOL_GUIDANCE,
    brief,
    lifecyclePrompt: lifecyclePrompt ?? '',
    toolCards,
    pathway: pathway?.prompt ?? '',
    stance: stance?.prompt ?? '',
    temporal: renderTemporalScope(temporalScope),
    evidence: evidencePrompt,
    immediateContext,
    summary: summary ?? '',
    memories: memoriesPrompt,
    intent: intentPrompt,
  });
  input.tracer?.trace({
    turnId,
    sessionId: session.id,
    kind: 'prompt',
    payload: {
      userMessage: input.message,
      selectedTools: scopedFunctions.map((fn) => fn.name),
      hasPathway: Boolean(pathway?.prompt),
      hasStance: Boolean(stance?.prompt),
      hasTemporal: true,
      hasEvidence: evidencePrompt.length > 0,
      hasImmediateContext: immediateContext.length > 0,
      hasSummary: Boolean(summary),
      hasMemories: memoriesPrompt.length > 0,
      // Full prompts can carry customer PII (memories, context). 'redacted'
      // keeps the journal auditable (sections + size) without storing content.
      prompt:
        input.tracePrompt === 'redacted' ? `[redacted: ${system.length} chars]` : system,
    },
  });

  const engineInput = {
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
    functionCallStore: input.functionCallStore,
  };
  const loopResult = input.harness?.enabled
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
        ledgerAelioDb: input.ledgerAelioDb!,
        bindingCache: input.bindingCache,
        customerStore: input.customerStore,
      })
    : await runToolLoop(engineInput);
  input.tracer?.trace({
    turnId,
    sessionId: session.id,
    kind: 'reply',
    payload: {
      userMessage: input.message,
      reply: loopResult.reply,
      executedToolNames: loopResult.executedToolNames,
      toolCallsExecuted: loopResult.toolCallsExecuted,
      pendingConfirmation: loopResult.pendingConfirmation ?? null,
      strategy: pathway?.strategy ?? null,
      semanticIntent: pathway?.intent ?? null,
      stance: stance?.overall ?? null,
    },
  });

  if (loopResult.pendingConfirmation) {
    await setPendingConfirmation(session.id, loopResult.pendingConfirmation, input.sessionStore);
  }

  let postTurnSnapshot = preTurnSnapshot;
  if (input.intent?.enabled) {
    const nextStack = updateIntentStack({
      stack: intentStack,
      userMessage: input.message,
      assistantReply: loopResult.reply,
      toolNames: loopResult.executedToolNames,
      registeredTools: input.sdk.getFunctions(),
      semanticIntent: pathway
        ? {
            label: pathway.intent.label,
            confidence: pathway.intent.confidence,
          }
        : undefined,
      config: input.intent,
    });
    await saveIntentStack(session.id, nextStack, input.sessionStore);
    postTurnSnapshot = { ...preTurnSnapshot, intentStack: nextStack };
  }

  if (pathway) {
    await input.sessionStore.updateMetadata(session.id, {
      semanticPathway: {
        intent: pathway.intent,
        strategy: pathway.strategy,
        flowId: pathway.flow?.definition.id ?? null,
        flowStepId: pathway.flow?.stepId ?? null,
        proactive: pathway.proactive,
        evaluatedAt: Date.now(),
      },
    });
  }
  if (stance && stance.categories.some((category) => category.fed)) {
    await input.sessionStore.updateMetadata(session.id, {
      conversationalStance: {
        overall: stance.overall,
        categories: stance.categories
          .filter((category) => category.fed)
          .map((category) => ({
            category: category.category,
            dominant: category.dominant,
            strength: category.strength,
          })),
        evaluatedAt: Date.now(),
      },
    });
  }

  // Resolution state for the proactive daemon — deterministic, never LLM-chosen.
  const sessionRecord = await input.sessionStore.get(session.id);
  const priorResolution = (sessionRecord?.metadata?.resolution as
    | {
        state?: string;
        intentLabel?: string;
        flowId?: string | null;
        reason?: string;
        updatedAt?: number;
        attemptCount?: number;
      }
    | undefined) ?? null;
  const resolution = deriveResolutionState({
    pathwayStrategy: pathway?.strategy ?? null,
    pathwayProactive: pathway?.proactive?.action ?? null,
    stanceOverall: stance?.overall.valence ?? null,
    pendingConfirmation: Boolean(loopResult.pendingConfirmation),
    toolSuccess: loopResult.toolCallsExecuted > 0 && !loopResult.pendingConfirmation,
    explicitStop: pathway?.strategy === 'disengage',
    current: priorResolution
      ? {
          state: (priorResolution.state as
            | 'active'
            | 'awaiting_user'
            | 'awaiting_system'
            | 'resolved'
            | 'satisfied'
            | 'disengaged'
            | 'do_not_contact') ?? 'active',
          intentLabel: priorResolution.intentLabel,
          flowId: priorResolution.flowId,
          reason: priorResolution.reason ?? '',
          updatedAt: priorResolution.updatedAt ?? Date.now(),
          attemptCount: priorResolution.attemptCount ?? 0,
        }
      : null,
  });
  await input.sessionStore.updateMetadata(session.id, {
    resolution: {
      ...resolution,
      intentLabel: pathway?.intent.label ?? resolution.intentLabel,
      flowId: pathway?.flow?.definition.id ?? resolution.flowId ?? null,
    },
  });

  // Harness Axis write-back: durable occurrence chain per fed aspect.
  if (input.axisStore && messageVector && stance) {
    const fed = stance.categories.filter((category) => category.fed && category.aspectId);
    void Promise.all(
      fed.map((category, spanIndex) =>
        input.axisStore!.recordOccurrence({
          customerId,
          aspectId: category.aspectId!,
          aspectName: category.category,
          turnId,
          spanIndex,
          sessionId: session.id,
          span: category.span,
          valence: category.dominant,
          positive: category.positive,
          negative: category.negative,
          neutral: category.neutral,
          strength: category.strength,
          intentLabel: pathway?.intent.label,
          flowId: pathway?.flow?.definition.id,
          embedding: messageVector,
        }),
      ),
    ).catch(() => {});
  }

  await persistMessage(input, {
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
      memoryStore: input.memoryStore,
      customerId,
      sessionId: session.id,
      userMessage: input.message,
      assistantReply: loopResult.reply,
      turnId,
    });
  }

  // Self-learning stance: grow the aspect taxonomy from this message. Reuses the
  // single turn embedding and is cost-guarded (no LLM when already covered).
  if (input.archetypeEngine && messageVector) {
    void input.archetypeEngine.learn({
      message: input.message,
      queryVector: messageVector,
      llm: input.llm,
      model: input.model,
      maxTokens: input.maxTokens,
    });
  }

  // Cache only no-tool, non-pending, real replies — never tool-backed answers.
  if (
    input.cache?.enabled &&
    loopResult.toolCallsExecuted === 0 &&
    !loopResult.pendingConfirmation &&
    loopResult.reply.trim().length > 0
  ) {
    await storeCachedResponse({
      customerId,
      message: input.message,
      reply: loopResult.reply,
      ttlMinutes: input.cache.ttlMinutes,
      responseCacheStore: input.responseCacheStore!,
    });
  }

  return {
    reply: loopResult.reply,
    turnId,
    ...(loopResult.pendingConfirmation ? { awaitingConfirmation: true } : {}),
  };
}
