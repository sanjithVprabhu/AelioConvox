import type { FunctionDefinition } from '@aelio/protocol';
import type { LLMProvider, ToolDefinition } from '@aelio/llm';
import type { AelioDatabase } from '@aelio/db';
import type { GeneratedPlan, HarnessConfig } from './types.js';
import { PlanValidationError } from './types.js';
import { recordLlmTokenSpend } from './budget.js';
import { assertValidPlan } from './cycle-detect.js';
import { buildInputSchema } from '../runtime/tool-schema.js';

const SUBMIT_PLAN_TOOL: ToolDefinition = {
  name: 'submit_plan',
  description: 'Submit the ordered list of tool calls needed to fulfill the user request',
  input_schema: {
    type: 'object',
    properties: {
      steps: {
        type: 'array',
        items: {
          type: 'object',
          properties: {
            tool_name: { type: 'string' },
            input: { type: 'object' },
            depends_on_step_index: {
              type: 'array',
              items: { type: 'integer' },
            },
          },
          required: ['tool_name', 'input'],
        },
      },
    },
    required: ['steps'],
  },
};

function buildPlannerSystem(
  tools: FunctionDefinition[],
  policies: string[],
  flowHint?: string,
): string {
  const toolLines = tools
    .map(
      (tool) =>
        `- ${tool.name}: ${tool.description}${tool.intent ? ` (intent: ${tool.intent})` : ''}`,
    )
    .join('\n');
  const policyBlock =
    policies.length > 0 ? `\nTenant policies:\n${policies.join('\n')}` : '';
  const flowBlock = flowHint ? `\nMatched flow hint:\n${flowHint}` : '';
  return `You are a planning agent. Produce a minimal ordered plan of tool calls to fulfill the user request.
Use submit_plan with tool_name values from the available tools only.
Reference prior step outputs in input using "$step_N.output.field" string placeholders.
Keep the plan as short as possible.${policyBlock}${flowBlock}

Available tools:
${toolLines}`;
}

export async function generatePlan(input: {
  llm: LLMProvider;
  model: string;
  maxTokens: number;
  userMessage: string;
  tools: FunctionDefinition[];
  policies: string[];
  config: HarnessConfig;
  database?: AelioDatabase['db'];
  planId?: string;
  flowHint?: string;
  failureContext?: string;
}): Promise<GeneratedPlan> {
  const system = buildPlannerSystem(input.tools, input.policies, input.flowHint);
  const userContent = input.failureContext
    ? `${input.userMessage}\n\nPrevious attempt failed:\n${input.failureContext}`
    : input.userMessage;

  const result = await input.llm.complete({
    model: input.model,
    maxTokens: input.maxTokens,
    system,
    messages: [{ role: 'user', content: userContent }],
    tools: [SUBMIT_PLAN_TOOL],
    toolChoice: { mode: 'required', name: 'submit_plan' },
    telemetry: {
      purpose: input.failureContext ? 'harness_replan' : 'harness_planner',
    },
  });

  if (input.database && input.planId) {
    await recordLlmTokenSpend(input.database, input.planId, result.usage);
  }

  const call = result.toolCalls.find((entry) => entry.name === 'submit_plan');
  if (!call) {
    throw new PlanValidationError('Planner did not return submit_plan');
  }

  const stepsRaw = call.args.steps;
  if (!Array.isArray(stepsRaw)) {
    throw new PlanValidationError('submit_plan.steps must be an array');
  }

  const plan: GeneratedPlan = {
    steps: stepsRaw.map((entry, index) => {
      if (!entry || typeof entry !== 'object') {
        throw new PlanValidationError(`Step ${index} is invalid`);
      }
      const record = entry as Record<string, unknown>;
      const toolName = record.tool_name;
      if (typeof toolName !== 'string' || !toolName.trim()) {
        throw new PlanValidationError(`Step ${index} is missing tool_name`);
      }
      const args =
        record.input && typeof record.input === 'object' && !Array.isArray(record.input)
          ? (record.input as Record<string, unknown>)
          : {};
      const depends =
        Array.isArray(record.depends_on_step_index) &&
        record.depends_on_step_index.every((value) => typeof value === 'number')
          ? (record.depends_on_step_index as number[])
          : [];
      return {
        tool_name: toolName,
        input: args,
        depends_on_step_index: depends,
      };
    }),
  };

  for (const step of plan.steps) {
    const fn = input.tools.find((tool) => tool.name === step.tool_name);
    if (!fn) {
      throw new PlanValidationError(`Unknown tool in plan: ${step.tool_name}`);
    }
    // Validate args against schema shape at plan time when possible.
    buildInputSchema(fn.params);
  }

  assertValidPlan(plan, input.config.planStepCap);
  return plan;
}

export function toolsToDefinitions(tools: FunctionDefinition[]): ToolDefinition[] {
  return tools.map((tool) => ({
    name: tool.name,
    description: tool.description,
    input_schema: buildInputSchema(tool.params),
  }));
}
