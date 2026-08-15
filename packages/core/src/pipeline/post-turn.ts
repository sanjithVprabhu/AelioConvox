import type { AelioDatabase } from '@aelio/db';
import type { PipelineContext } from './evaluate.js';
import { advanceFlowProgress, stepMatchesTool, stepType } from './flow-runner.js';

export async function handlePipelinePostTurn(input: {
  database: AelioDatabase;
  externalId: string;
  internalCustomerId: string;
  pipeline: PipelineContext;
  executedToolNames: string[];
  userMessage: string;
}): Promise<void> {
  const { pipeline, database, externalId, internalCustomerId, executedToolNames, userMessage } =
    input;
  if (!pipeline.enabled || !pipeline.activeFlow || !pipeline.currentStep) {
    return;
  }

  const step = pipeline.currentStep;
  const stageNext = pipeline.stage?.next;

  if (pipeline.collectedAttributeThisTurn && stepType(step) === 'attribute') {
    await advanceFlowProgress(
      database.db,
      externalId,
      internalCustomerId,
      pipeline.activeFlow,
      pipeline.currentStepIndex,
      step.id,
      stageNext,
    );
    return;
  }

  if (stepType(step) === 'content' && userMessage.trim().length >= 1) {
    await advanceFlowProgress(
      database.db,
      externalId,
      internalCustomerId,
      pipeline.activeFlow,
      pipeline.currentStepIndex,
      step.id,
      stageNext,
    );
    return;
  }

  for (const toolName of executedToolNames) {
    if (stepMatchesTool(step, toolName)) {
      await advanceFlowProgress(
        database.db,
        externalId,
        internalCustomerId,
        pipeline.activeFlow,
        pipeline.currentStepIndex,
        step.id,
        stageNext,
      );
      return;
    }
  }
}
