import type { AelioDatabase } from '@aelio/db';
import type {
  AttributeDefinition,
  FlowDefinition,
  FlowStepDefinition,
  PipelineManifest,
  PipelineStageDefinition,
  UiDirective,
} from '@aelio/protocol';
import {
  getCustomerLifecycleMetadata,
  type FlowProgressRecord,
} from '../lifecycle/index.js';
import {
  collectAttributeFromMessage,
  findFirstIncompleteStepIndex,
  matchEnumValue,
  resolveFlowStep,
  stepType,
} from './flow-runner.js';
import {
  getCustomerAttributes,
  getPipelineState,
  hasAttribute,
  upsertPipelineState,
  type CustomerAttributeRecord,
} from './state-store.js';

/** Short/generic replies must not satisfy free-text attribute steps (e.g. name intake). */
const GENERIC_ATTRIBUTE_REPLIES = new Set([
  'yes',
  'yeah',
  'yep',
  'sure',
  'ok',
  'okay',
  'hi',
  'hey',
  'hello',
  'hii',
  'hiii',
  'no',
  'nope',
  'thanks',
  'thank you',
]);

export function isSubstantiveAttributeValue(
  message: string,
  attributeDef?: AttributeDefinition,
): boolean {
  const trimmed = message.trim();
  if (attributeDef?.enum_values?.length) {
    return matchEnumValue(trimmed, attributeDef.enum_values) !== undefined;
  }
  if (trimmed.length < 2) {
    return false;
  }
  return !GENERIC_ATTRIBUTE_REPLIES.has(trimmed.toLowerCase());
}

/** Ensure currentStep is set when the stage references a flow (manifest present, step missing). */
export function patchPipelineContext(
  pipeline: PipelineContext,
  flows: FlowDefinition[],
): PipelineContext {
  if (pipeline.currentStep || !pipeline.enabled || !pipeline.stage?.flow) {
    return pipeline;
  }
  const activeFlow = flows.find((flow) => flow.id === pipeline.stage!.flow);
  if (!activeFlow?.steps.length) {
    return pipeline;
  }
  const stepIndex = Math.min(
    Math.max(pipeline.currentStepIndex ?? 0, 0),
    activeFlow.steps.length - 1,
  );
  const currentStep = activeFlow.steps[stepIndex];
  if (!currentStep) {
    return pipeline;
  }
  return {
    ...pipeline,
    activeFlow,
    currentStep,
    currentStepIndex: stepIndex,
  };
}

export type PipelineEvaluateInput = {
  database: AelioDatabase;
  internalCustomerId: string;
  externalId: string;
  userMessage: string;
  manifest: PipelineManifest | null;
  flows: FlowDefinition[];
  attributes: AttributeDefinition[];
  /** When true the user authenticated via magic link / session token. */
  authenticated: boolean;
};

export type PipelineContext = {
  enabled: boolean;
  globalStage: string;
  stage?: PipelineStageDefinition;
  activeFlow?: FlowDefinition;
  currentStep?: FlowStepDefinition;
  currentStepIndex: number;
  flowProgress?: FlowProgressRecord;
  uiDirective?: UiDirective;
  pipelinePrompt: string;
  /** Tenant-defined profile memory — collected values + schema for harness context. */
  memoryPrompt: string;
  forceIncludeTools: string[];
  collectedAttributeThisTurn?: string;
};

function buildStagePrompt(
  stage: PipelineStageDefinition,
  step?: FlowStepDefinition,
  attributeDef?: AttributeDefinition,
): string {
  const lines: string[] = [
    `Pipeline stage: ${stage.id}`,
    stage.description.trim(),
  ];

  if (stage.content?.greeting) {
    lines.push(`Opening message for this stage: ${stage.content.greeting}`);
  }

  if (step) {
    const kind = stepType(step);
    lines.push(`Active pipeline step (${kind}): ${step.goal}`);
    if (kind === 'tool' && step.tool) {
      lines.push(`This step completes when tool "${step.tool}" succeeds. Do not skip ahead.`);
    }
    if (kind === 'attribute' && step.attribute) {
      const prompt =
        attributeDef?.prompts?.[0] ??
        `Collect the "${step.attribute}" attribute from the user conversationally.`;
      lines.push(`PROFILE COLLECTION — NOT A TOOL. Collect attribute "${step.attribute}": ${prompt}`);
      lines.push(
        'Do NOT invoke tools or mention step ids. Ask naturally; the platform saves the answer automatically.',
      );
    }
    if (kind === 'content') {
      lines.push(
        'CONTENT STEP — deliver the goal conversationally. No tool call; the platform advances when the user responds.',
      );
    }
    lines.push('Guide the user through this step only. Do not skip to later pipeline steps.');
  }

  lines.push('The pipeline engine advances steps automatically — do not announce stage changes yourself.');

  return lines.join('\n');
}

/** Profile memory block: SaaS-defined attribute schema + collected values for this customer. */
export function buildProfileMemoryPrompt(
  schema: AttributeDefinition[],
  collected: CustomerAttributeRecord[],
): string {
  if (schema.length === 0 && collected.length === 0) {
    return '';
  }

  const values = new Map(collected.map((entry) => [entry.attributeId, entry.value]));
  const lines = [
    'Profile memory (tenant-defined — use as factual context; never invent values):',
  ];

  for (const def of schema) {
    const value = values.get(def.id);
    if (value !== undefined && value !== null && value !== '') {
      lines.push(`- ${def.id} (${def.label}): ${JSON.stringify(value)}`);
    } else {
      lines.push(`- ${def.id} (${def.label}): [not collected]`);
    }
  }

  for (const entry of collected) {
    if (!schema.some((def) => def.id === entry.attributeId)) {
      lines.push(`- ${entry.attributeId}: ${JSON.stringify(entry.value)}`);
    }
  }

  return lines.join('\n');
}

function buildUiDirective(
  step: FlowStepDefinition,
  attributeDef?: AttributeDefinition,
): UiDirective | undefined {
  const ui = step.ui_format ?? attributeDef?.ui_format;
  if (!ui) {
    if (attributeDef?.enum_values?.length) {
      return {
        component: 'quick_reply',
        config: { options: attributeDef.enum_values.map(String) },
        attribute_id: step.attribute,
        step_id: step.id,
      };
    }
    return undefined;
  }

  return {
    component: ui.component,
    config: ui.config,
    attribute_id: step.attribute,
    step_id: step.id,
  };
}

async function buildFlowStepContext(input: {
  database: AelioDatabase;
  internalCustomerId: string;
  externalId: string;
  userMessage: string;
  attributes: AttributeDefinition[];
  activeFlow: FlowDefinition;
  flowProgress: FlowProgressRecord | undefined;
  globalStage: string;
  stage?: PipelineStageDefinition;
  pipelinePrompt?: string;
}): Promise<Omit<PipelineContext, 'enabled'>> {
  const db = input.database.db;
  const rawIndex = input.flowProgress?.currentStepIndex ?? 0;
  const currentStepIndex = await findFirstIncompleteStepIndex(
    db,
    input.internalCustomerId,
    input.activeFlow,
    rawIndex,
  );
  const currentStep = resolveFlowStep(input.activeFlow, currentStepIndex) ?? undefined;
  const attributeDef =
    currentStep?.attribute != null
      ? input.attributes.find((entry) => entry.id === currentStep.attribute)
      : undefined;

  let collectedAttributeThisTurn: string | undefined;
  if (currentStep && stepType(currentStep) === 'attribute' && currentStep.attribute) {
    const already = await hasAttribute(db, input.internalCustomerId, currentStep.attribute);
    if (!already) {
      const substantive = isSubstantiveAttributeValue(input.userMessage, attributeDef);
      if (substantive) {
        await collectAttributeFromMessage(
          db,
          input.internalCustomerId,
          currentStep.attribute,
          input.userMessage,
          attributeDef?.enum_values,
        );
        collectedAttributeThisTurn = currentStep.attribute;
      }
    }
  }

  const collectedAttributes = await getCustomerAttributes(db, input.internalCustomerId);
  const memoryPrompt = buildProfileMemoryPrompt(input.attributes, collectedAttributes);

  const forceIncludeTools: string[] = [];
  if (currentStep?.tool) {
    forceIncludeTools.push(currentStep.tool);
  }
  if (input.stage?.allowedTools) {
    forceIncludeTools.push(...input.stage.allowedTools);
  }

  const uiDirective = currentStep ? buildUiDirective(currentStep, attributeDef) : undefined;
  const pipelinePrompt =
    input.pipelinePrompt ??
    (input.stage
      ? buildStagePrompt(input.stage, currentStep, attributeDef)
      : `Lifecycle flow "${input.activeFlow.id}" (stage ${input.globalStage}): guide the active step conversationally.`);

  return {
    globalStage: input.globalStage,
    stage: input.stage,
    activeFlow: input.activeFlow,
    currentStep,
    currentStepIndex,
    flowProgress: input.flowProgress,
    uiDirective,
    pipelinePrompt,
    memoryPrompt,
    forceIncludeTools,
    collectedAttributeThisTurn,
  };
}

/** Resolve flow steps from lifecycle metadata when no pipeline manifest is registered. */
async function evaluateLifecycleFlow(
  input: PipelineEvaluateInput,
  lifecycle: Awaited<ReturnType<typeof getCustomerLifecycleMetadata>>,
  disabled: PipelineContext,
): Promise<PipelineContext> {
  const lifecycleState = lifecycle.lifecycleState;
  if (!lifecycleState) {
    return disabled;
  }

  const activeFlows = input.flows.filter((flow) => flow.state === lifecycleState);
  if (activeFlows.length === 0) {
    return disabled;
  }

  let activeFlow = activeFlows[0]!;
  for (const flow of activeFlows) {
    if (lifecycle.flowProgress?.[flow.id]) {
      activeFlow = flow;
      break;
    }
  }

  const ctx = await buildFlowStepContext({
    database: input.database,
    internalCustomerId: input.internalCustomerId,
    externalId: input.externalId,
    userMessage: input.userMessage,
    attributes: input.attributes,
    activeFlow,
    flowProgress: lifecycle.flowProgress?.[activeFlow.id],
    globalStage: lifecycleState,
  });

  if (!ctx.currentStep) {
    return disabled;
  }

  return { enabled: true, ...ctx };
}

export async function evaluatePipeline(input: PipelineEvaluateInput): Promise<PipelineContext> {
  const disabled: PipelineContext = {
    enabled: false,
    globalStage: 'active',
    currentStepIndex: 0,
    pipelinePrompt: '',
    memoryPrompt: '',
    forceIncludeTools: [],
  };

  const db = input.database.db;
  const lifecycle = await getCustomerLifecycleMetadata(db, input.internalCustomerId);

  if (!input.manifest || Object.keys(input.manifest.stages).length === 0) {
    return evaluateLifecycleFlow(input, lifecycle, disabled);
  }

  let pipelineState = await getPipelineState(db, input.internalCustomerId);

  let globalStage =
    pipelineState?.globalStage ??
    lifecycle.lifecycleState ??
    input.manifest.initial_stage ??
    'unverified';

  if (
    input.authenticated &&
    globalStage === 'unverified' &&
    input.manifest.stages.verified
  ) {
    globalStage = 'verified';
  }

  if (!pipelineState) {
    await upsertPipelineState(db, input.internalCustomerId, globalStage);
    pipelineState = { globalStage, enteredAt: new Date(), entryMethod: 'system_triggered' };
  }

  const stage = input.manifest.stages[globalStage];
  if (!stage) {
    return {
      ...disabled,
      enabled: true,
      globalStage,
      pipelinePrompt: `Pipeline stage "${globalStage}" has no manifest definition.`,
      memoryPrompt: '',
    };
  }

  if (!stage.flow) {
    const collectedAttributes = await getCustomerAttributes(db, input.internalCustomerId);
    return {
      enabled: true,
      globalStage,
      stage,
      currentStepIndex: 0,
      pipelinePrompt: buildStagePrompt(stage),
      memoryPrompt: buildProfileMemoryPrompt(input.attributes, collectedAttributes),
      forceIncludeTools: stage.allowedTools ?? [],
    };
  }

  const activeFlow = input.flows.find((flow) => flow.id === stage.flow);
  if (!activeFlow) {
    const collectedAttributes = await getCustomerAttributes(db, input.internalCustomerId);
    return {
      enabled: true,
      globalStage,
      stage,
      currentStepIndex: 0,
      pipelinePrompt: buildStagePrompt(stage),
      memoryPrompt: buildProfileMemoryPrompt(input.attributes, collectedAttributes),
      forceIncludeTools: stage.allowedTools ?? [],
    };
  }

  const ctx = await buildFlowStepContext({
    database: input.database,
    internalCustomerId: input.internalCustomerId,
    externalId: input.externalId,
    userMessage: input.userMessage,
    attributes: input.attributes,
    activeFlow,
    flowProgress: lifecycle.flowProgress?.[stage.flow],
    globalStage,
    stage,
  });

  return { enabled: true, ...ctx };
}
