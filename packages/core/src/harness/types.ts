import type { ChatMessage } from '@aelio/llm';
import type { FunctionDefinition, InvocationContext } from '@aelio/protocol';
import type { AelioDatabase } from '@aelio/db';
import type { LLMProvider } from '@aelio/llm';
import type { SdkBridge } from '../sdk-bridge/types.js';
import type { SafetyConfig } from '../safety/policy.js';
import type { PendingConfirmation } from '../safety/confirmations.js';

export type PlanStatus =
  | 'pending'
  | 'running'
  | 'done'
  | 'failed'
  | 'aborted'
  | 'awaiting_confirmation';

export type StepStatus = 'pending' | 'running' | 'success' | 'failed' | 'skipped';

export type HarnessConfig = {
  enabled: boolean;
  routerModel?: string;
  plannerModel?: string;
  synthesisModel?: string;
  planStepCap: number;
  replanCap: number;
  tokenBudget: number;
  wallClockMs: number;
  flowConfidenceThreshold: number;
  toolRetrievalK: number;
  forceCategories: string[];
  forceOnActiveFlow: boolean;
};

export const DEFAULT_HARNESS_CONFIG: HarnessConfig = {
  enabled: false,
  planStepCap: 10,
  replanCap: 3,
  tokenBudget: 50_000,
  wallClockMs: 60_000,
  flowConfidenceThreshold: 0.82,
  toolRetrievalK: 12,
  forceCategories: ['transaction', 'checkout', 'order', 'multi_step'],
  forceOnActiveFlow: true,
};

export type RoutedIntent = {
  intent: string;
  category: string | null;
};

export type PlannedStep = {
  tool_name: string;
  input: Record<string, unknown>;
  depends_on_step_index?: number[];
};

export type GeneratedPlan = {
  steps: PlannedStep[];
};

export type PlanStepRecord = {
  id: string;
  planId: string;
  stepOrder: number;
  toolName: string;
  dependsOn: string[];
  inputTemplate: Record<string, unknown>;
  resolvedInput: Record<string, unknown> | null;
  output: unknown;
  status: StepStatus;
  idempotencyKey: string;
  attemptCount: number;
  errorMessage: string | null;
  startedAt: Date | null;
  finishedAt: Date | null;
};

export type PlanRecord = {
  id: string;
  sessionId: string;
  customerId: string;
  turnId: string;
  status: PlanStatus;
  matchedFlowId: string | null;
  userMessage: string;
  intent: string | null;
  intentCategory: string | null;
  tokenSpend: number;
  replanCount: number;
  abortReason: string | null;
  createdAt: Date;
  updatedAt: Date;
};

export type HarnessTurnInput = {
  database: AelioDatabase;
  llm: LLMProvider;
  sdk: SdkBridge;
  model: string;
  maxTokens: number;
  harness: HarnessConfig;
  internalCustomerId: string;
  sessionId: string;
  turnId: string;
  userMessage: string;
  history: ChatMessage[];
  system: string;
  functions: FunctionDefinition[];
  context: InvocationContext;
  safety: SafetyConfig;
  activeFlowId?: string;
};

export type HarnessTurnResult = {
  reply: string;
  toolCallsExecuted: number;
  executedToolNames: string[];
  planId?: string;
  pendingConfirmation?: PendingConfirmation;
  usedHarness: boolean;
};

export class PlanValidationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'PlanValidationError';
  }
}

export class BudgetExceededError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'BudgetExceededError';
  }
}
