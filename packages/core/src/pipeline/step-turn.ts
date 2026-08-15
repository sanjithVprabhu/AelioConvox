import type { InvocationContext } from '@aelio/protocol';
import type { ChatMessage, LLMProvider } from '@aelio/llm';
import type { AelioDatabase } from '@aelio/db';
import type { AttributeDefinition } from '@aelio/protocol';
import { logFunctionCall } from '../audit/function-calls.js';
import type { SdkBridge } from '../sdk-bridge/types.js';
import type { SafetyConfig } from '../safety/policy.js';
import { evaluateSafety } from '../safety/policy.js';
import type { ToolLoopResult } from '../runtime/tool-loop.js';
import type { PipelineContext } from './evaluate.js';
import { stepType } from './flow-runner.js';

const PIPELINE_STEP_GUIDANCE = `You are executing a PIPELINE STEP (conversation / profile collection — NOT a tool invocation).
- NEVER mention internal step ids (like "collect_name"), capabilities, or missing tools.
- NEVER say you don't have a way to do something — asking questions and saving answers IS how this step works.
- NEVER say "one moment", "please wait", "let me sync/check/pull that up" — either ask the next question or confirm what you received. The platform runs tools automatically; do not promise work you are not doing in this reply.
- Stay focused on the current step goal only. Keep replies concise and natural.`;

const PROCESS_QUESTION =
  /\b(what (do i|should i|to)|how (do i|to)|steps?|onboarding|complete|finish|need to do|what'?s left|remaining)\b/i;

function buildRemainingStepsOutline(pipeline: PipelineContext): string {
  if (!pipeline.activeFlow) {
    return '';
  }
  const lines = pipeline.activeFlow.steps.map((step, index) => {
    const kind = stepType(step);
    const marker =
      pipeline.currentStep?.id === step.id
        ? ' ← you are here'
        : index < pipeline.currentStepIndex
          ? ' ✓'
          : '';
    if (kind === 'tool' && step.tool) {
      return `${index + 1}. ${step.goal} (uses your ${step.tool} action)${marker}`;
    }
    if (kind === 'attribute') {
      return `${index + 1}. ${step.goal} (answer a quick question)${marker}`;
    }
    return `${index + 1}. ${step.goal}${marker}`;
  });
  return `Onboarding checklist for this flow:\n${lines.join('\n')}`;
}

/** Attribute/content steps, or meta questions during a tool step — bypass harness binding. */
export function shouldUsePipelineConversation(
  pipeline: PipelineContext,
  userMessage: string,
): boolean {
  if (!pipeline.enabled || !pipeline.currentStep) {
    return false;
  }
  const kind = stepType(pipeline.currentStep);
  if (kind === 'attribute' || kind === 'content') {
    return true;
  }
  if (kind === 'tool' && PROCESS_QUESTION.test(userMessage)) {
    return true;
  }
  return false;
}

/** @deprecated use shouldUsePipelineConversation */
export function isPipelineConversationStep(pipeline: PipelineContext): boolean {
  return shouldUsePipelineConversation(pipeline, '');
}

export function isPipelineToolStep(pipeline: PipelineContext): boolean {
  if (!pipeline.enabled || !pipeline.currentStep) {
    return false;
  }
  return stepType(pipeline.currentStep) === 'tool';
}

export type PipelineRoute = 'conversation' | 'tool' | null;

/** Which pipeline handler should run — null means fall through to harness/legacy loop. */
export function resolvePipelineRoute(
  pipeline: PipelineContext,
  userMessage: string,
): PipelineRoute {
  if (!pipeline.enabled || !pipeline.currentStep) {
    return null;
  }
  if (shouldUsePipelineConversation(pipeline, userMessage)) {
    return 'conversation';
  }
  if (isPipelineToolStep(pipeline)) {
    return 'tool';
  }
  return null;
}

export async function runPipelineStepTurn(input: {
  llm: LLMProvider;
  model: string;
  maxTokens: number;
  system: string;
  history: ChatMessage[];
  userMessage: string;
  pipeline: PipelineContext;
  attributes: AttributeDefinition[];
}): Promise<ToolLoopResult> {
  const step = input.pipeline.currentStep!;
  const kind = stepType(step);
  const attrDef = step.attribute
    ? input.attributes.find((entry) => entry.id === step.attribute)
    : undefined;
  const checklist = buildRemainingStepsOutline(input.pipeline);

  if (kind === 'attribute' && input.pipeline.collectedAttributeThisTurn && step.attribute) {
    const label = attrDef?.label ?? step.attribute;
    const result = await input.llm.complete({
      model: input.model,
      maxTokens: Math.min(input.maxTokens, 600),
      tools: [],
      system: `${input.system}\n\n${PIPELINE_STEP_GUIDANCE}\n\nThe user just provided their ${label} (${step.attribute}). Acknowledge warmly in one short sentence. Do not ask for it again.`,
      messages: [...input.history, { role: 'user', content: input.userMessage }],
      telemetry: { purpose: 'chat_completion', iteration: 0 },
    });
    return {
      reply: result.text.trim() || `Thanks — I've got that.`,
      toolCallsExecuted: 0,
      executedToolNames: [],
    };
  }

  let stepInstruction = step.goal;
  if (kind === 'attribute' && attrDef) {
    stepInstruction = attrDef.prompts?.[0] ?? step.goal;
    if (attrDef.enum_values?.length) {
      stepInstruction += ` Acceptable answers: ${attrDef.enum_values.map(String).join(', ')}.`;
    }
  }

  const processHint =
    PROCESS_QUESTION.test(input.userMessage) && checklist
      ? `\n\nThe user asked about the overall process. Use this outline — speak in plain language, never mention internal step ids:\n${checklist}`
      : '';

  const result = await input.llm.complete({
    model: input.model,
    maxTokens: input.maxTokens,
    tools: [],
    system: `${input.system}\n\n${PIPELINE_STEP_GUIDANCE}\n\nCurrent pipeline step type: ${kind}.\nStep goal: ${stepInstruction}${processHint}`,
    messages: [...input.history, { role: 'user', content: input.userMessage }],
    telemetry: { purpose: 'chat_completion', iteration: 0 },
  });

  return {
    reply: result.text.trim() || stepInstruction,
    toolCallsExecuted: 0,
    executedToolNames: [],
  };
}

/** Active pipeline tool step — invoke the step's SaaS tool directly (no planner bind). */
export async function runPipelineToolTurn(input: {
  database?: AelioDatabase;
  internalCustomerId?: string;
  llm: LLMProvider;
  sdk: SdkBridge;
  model: string;
  maxTokens: number;
  system: string;
  history: ChatMessage[];
  userMessage: string;
  context: InvocationContext;
  pipeline: PipelineContext;
  safety: SafetyConfig;
}): Promise<ToolLoopResult> {
  const step = input.pipeline.currentStep!;
  const toolName = step.tool;
  if (!toolName) {
    return {
      reply: 'This step is not ready yet — please try again in a moment.',
      toolCallsExecuted: 0,
      executedToolNames: [],
    };
  }

  const fn = input.sdk.getFunctions().find((entry) => entry.name === toolName);
  if (!fn) {
    return {
      reply: `The "${toolName}" action is not registered with ShopCo yet.`,
      toolCallsExecuted: 0,
      executedToolNames: [],
    };
  }

  const decision = evaluateSafety(fn, input.safety);
  if (!decision.allowed) {
    return {
      reply: decision.reason ?? 'That action is not allowed right now.',
      toolCallsExecuted: 0,
      executedToolNames: [],
    };
  }

  const invokeResult = await input.sdk.invoke(toolName, {}, input.context);

  if (input.database && input.internalCustomerId) {
    await logFunctionCall(input.database.db, {
      sessionId: input.context.sessionId,
      customerId: input.internalCustomerId,
      functionName: toolName,
      args: {},
      result: invokeResult.data,
      status: invokeResult.ok ? 'success' : 'error',
      safetyLevel: fn.safety,
      requiredConfirmation: false,
      confirmed: false,
      durationMs: invokeResult.durationMs,
      errorMessage: invokeResult.error,
    });
  }

  if (!invokeResult.ok) {
    return {
      reply: `I couldn't complete that: ${invokeResult.error ?? 'unknown error'}`,
      toolCallsExecuted: 0,
      executedToolNames: [],
    };
  }

  const result = await input.llm.complete({
    model: input.model,
    maxTokens: Math.min(input.maxTokens, 800),
    tools: [],
    system: `${input.system}\n\nThe active pipeline step just ran the "${toolName}" action successfully — the result is in the message below. Present the ACTUAL data to the user now in a clear, friendly summary. NEVER say "one moment", "please wait", or promise to fetch data — you already have it. Then briefly continue the tour if appropriate.`,
    messages: [
      ...input.history,
      { role: 'user', content: input.userMessage },
      {
        role: 'assistant',
        content: `[${toolName} result]: ${JSON.stringify(invokeResult.data)}`,
      },
    ],
    telemetry: { purpose: 'chat_completion', iteration: 0 },
  });

  return {
    reply: result.text.trim() || 'Done — your profile is synced.',
    toolCallsExecuted: 1,
    executedToolNames: [toolName],
  };
}
