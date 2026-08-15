/**
 * Load ShopCo pipeline / memory / flows from YAML manifest.
 * Supports `tool_groups` + stage `allowed_groups` / `blocked_groups` (expanded by the SDK).
 */
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from 'yaml';
import { aelio, type ToolGroupDefinition } from '@aelio/sdk';

type AccessFields = {
  allowed_tools?: string[];
  blocked_tools?: string[];
  allowed_intents?: string[];
  blocked_intents?: string[];
  allowed_safety?: Array<'read' | 'write' | 'destructive'>;
  blocked_safety?: Array<'read' | 'write' | 'destructive'>;
  allowed_groups?: string[];
  blocked_groups?: string[];
};

type ShopCoManifest = {
  product: {
    name: string;
    description: string;
    persona: string;
  };
  tool_groups?: Record<
    string,
    {
      tools?: string[];
      intents?: string[];
      safety?: Array<'read' | 'write' | 'destructive'>;
    }
  >;
  pipeline: {
    initial_stage: string;
    stages: Record<
      string,
      AccessFields & {
        description: string;
        content?: { greeting?: string; cta?: string };
        flow?: string;
        guards?: { requires_fields?: string[] };
        next?: string;
      }
    >;
  };
  attributes: Record<
    string,
    {
      label: string;
      data_type?: string;
      sensitivity_tier?: string;
      prompts?: string[];
      enum_values?: Array<string | number>;
      ui_format?: { component: string; config?: Record<string, unknown> };
    }
  >;
  flows: Record<
    string,
    {
      state: string;
      description: string;
      steps: Record<
        string,
        {
          type?: 'tool' | 'attribute' | 'content';
          goal: string;
          tool?: string;
          attribute?: string;
          skip_if_present?: string;
        }
      >;
    }
  >;
  lifecycle_states?: Record<
    string,
    AccessFields & {
      description: string;
    }
  >;
  policies: Record<
    string,
    {
      description: string;
      severity?: 'hard' | 'soft';
    }
  >;
};

function mapAccess(fields: AccessFields) {
  return {
    ...(fields.allowed_tools ? { allowedTools: fields.allowed_tools } : {}),
    ...(fields.blocked_tools ? { blockedTools: fields.blocked_tools } : {}),
    ...(fields.allowed_intents ? { allowedIntents: fields.allowed_intents } : {}),
    ...(fields.blocked_intents ? { blockedIntents: fields.blocked_intents } : {}),
    ...(fields.allowed_safety ? { allowedSafety: fields.allowed_safety } : {}),
    ...(fields.blocked_safety ? { blockedSafety: fields.blocked_safety } : {}),
    ...(fields.allowed_groups ? { allowedGroups: fields.allowed_groups } : {}),
    ...(fields.blocked_groups ? { blockedGroups: fields.blocked_groups } : {}),
  };
}

function mapToolGroups(
  raw: ShopCoManifest['tool_groups'],
): Record<string, ToolGroupDefinition> | undefined {
  if (!raw) return undefined;
  return Object.fromEntries(
    Object.entries(raw).map(([id, group]) => [
      id,
      {
        ...(group.tools ? { tools: group.tools } : {}),
        ...(group.intents ? { intents: group.intents } : {}),
        ...(group.safety ? { safety: group.safety } : {}),
      },
    ]),
  );
}

export function loadShopCoManifest(manifestPath?: string): void {
  const __dirname = dirname(fileURLToPath(import.meta.url));
  const path = manifestPath ?? join(__dirname, '../manifests/shopco.pipeline.yaml');
  const raw = parse(readFileSync(path, 'utf8')) as ShopCoManifest;

  aelio.describe(raw.product.description.trim());
  aelio.persona(raw.product.persona.trim());

  const toolGroups = mapToolGroups(raw.tool_groups);
  if (toolGroups) {
    aelio.toolGroups(toolGroups);
  }

  aelio.pipeline({
    initialStage: raw.pipeline.initial_stage,
    ...(toolGroups ? { toolGroups } : {}),
    stages: Object.fromEntries(
      Object.entries(raw.pipeline.stages).map(([id, stage]) => [
        id,
        {
          description: stage.description.trim(),
          ...(stage.content ? { content: stage.content } : {}),
          ...(stage.flow ? { flow: stage.flow } : {}),
          ...mapAccess(stage),
          ...(stage.guards?.requires_fields
            ? { guards: { requiresFields: stage.guards.requires_fields } }
            : {}),
          ...(stage.next ? { next: stage.next } : {}),
        },
      ]),
    ),
  });

  for (const [id, attr] of Object.entries(raw.attributes)) {
    aelio.attribute(id, {
      label: attr.label,
      dataType: (attr.data_type ?? 'string') as 'string',
      sensitivityTier: (attr.sensitivity_tier ?? 'pii') as 'pii',
      ...(attr.prompts ? { prompts: attr.prompts } : {}),
      ...(attr.enum_values ? { enumValues: attr.enum_values } : {}),
      ...(attr.ui_format
        ? {
            uiFormat: {
              component: attr.ui_format.component,
              ...(attr.ui_format.config ? { config: attr.ui_format.config } : {}),
            },
          }
        : {}),
    });
  }

  for (const [flowId, flow] of Object.entries(raw.flows)) {
    aelio.flow(flowId, {
      state: flow.state,
      description: flow.description.trim(),
      steps: Object.fromEntries(
        Object.entries(flow.steps).map(([stepId, step]) => [
          stepId,
          {
            goal: step.goal,
            ...(step.type ? { type: step.type } : {}),
            ...(step.tool ? { tool: step.tool } : {}),
            ...(step.attribute ? { attribute: step.attribute } : {}),
            ...(step.skip_if_present ? { skipIfPresent: step.skip_if_present } : {}),
          },
        ]),
      ),
    });
  }

  for (const [id, state] of Object.entries(raw.lifecycle_states ?? {})) {
    aelio.state(id, {
      description: state.description.trim(),
      ...mapAccess(state),
    });
  }

  for (const [id, policy] of Object.entries(raw.policies)) {
    aelio.policy(id, {
      description: policy.description.trim(),
      severity: policy.severity ?? 'soft',
    });
  }
}
