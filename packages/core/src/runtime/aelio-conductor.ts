import type { FunctionDefinition, InvocationContext, StateDefinition } from '@aelio/protocol';
import type { LLMProvider } from '@aelio/llm';
import type { ChatMessage } from '@aelio/llm';
import { runHarness } from '../harness/index.js';
import type { HarnessBindingConfig, HarnessBudgets } from '../harness/schema.js';
import type { SuspensionStorePort } from '../harness/suspension.js';
import type { HarnessTracer } from '../harness/traces.js';
import type { LighthouseService } from '../lighthouse/index.js';
import {
  buildLifecycleSystemPrompt,
  filterFunctionsByState,
  type FlowProgressRecord,
} from '../lifecycle/index.js';
import type { SafetyConfig } from '../safety/policy.js';
import type { SdkBridge, SdkInvokeResult } from '../sdk-bridge/types.js';
import { summarizeMemories } from '../analyst/summarize.js';
import { hashArgs } from '../harness/executor.js';
import type { AelioMemoryStore } from './aelio-memory.js';
import type { Conductor } from './conductor.js';
import { composeSystemPrompt } from './prompt-composer.js';
import { selectRelevantTools } from './tool-retrieval.js';
import { runToolLoop } from './tool-loop.js';
import type { JsonValue, RuntimeDecisionV1, RuntimeEventV1, RuntimeSnapshotV1 } from './contracts.js';

/**
 * The durable per-subject state the Aelio Runtime owns. Everything a turn needs to be decided
 * lives here, so a turn never reads SQLite. Kinds are kept separate on purpose (§4.1 of the
 * implementation plan): they differ in writer, retention, and revision semantics.
 */
export type AelioSubjectState = {
  version: 1;
  /** Interaction state: the rolling conversation window used to prompt the model. */
  history: Array<{ role: 'user' | 'assistant'; content: string; at: number }>;
  /** Lifecycle state: advanced only by declarative transitions or an SDK push. */
  lifecycleState: string | null;
  lifecycleReason: string | null;
  /** Workflow state: how far the subject has progressed through each declared SDK flow. */
  flowProgress: Record<string, FlowProgressRecord>;
  /** Understanding state: verified fields available to `requires_fields` guards. */
  profile: Record<string, JsonValue>;
  turns: number;
  lastChannel: string | null;
  lastEventAt: number;
};

const DEFAULT_PERSONA = `You are Aelio, a helpful conversational assistant for a SaaS product.
Answer clearly and concisely. Use available tools when you need account-specific data.
Never invent account details — always use tools for factual lookups.`;

const TOOL_GUIDANCE = `For write actions, do NOT ask the user to confirm yourself — once you have the required
arguments, call the tool directly. The platform automatically asks the user to confirm
before any write executes, so a second confirmation question from you is redundant.`;

export type AelioConductorConfig = {
  llm: LLMProvider;
  sdk: SdkBridge;
  model: string;
  maxTokens: number;
  historyWindow: number;
  safety: SafetyConfig;
  /** Durable parked plans. Required: without it a mid-plan confirmation cannot survive a restart. */
  suspensionStore: SuspensionStorePort;
  harness: { enabled: boolean; budgets: HarnessBudgets; binding: HarnessBindingConfig };
  persona?: string | null;
  lighthouse?: LighthouseService;
  tracer?: HarnessTracer;
  memory?: AelioMemoryStore;
  memoryEnabled?: boolean;
  memoryRecallLimit?: number;
  /**
   * Makes tool execution exactly-once per logical event. Strongly recommended: without it, a crash
   * after a tool ran but before the turn committed will re-run that tool on replay.
   */
  effectJournal?: RuntimeEffectJournal;
};

export type AelioTurnDelivery = { channel: string; to: string };

/**
 * The durable intent/result journal a tool call is wrapped in. `AelioRuntimeStore` satisfies this
 * structurally; the conductor takes the narrow port so it never depends on the whole repository.
 */
export type RuntimeEffectJournal = {
  beginEffect(input: {
    key: string;
    tenantId: string;
    subjectId: string;
    instanceId: string;
    eventId: string;
    kind: string;
    request: JsonValue;
  }): Promise<{ state: 'new' } | { state: 'completed'; result: JsonValue } | { state: 'unknown' }>;
  recordEffectResult(input: {
    key: string;
    tenantId: string;
    subjectId: string;
    instanceId: string;
    eventId: string;
    result: JsonValue;
  }): Promise<void>;
};

/**
 * The Aelio Runtime control plane.
 *
 * It reproduces every capability the legacy SQLite turn had — lifecycle-scoped tools, relevance
 * retrieval, memory recall, the plan/bind/resolve/execute harness, confirmation gating, and
 * durable park/resume — with Aelio DB as the only store. The conductor itself decides nothing
 * about effects: it returns one `RuntimeDecisionV1`, and `runConductorEvent` commits the event
 * claim, snapshot revision, ledger, and outbox effects in a single conditional transaction.
 */
export function createAelioConductor(config: AelioConductorConfig): Conductor {
  return {
    decide: async ({ event, snapshot }) => decideTurn(config, event, snapshot),
  };
}

async function decideTurn(
  config: AelioConductorConfig,
  event: RuntimeEventV1,
  snapshot: RuntimeSnapshotV1 | null,
): Promise<RuntimeDecisionV1> {
  const state = readSubjectState(snapshot?.state);
  const payload = asObject(event.payload);
  const message = typeof payload.message === 'string' ? payload.message.trim() : '';
  const channel = typeof payload.channel === 'string' ? payload.channel : state.lastChannel ?? 'web';
  const delivery = readDelivery(payload.delivery);

  if (!message) {
    // A malformed or empty ingress is claimed and ledgered, then ignored. It must never reach the
    // planner, and it must never be silently retried by the provider.
    return { kind: 'exit', reason: 'empty_message', state: toJson({ ...state, lastEventAt: event.receivedAt }) };
  }

  // Session identity is the subject stream itself: parked plans, gates, and tool invocation
  // context all key on it, so a subject has exactly one ordered runtime conversation.
  const sessionId = `${event.tenantId}:${event.subjectId}`;
  const context: InvocationContext = {
    customerId: event.subjectId,
    sessionId,
    channel: channel as InvocationContext['channel'],
    channelAddress: delivery?.to ?? `${channel}:${event.subjectId}`,
  };

  const stateDef = state.lifecycleState
    ? config.sdk.getStates().find((entry) => entry.id === state.lifecycleState)
    : undefined;
  const presentFields = new Set(Object.keys(state.profile));
  const functions = await scopeFunctions(config, stateDef, message, state.flowProgress);
  const memoriesPrompt = await recallPrompt(config, event.subjectId, message);
  const system = buildSystemPrompt(config, { stateDef, functions, memoriesPrompt, state });
  const history = toChatMessages(state.history, config.historyWindow);

  // Lifecycle transitions are applied to the durable snapshot, not to SQLite. They are collected
  // during execution and folded into the state this decision commits.
  let transitionedTo: string | null = null;
  let transitionReason: string | null = null;
  const onToolSuccess = async (toolName: string): Promise<void> => {
    const applied = matchTransition(stateDef, toolName, presentFields);
    if (applied) {
      transitionedTo = applied;
      transitionReason = `auto: ${toolName} succeeded`;
    }
  };

  const sdk = config.effectJournal
    ? journaledSdk(config.sdk, config.effectJournal, event)
    : config.sdk;

  const engineInput = {
    llm: config.llm,
    sdk,
    functions,
    model: config.model,
    maxTokens: config.maxTokens,
    system,
    history,
    userMessage: message,
    context,
    safety: config.safety,
  };

  const result = config.harness.enabled
    ? await runHarness({
        ...engineInput,
        ...(stateDef ? { state: stateDef } : {}),
        presentFields,
        ...(config.lighthouse ? { lighthouse: config.lighthouse } : {}),
        ...(config.tracer ? { tracer: config.tracer } : {}),
        suspensionStore: config.suspensionStore,
        budgets: config.harness.budgets,
        binding: config.harness.binding,
        turnId: event.eventId,
        onToolSuccess,
      })
    : await runToolLoop(engineInput);

  const reply = result.reply?.trim() || 'I could not produce a response. Please try again.';

  // Durable facts are derived only from what actually happened this turn, and are written outside
  // the state transaction: memory is an enhancement and must never block or fail a committed turn.
  if (config.memoryEnabled !== false && config.memory) {
    void rememberTurn(config.memory, event.subjectId, message);
  }

  const nextState: AelioSubjectState = {
    ...state,
    history: appendHistory(state.history, message, reply, config.historyWindow, event.receivedAt),
    lifecycleState: transitionedTo ?? state.lifecycleState,
    lifecycleReason: transitionedTo ? transitionReason : state.lifecycleReason,
    turns: state.turns + 1,
    lastChannel: channel,
    lastEventAt: event.receivedAt,
  };

  return {
    kind: 'reply',
    text: reply,
    state: toJson(nextState),
    ...(delivery
      ? {
          effects: [{
            effectId: `${event.eventId}:reply`,
            idempotencyKey: `reply:${event.idempotencyKey}`,
            kind: 'reply.send',
            payload: { channel: delivery.channel, to: delivery.to, text: reply },
          }],
        }
      : {}),
  };
}

/**
 * Wrap tool invocation in the durable intent/result journal, keyed by the event's idempotency key
 * plus the tool and its canonical arguments.
 *
 * This is what closes the gap between "the tool ran" and "the turn committed". A tool executes
 * before the runtime transaction commits — it has to, because the reply depends on its result — so
 * a crash in between would otherwise replay the tool when the provider redelivers the message.
 *
 * The key is stable across replays of the same logical event and different for a genuinely new
 * message, so a user really can cancel the same order twice on purpose, but a redelivery cannot.
 * An interrupted call resolves to `unknown` and fails closed rather than being repeated blindly.
 */
function journaledSdk(sdk: SdkBridge, journal: RuntimeEffectJournal, event: RuntimeEventV1): SdkBridge {
  return {
    getFunctions: () => sdk.getFunctions(),
    getStates: () => sdk.getStates(),
    getPolicies: () => sdk.getPolicies(),
    getFlows: () => sdk.getFlows(),
    ...(sdk.getPersona ? { getPersona: () => sdk.getPersona!() } : {}),
    ...(sdk.getProductBrief ? { getProductBrief: () => sdk.getProductBrief!() } : {}),
    invoke: async (functionName, args, invocationContext) => {
      const scope = {
        key: `turn:${event.tenantId}:${event.idempotencyKey}:${functionName}:${hashArgs(args)}`,
        tenantId: event.tenantId,
        subjectId: event.subjectId,
        instanceId: event.eventId,
        eventId: event.eventId,
      };
      const claim = await journal.beginEffect({ ...scope, kind: 'turn.tool', request: { functionName, args } as JsonValue });
      if (claim.state === 'completed') {
        return (claim.result ?? { ok: false, error: 'missing journal result', durationMs: 0 }) as unknown as SdkInvokeResult;
      }
      if (claim.state === 'unknown') {
        return {
          ok: false,
          error: `a previous attempt at ${functionName} was interrupted and its outcome is unknown; it needs reconciliation`,
          durationMs: 0,
        };
      }
      const result = await sdk.invoke(functionName, args, invocationContext);
      await journal.recordEffectResult({ ...scope, result: result as unknown as JsonValue });
      return result;
    },
  };
}

/**
 * Lifecycle scoping first (allowed/blocked per state), then relevance retrieval — identical to
 * the legacy turn, so cutting a tenant over to the runtime cannot silently widen or narrow which
 * tools a state permits.
 */
async function scopeFunctions(
  config: AelioConductorConfig,
  stateDef: StateDefinition | undefined,
  message: string,
  flowProgress: Record<string, FlowProgressRecord>,
): Promise<FunctionDefinition[]> {
  const scoped = filterFunctionsByState(config.sdk.getFunctions(), stateDef);
  // The tool the subject's active flow step needs must survive relevance filtering, or a
  // multi-step flow can stall simply because this message did not mention it.
  const activeFlowTools: string[] = [];
  for (const flow of config.sdk.getFlows()) {
    const progress = flowProgress[flow.id];
    if (!progress) continue;
    const step = flow.steps[Math.min(progress.currentStepIndex, flow.steps.length - 1)];
    if (step?.tool) activeFlowTools.push(step.tool);
  }
  return selectRelevantTools(scoped, message, { forceInclude: activeFlowTools });
}

async function recallPrompt(
  config: AelioConductorConfig,
  subjectId: string,
  message: string,
): Promise<string> {
  if (config.memoryEnabled === false || !config.memory) return '';
  try {
    const recalled = await config.memory.recall(subjectId, message, config.memoryRecallLimit ?? 5);
    return recalled.length > 0 ? summarizeMemories(recalled) : '';
  } catch {
    return '';
  }
}

function buildSystemPrompt(
  config: AelioConductorConfig,
  input: {
    stateDef: StateDefinition | undefined;
    functions: FunctionDefinition[];
    memoriesPrompt: string;
    state: AelioSubjectState;
  },
): string {
  const persona = config.sdk.getPersona?.() ?? config.persona ?? DEFAULT_PERSONA;
  const lifecyclePrompt = buildLifecycleSystemPrompt({
    stateId: input.state.lifecycleState ?? undefined,
    state: input.stateDef,
    policies: config.sdk.getPolicies(),
    flows: config.sdk.getFlows(),
    flowProgress: input.state.flowProgress,
  });
  const brief = config.harness.enabled ? config.lighthouse?.getBrief() ?? '' : '';
  const toolCards = config.harness.enabled
    ? input.functions
        .map((fn) => `- ${fn.name} (${fn.intent ?? fn.name})${fn.safety !== 'read' ? ` [${fn.safety}]` : ''}: ${fn.description}`)
        .join('\n')
    : '';
  return composeSystemPrompt([
    { id: 'persona', content: persona, stability: 'stable', priority: 100, maxTokens: 800 },
    { id: 'guidance', content: TOOL_GUIDANCE, stability: 'stable', priority: 95 },
    { id: 'brief', content: brief, stability: 'stable', priority: 92, maxTokens: 1500 },
    { id: 'lifecycle', content: lifecyclePrompt ?? '', stability: 'stable', priority: 90, maxTokens: 1200 },
    { id: 'tools', content: toolCards ? `Available tools for this turn:\n${toolCards}` : '', stability: 'volatile', priority: 70, maxTokens: 1500 },
    { id: 'memories', content: input.memoriesPrompt, stability: 'volatile', priority: 50, maxTokens: 600 },
  ]);
}

/** Deterministic transition matching — the same rule the SQLite writer applies, without the write. */
function matchTransition(
  stateDef: StateDefinition | undefined,
  toolName: string,
  presentFields: Set<string>,
): string | null {
  for (const transition of stateDef?.transitions ?? []) {
    if (transition.on_tool_success !== toolName) continue;
    const requires = transition.guard?.requires_fields ?? [];
    if (requires.some((field) => !presentFields.has(field))) continue;
    return transition.to;
  }
  return null;
}

async function rememberTurn(memory: AelioMemoryStore, subjectId: string, message: string): Promise<void> {
  try {
    for (const fact of detectDurableFacts(message)) {
      await memory.remember({ subjectId, content: fact.content, category: fact.category });
    }
  } catch (error) {
    console.error(
      '[aelio] runtime memory write failed (turn already committed):',
      error instanceof Error ? error.message : String(error),
    );
  }
}

/**
 * Domain-neutral, explicit self-declarations only. Anything richer belongs to an asynchronous
 * reflection job, not to the critical path of a turn.
 */
function detectDurableFacts(message: string): Array<{ content: string; category: string }> {
  const facts: Array<{ content: string; category: string }> = [];
  const trimmed = message.trim();

  const email = trimmed.match(/[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/i);
  if (email) facts.push({ content: `User email is ${email[0]}`, category: 'profile' });

  const name = trimmed.match(/\bmy name is\s+([A-Za-z][A-Za-z '-]{1,40})/i);
  if (name?.[1]) facts.push({ content: `User name is ${name[1].trim()}`, category: 'profile' });

  const phone = trimmed.match(/\bmy (?:phone|number) is\s*([+()\d][\d\s()-]{6,18}\d)/i);
  if (phone?.[1]) facts.push({ content: `User phone is ${phone[1].trim()}`, category: 'profile' });

  for (const line of trimmed.split(/[.!?\n]/)) {
    const part = line.trim();
    if (part.length === 0 || part.length > 140) continue;
    if (
      /\b(?:i|we)\s+(?:prefer|always|usually|never|only|like to|want to|don'?t want)\b/i.test(part) ||
      /\bplease\s+(?:always|never|don'?t)\b/i.test(part)
    ) {
      facts.push({ content: `Stated preference: ${part}`, category: 'preference' });
    }
  }
  return [...new Map(facts.map((fact) => [`${fact.category}:${fact.content}`, fact])).values()];
}

function appendHistory(
  history: AelioSubjectState['history'],
  userMessage: string,
  reply: string,
  window: number,
  at: number,
): AelioSubjectState['history'] {
  const next = [
    ...history,
    { role: 'user' as const, content: userMessage, at },
    { role: 'assistant' as const, content: reply, at: Date.now() },
  ];
  // Keep the window generous enough to prompt with, but bounded: the snapshot is rewritten in
  // full on every turn, so unbounded history would make each commit grow without limit.
  return next.slice(-Math.max(window * 2, 2));
}

function toChatMessages(history: AelioSubjectState['history'], window: number): ChatMessage[] {
  return history.slice(-window).map((entry) => ({ role: entry.role, content: entry.content }));
}

export function readSubjectState(value: unknown): AelioSubjectState {
  const raw = value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
  const history = Array.isArray(raw.history)
    ? raw.history
        .filter((entry): entry is Record<string, unknown> => Boolean(entry) && typeof entry === 'object')
        .map((entry) => ({
          role: entry.role === 'assistant' ? ('assistant' as const) : ('user' as const),
          content: typeof entry.content === 'string' ? entry.content : '',
          at: typeof entry.at === 'number' ? entry.at : 0,
        }))
        .filter((entry) => entry.content.length > 0)
    : [];
  return {
    version: 1,
    history,
    lifecycleState: typeof raw.lifecycleState === 'string' ? raw.lifecycleState : null,
    lifecycleReason: typeof raw.lifecycleReason === 'string' ? raw.lifecycleReason : null,
    flowProgress:
      raw.flowProgress && typeof raw.flowProgress === 'object' && !Array.isArray(raw.flowProgress)
        ? (raw.flowProgress as Record<string, FlowProgressRecord>)
        : {},
    profile:
      raw.profile && typeof raw.profile === 'object' && !Array.isArray(raw.profile)
        ? (raw.profile as Record<string, JsonValue>)
        : {},
    turns: typeof raw.turns === 'number' ? raw.turns : 0,
    lastChannel: typeof raw.lastChannel === 'string' ? raw.lastChannel : null,
    lastEventAt: typeof raw.lastEventAt === 'number' ? raw.lastEventAt : 0,
  };
}

function toJson(state: AelioSubjectState): JsonValue {
  return state as unknown as JsonValue;
}

function asObject(value: JsonValue): Record<string, JsonValue> {
  return value && typeof value === 'object' && !Array.isArray(value) ? value : {};
}

function readDelivery(value: JsonValue | undefined): AelioTurnDelivery | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const channel = value.channel;
  const to = value.to;
  if (typeof channel !== 'string' || typeof to !== 'string' || !channel || !to) return null;
  return { channel, to };
}
