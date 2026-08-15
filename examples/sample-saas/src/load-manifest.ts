/**
 * Load ShopCo pipeline / memory / flows from YAML manifest.
 */
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from 'yaml';
import { aelio } from '@aelio/sdk';

type ShopCoManifest = {
  product: {
    name: string;
    description: string;
    persona: string;
  };
  pipeline: {
    initial_stage: string;
    stages: Record<
      string,
      {
        description: string;
        content?: { greeting?: string; cta?: string };
        flow?: string;
        allowed_tools?: string[];
        blocked_tools?: string[];
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
  lifecycle_states: Record<
    string,
    {
      description: string;
      allowed_tools?: string[];
      blocked_tools?: string[];
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

export function loadShopCoManifest(manifestPath?: string): void {
  const __dirname = dirname(fileURLToPath(import.meta.url));
  const path = manifestPath ?? join(__dirname, '../manifests/shopco.pipeline.yaml');
  const raw = parse(readFileSync(path, 'utf8')) as ShopCoManifest;

  aelio.describe(raw.product.description.trim());
  aelio.persona(raw.product.persona.trim());

  aelio.pipeline({
    initialStage: raw.pipeline.initial_stage,
    stages: Object.fromEntries(
      Object.entries(raw.pipeline.stages).map(([id, stage]) => [
        id,
        {
          description: stage.description.trim(),
          ...(stage.content ? { content: stage.content } : {}),
          ...(stage.flow ? { flow: stage.flow } : {}),
          ...(stage.allowed_tools ? { allowedTools: stage.allowed_tools } : {}),
          ...(stage.blocked_tools ? { blockedTools: stage.blocked_tools } : {}),
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

  for (const [id, state] of Object.entries(raw.lifecycle_states)) {
    aelio.state(id, {
      description: state.description.trim(),
      ...(state.allowed_tools ? { allowedTools: state.allowed_tools } : {}),
      ...(state.blocked_tools ? { blockedTools: state.blocked_tools } : {}),
    });
  }

  for (const [id, policy] of Object.entries(raw.policies)) {
    aelio.policy(id, {
      description: policy.description.trim(),
      severity: policy.severity ?? 'soft',
    });
  }
}
