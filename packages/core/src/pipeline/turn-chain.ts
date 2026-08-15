import type { FlowDefinition } from '@aelio/protocol';
import type { InvocationContext } from '@aelio/protocol';
import type { ChatMessage, LLMProvider } from '@aelio/llm';
import type { AelioDatabase } from '@aelio/db';
import type { AttributeDefinition } from '@aelio/protocol';
import type { SdkBridge } from '../sdk-bridge/types.js';
import type { SafetyConfig } from '../safety/policy.js';
import type { ToolLoopResult } from '../runtime/tool-loop.js';
import {
  evaluatePipeline,
  patchPipelineContext,
  type PipelineContext,
  type PipelineEvaluateInput,
} from './evaluate.js';
import { stepType } from './flow-runner.js';
import { handlePipelinePostTurn } from './post-turn.js';
import {
  isPipelineToolStep,
  resolvePipelineRoute,
  runPipelineStepTurn,
  runPipelineToolTurn,
} from './step-turn.js';

async function coalesceReplies(input: {
  llm: LLMProvider;
  model: string;
  maxTokens: number;
  ack: string;
  followUp: string;
}): Promise<string> {
  if (!input.followUp.trim()) {
    return input.ack;
  }
  const result = await input.llm.complete({
    model: input.model,
    maxTokens: Math.min(input.maxTokens, 700),
    tools: [],
    system:
      'Combine the two assistant parts into one short, natural message. Do not say "one moment" or ask the user to wait.',
    messages: [
      {
        role: 'user',
        content: `Part 1 (acknowledgment): ${input.ack}\n\nPart 2 (action result): ${input.followUp}`,
      },
    ],
    telemetry: { purpose: 'chat_completion', iteration: 0 },
  });
  return result.text.trim() || `${input.ack}\n\n${input.followUp}`;
}

/** After a conversation step completes, immediately run the next tool step (same turn). */
async function chainNextToolStep(input: {
  database: AelioDatabase;
  externalId: string;
  internalCustomerId: string;
  flows: FlowDefinition[];
  evaluateBase: PipelineEvaluateInput;
  llm: LLMProvider;
  sdk: SdkBridge;
  model: string;
  maxTokens: number;
  system: string;
  history: ChatMessage[];
  userMessage: string;
  context: InvocationContext;
  safety: SafetyConfig;
  prior: ToolLoopResult;
}): Promise<ToolLoopResult> {
  const nextPipeline = patchPipelineContext(
    await evaluatePipeline({ ...input.evaluateBase, userMessage: input.userMessage }),
    input.flows,
  );
  if (!isPipelineToolStep(nextPipeline)) {
    return input.prior;
  }

  const toolResult = await runPipelineToolTurn({
    database: input.database,
    internalCustomerId: input.internalCustomerId,
    llm: input.llm,
    sdk: input.sdk,
    model: input.model,
    maxTokens: input.maxTokens,
    system: input.system,
    history: input.history,
    userMessage: input.userMessage,
    context: input.context,
    pipeline: nextPipeline,
    safety: input.safety,
  });

  await handlePipelinePostTurn({
    database: input.database,
    externalId: input.externalId,
    internalCustomerId: input.internalCustomerId,
    pipeline: nextPipeline,
    executedToolNames: toolResult.executedToolNames,
    userMessage: input.userMessage,
  });

  const reply = await coalesceReplies({
    llm: input.llm,
    model: input.model,
    maxTokens: input.maxTokens,
    ack: input.prior.reply,
    followUp: toolResult.reply,
  });

  return {
    reply,
    toolCallsExecuted: input.prior.toolCallsExecuted + toolResult.toolCallsExecuted,
    executedToolNames: [...input.prior.executedToolNames, ...toolResult.executedToolNames],
  };
}

function stepAdvancedThisTurn(pipeline: PipelineContext, userMessage: string): boolean {
  if (!pipeline.currentStep) {
    return false;
  }
  if (pipeline.collectedAttributeThisTurn) {
    return true;
  }
  return stepType(pipeline.currentStep) === 'content' && userMessage.trim().length >= 1;
}

export async function runPipelineTurn(input: {
  database: AelioDatabase;
  externalId: string;
  internalCustomerId: string;
  flows: FlowDefinition[];
  attributes: AttributeDefinition[];
  evaluateBase: PipelineEvaluateInput;
  pipeline: PipelineContext;
  llm: LLMProvider;
  sdk: SdkBridge;
  model: string;
  maxTokens: number;
  system: string;
  history: ChatMessage[];
  userMessage: string;
  context: InvocationContext;
  safety: SafetyConfig;
}): Promise<ToolLoopResult> {
  const route = resolvePipelineRoute(input.pipeline, input.userMessage);

  if (route === 'conversation') {
    let result = await runPipelineStepTurn({
      llm: input.llm,
      model: input.model,
      maxTokens: input.maxTokens,
      system: input.system,
      history: input.history,
      userMessage: input.userMessage,
      pipeline: input.pipeline,
      attributes: input.attributes,
    });

    await handlePipelinePostTurn({
      database: input.database,
      externalId: input.externalId,
      internalCustomerId: input.internalCustomerId,
      pipeline: input.pipeline,
      executedToolNames: result.executedToolNames,
      userMessage: input.userMessage,
    });

    if (stepAdvancedThisTurn(input.pipeline, input.userMessage)) {
      result = await chainNextToolStep({ ...input, prior: result });
    }

    return result;
  }

  if (route === 'tool') {
    const result = await runPipelineToolTurn({
      database: input.database,
      internalCustomerId: input.internalCustomerId,
      llm: input.llm,
      sdk: input.sdk,
      model: input.model,
      maxTokens: input.maxTokens,
      system: input.system,
      history: input.history,
      userMessage: input.userMessage,
      context: input.context,
      pipeline: input.pipeline,
      safety: input.safety,
    });

    await handlePipelinePostTurn({
      database: input.database,
      externalId: input.externalId,
      internalCustomerId: input.internalCustomerId,
      pipeline: input.pipeline,
      executedToolNames: result.executedToolNames,
      userMessage: input.userMessage,
    });

    return result;
  }

  return {
    reply: '',
    toolCallsExecuted: 0,
    executedToolNames: [],
  };
}
