import type {
  FlowDefinition,
  FunctionDefinition,
  PolicyDefinition,
} from '@aelio/protocol';
import { cosineSimilarity, embed } from '../analyst/embeddings.js';
import type { IntentStack } from '../intent/index.js';
import type { FlowProgressRecord } from '../lifecycle/index.js';
import type {
  ConvoxMemoryStore,
  MemoryRecallHit,
} from '../storage/memories.js';
import {
  rankRelevantTools,
  TOOL_RETRIEVAL_THRESHOLD,
  TOOL_RETRIEVAL_TOP_K,
  type RankedTool,
} from '../runtime/tool-retrieval.js';

const DISENGAGE_RE =
  /\b(bye|goodbye|see you|talk later|that's all|that is all|stop|leave me alone|do not contact|don't contact|no more messages)\b/i;

export type PathwayStrategy =
  | 'reply'
  | 'execute'
  | 'guide'
  | 'resume'
  | 'disengage';

export type SemanticIntent = {
  label: string;
  confidence: number;
  source: 'tool' | 'stack' | 'general';
};

export type PathwayProactiveHint = {
  action: 'none' | 'consider_followup' | 'suppress';
  reason: string;
};

export type PathwayTimings = {
  embeddingMs: number;
  parallelRetrievalMs: number;
  totalMs: number;
  targetMs: number;
  budgetExceeded: boolean;
};

export type SemanticPathwayDecision = {
  intent: SemanticIntent;
  strategy: PathwayStrategy;
  tools: RankedTool[];
  selectedFunctions: FunctionDefinition[];
  policies: Array<PolicyDefinition & { relevance: number }>;
  flow: {
    definition: FlowDefinition;
    stepIndex: number;
    stepId: string;
    goal: string;
    score: number;
  } | null;
  memories: MemoryRecallHit[];
  proactive: PathwayProactiveHint;
  timings: PathwayTimings;
  /** Retrieval sources that failed and were omitted without failing the turn. */
  degradedSources: string[];
  prompt: string;
};

export type SemanticPathwayInput = {
  customerId: string;
  message: string;
  functions: FunctionDefinition[];
  policies: PolicyDefinition[];
  flows: FlowDefinition[];
  activeStateId?: string;
  flowProgress?: Record<string, FlowProgressRecord>;
  intentStack: IntentStack;
  forceIncludeTools?: string[];
  memoryLimit?: number;
  /** Precomputed message embedding to reuse; embeds internally when omitted. */
  queryVector?: number[];
};

export type SemanticPathwayEngineOptions = {
  memoryStore: ConvoxMemoryStore;
  toolThreshold?: number;
  toolTopK?: number;
  softPolicyTopK?: number;
  /** Observability target for embedding + retrieval, not a hard timeout. Default 200ms. */
  latencyTargetMs?: number;
};

type RankedPolicy = PolicyDefinition & { relevance: number };
type RankedFlow = { definition: FlowDefinition; score: number };

function policyDescriptor(policy: PolicyDefinition): string {
  return `${policy.id} — ${policy.severity} policy — ${policy.description}`;
}

function flowDescriptor(flow: FlowDefinition): string {
  const steps = flow.steps.map((step) => `${step.id}: ${step.goal}`).join(' — ');
  return `${flow.id} — ${flow.description} — ${steps}`;
}

function resolveIntent(tools: RankedTool[], stack: IntentStack): SemanticIntent {
  const best = tools[0];
  if (best) {
    return {
      label: best.fn.intent ?? best.fn.name,
      confidence: Math.max(0, Math.min(1, best.score)),
      source: 'tool',
    };
  }
  const current = stack[0];
  if (current) {
    return { label: current.label, confidence: 0.5, source: 'stack' };
  }
  return { label: 'general', confidence: 0, source: 'general' };
}

function currentFlowStep(
  flow: FlowDefinition,
  progress?: FlowProgressRecord,
): { stepIndex: number; stepId: string; goal: string } {
  const stepIndex = Math.min(
    progress?.currentStepIndex ?? 0,
    Math.max(flow.steps.length - 1, 0),
  );
  const step = flow.steps[stepIndex]!;
  return { stepIndex, stepId: step.id, goal: step.goal };
}

function chooseStrategy(input: {
  message: string;
  tools: RankedTool[];
  flow: SemanticPathwayDecision['flow'];
  intentStack: IntentStack;
}): PathwayStrategy {
  if (DISENGAGE_RE.test(input.message.trim())) return 'disengage';
  if (input.intentStack[0]?.kind === 'recoil') return 'resume';
  if (input.flow) return 'guide';
  if ((input.tools[0]?.score ?? 0) >= 0.12) return 'execute';
  return 'reply';
}

function proactiveHint(
  strategy: PathwayStrategy,
  flow: SemanticPathwayDecision['flow'],
): PathwayProactiveHint {
  if (strategy === 'disengage') {
    return {
      action: 'suppress',
      reason: 'The user signalled conversation completion or no further contact.',
    };
  }
  if (flow) {
    return {
      action: 'consider_followup',
      reason: `The guided flow "${flow.definition.id}" remains active at step "${flow.stepId}".`,
    };
  }
  if (strategy === 'resume') {
    return {
      action: 'consider_followup',
      reason: 'The harness is collecting information for a suspended user goal.',
    };
  }
  return { action: 'none', reason: 'No unresolved guided pathway was detected.' };
}

function renderDecision(decision: Omit<SemanticPathwayDecision, 'prompt'>): string {
  const lines = [
    'SEMANTIC PATHWAY DECISION (retrieval guidance; deterministic policies and harness gates remain authoritative):',
    `Intent: ${decision.intent.label} (confidence ${decision.intent.confidence.toFixed(3)}, source ${decision.intent.source})`,
    `Response strategy: ${decision.strategy}`,
  ];
  if (decision.flow) {
    lines.push(
      `Selected flow: ${decision.flow.definition.id}; current step ${decision.flow.stepId} — ${decision.flow.goal}`,
    );
  }
  if (decision.tools.length > 0) {
    lines.push(
      `Most relevant capabilities: ${decision.tools
        .slice(0, 5)
        .map((entry) => `${entry.fn.name} (${entry.score.toFixed(3)})`)
        .join(', ')}`,
    );
  }
  if (decision.policies.length > 0) {
    lines.push(
      `Applicable policies: ${decision.policies
        .map((policy) => `${policy.id} [${policy.severity}]`)
        .join(', ')}`,
    );
  }
  if (decision.degradedSources.length > 0) {
    lines.push(`Unavailable retrieval sources: ${decision.degradedSources.join(', ')}`);
  }
  lines.push(
    decision.strategy === 'disengage'
      ? 'Respect the user’s closure: reply briefly, do not introduce a new goal, and do not schedule re-engagement.'
      : 'Let the user’s stated goal control the conversation. Guide assertively only when it helps complete that goal.',
  );
  return lines.join('\n');
}

export class SemanticPathwayEngine {
  private readonly memoryStore: ConvoxMemoryStore;
  private readonly toolThreshold: number;
  private readonly toolTopK: number;
  private readonly softPolicyTopK: number;
  private readonly latencyTargetMs: number;

  constructor(options: SemanticPathwayEngineOptions) {
    this.memoryStore = options.memoryStore;
    this.toolThreshold = options.toolThreshold ?? TOOL_RETRIEVAL_THRESHOLD;
    this.toolTopK = options.toolTopK ?? TOOL_RETRIEVAL_TOP_K;
    this.softPolicyTopK = options.softPolicyTopK ?? 3;
    this.latencyTargetMs = options.latencyTargetMs ?? 200;
  }

  async decide(input: SemanticPathwayInput): Promise<SemanticPathwayDecision> {
    const totalStarted = performance.now();
    const embeddingStarted = performance.now();
    const queryVector =
      input.queryVector ??
      (await embed(input.message, {
        purpose: 'pathway_retrieval',
        customerId: input.customerId,
      }));
    const embeddingMs = performance.now() - embeddingStarted;
    const retrievalStarted = performance.now();
    const degradedSources: string[] = [];

    const stateFlows = input.activeStateId
      ? input.flows.filter((flow) => flow.state === input.activeStateId)
      : [];
    const hardPolicies = input.policies.filter((policy) => policy.severity === 'hard');
    const softPolicies = input.policies.filter((policy) => policy.severity !== 'hard');

    const [rankedTools, memories, rankedPolicies, rankedFlows] = await Promise.all([
      rankRelevantTools(input.functions, input.message, queryVector),
      this.memoryStore
        .recallByVector(input.customerId, queryVector, input.memoryLimit ?? 5)
        .catch(() => {
          degradedSources.push('memories');
          return [];
        }),
      this.rankPolicies(softPolicies, queryVector),
      this.rankFlows(stateFlows, queryVector),
    ]);

    const forced = new Set(input.forceIncludeTools ?? []);
    const selectedNames = new Set<string>();
    for (const fn of input.functions) {
      if (forced.has(fn.name)) selectedNames.add(fn.name);
    }
    if (input.functions.length <= this.toolThreshold) {
      for (const fn of input.functions) selectedNames.add(fn.name);
    } else {
      for (const entry of rankedTools) {
        if (selectedNames.size >= Math.max(this.toolTopK, forced.size)) break;
        selectedNames.add(entry.fn.name);
      }
    }
    const selectedFunctions = input.functions.filter((fn) => selectedNames.has(fn.name));

    const policies: RankedPolicy[] = [
      ...hardPolicies.map((policy) => ({ ...policy, relevance: 1 })),
      ...rankedPolicies.slice(0, this.softPolicyTopK),
    ];

    // A flow already carrying progress wins; otherwise semantic relevance picks
    // among flows valid for the active lifecycle state.
    const progressed = rankedFlows.find(
      ({ definition }) => (input.flowProgress?.[definition.id]?.completedSteps.length ?? 0) > 0,
    );
    const selectedFlow = progressed ?? rankedFlows[0] ?? null;
    const flow = selectedFlow
      ? {
          definition: selectedFlow.definition,
          ...currentFlowStep(
            selectedFlow.definition,
            input.flowProgress?.[selectedFlow.definition.id],
          ),
          score: selectedFlow.score,
        }
      : null;

    const intent = resolveIntent(rankedTools, input.intentStack);
    const strategy = chooseStrategy({
      message: input.message,
      tools: rankedTools,
      flow,
      intentStack: input.intentStack,
    });
    const parallelRetrievalMs = performance.now() - retrievalStarted;
    const totalMs = performance.now() - totalStarted;
    const base = {
      intent,
      strategy,
      tools: rankedTools.slice(0, this.toolTopK),
      selectedFunctions,
      policies,
      flow,
      memories,
      proactive: proactiveHint(strategy, flow),
      degradedSources,
      timings: {
        embeddingMs,
        parallelRetrievalMs,
        totalMs,
        targetMs: this.latencyTargetMs,
        budgetExceeded: totalMs > this.latencyTargetMs,
      },
    };
    return { ...base, prompt: renderDecision(base) };
  }

  private async rankPolicies(
    policies: PolicyDefinition[],
    queryVector: number[],
  ): Promise<RankedPolicy[]> {
    const ranked = await Promise.all(
      policies.map(async (policy) => ({
        ...policy,
        relevance: cosineSimilarity(queryVector, await embed(policyDescriptor(policy))),
      })),
    );
    return ranked.sort((a, b) => b.relevance - a.relevance);
  }

  private async rankFlows(
    flows: FlowDefinition[],
    queryVector: number[],
  ): Promise<RankedFlow[]> {
    const ranked = await Promise.all(
      flows.map(async (definition) => ({
        definition,
        score: cosineSimilarity(queryVector, await embed(flowDescriptor(definition))),
      })),
    );
    return ranked.sort((a, b) => b.score - a.score);
  }
}

export function createSemanticPathwayEngine(
  options: SemanticPathwayEngineOptions,
): SemanticPathwayEngine {
  return new SemanticPathwayEngine(options);
}
