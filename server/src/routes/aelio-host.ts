import { embed } from '@aelio/core/edge';
import type { FastifyInstance, FastifyRequest } from 'fastify';
import { z } from 'zod';
import { secretsMatch } from '../auth.js';
import { canonicalAgentToolId } from '../aelio-agent-catalog.js';
import type { RuntimeDeps } from '../runtime-deps.js';

const HostRequestSchema = z.object({
  protocol: z.literal('aelio-host/1'),
  corr: z.string().min(1),
  tenant: z.string().min(1),
  instance_id: z.string().min(1),
  turn_id: z.string().min(1),
  nid: z.string().min(1),
  deadline_ms: z.number().int().positive().nullable(),
  target: z.string().min(1),
  args: z.unknown(),
}).strict();

const ModelArgsSchema = z.object({
  prompt: z.string().min(1),
  prompt_hash: z.string().min(1),
  model: z.string().min(1).optional(),
  max_tokens: z.number().int().positive().max(32_768).optional(),
  temperature: z.number().min(0).max(2).optional(),
}).strict();

const EmbedArgsSchema = z.object({
  text: z.string().min(1),
  model: z.string().min(1).optional(),
}).strict();

const ToolArgsSchema = z.object({
  args: z.record(z.unknown()),
  context: z.object({
    channel: z.string().min(1).default('web'),
    channel_address: z.string().min(1).optional(),
    locale: z.string().optional(),
    user_id: z.string().min(1).optional(),
  }).optional(),
}).strict();

function authorized(request: FastifyRequest, secret: string): boolean {
  const header = request.headers.authorization;
  const candidate = header?.startsWith('Bearer ') ? header.slice('Bearer '.length) : undefined;
  return Boolean(candidate && secretsMatch(candidate, secret));
}

function targetName(target: string): string {
  const separator = target.lastIndexOf('@');
  return separator > 0 ? target.slice(0, separator) : target;
}

/** The only reverse boundary Rust exposes to TypeScript: intelligence and external effects. */
export async function registerAelioHostRoutes(app: FastifyInstance, deps: RuntimeDeps) {
  const hostToken = process.env.AELIO_HOST_TOKEN;
  if (!hostToken) {
    if (process.env.NODE_ENV === 'production') {
      throw new Error('AELIO_HOST_TOKEN is required for the Rust → TypeScript host boundary');
    }
    app.log.warn('AELIO_HOST_TOKEN is unset; the Rust host-adapter route is disabled');
    return;
  }

  app.post('/internal/aelio/target', async (request, reply) => {
    if (!authorized(request, hostToken)) {
      return reply.code(401).send({
        outcome: 'err',
        error: { code: 'policy_denied', detail: 'invalid host bearer token' },
      });
    }
    const parsed = HostRequestSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.code(400).send({
        outcome: 'err',
        error: { code: 'shape', detail: parsed.error.message },
      });
    }

    const call = parsed.data;
    const name = targetName(call.target);
    try {
      if (name === 'aelio.model.complete') {
        const args = ModelArgsSchema.parse(call.args);
        const result = await deps.llm.complete({
          messages: [{ role: 'user', content: args.prompt }],
          tools: [],
          model: args.model ?? deps.config.llm.model,
          maxTokens: args.max_tokens ?? deps.config.llm.max_tokens,
          temperature: args.temperature ?? 0,
          toolChoice: { type: 'none' },
        });
        return {
          outcome: 'ok',
          output: { text: result.text },
          usage_tokens: (result.usage?.inputTokens ?? 0) + (result.usage?.outputTokens ?? 0),
        };
      }

      if (name === 'aelio.embedding.embed') {
        const args = EmbedArgsSchema.parse(call.args);
        const vector = await embed(args.text, {
          purpose: 'pathway_retrieval',
          model: args.model,
          turnId: call.turn_id,
          sessionId: call.instance_id,
          customerId: call.instance_id,
        });
        return { outcome: 'ok', output: { vector }, usage_tokens: 0 };
      }

      const args = ToolArgsSchema.parse(call.args);
      const externalNames = deps.sdkBridge
        .getFunctions()
        .map((fn) => fn.name)
        .filter((functionName) => canonicalAgentToolId(functionName) === name);
      if (externalNames.length !== 1) {
        throw new Error(
          externalNames.length === 0
            ? `No SDK function maps to admitted tool "${name}"`
            : `Multiple SDK functions map to admitted tool "${name}"`,
        );
      }
      const toolContext = args.context;
      const customerId = toolContext?.user_id ?? call.instance_id;
      const sdkFunction = externalNames[0]!;
      const result = await deps.sdkBridge.invokeCorrelated(sdkFunction, args.args, {
        customerId,
        sessionId: customerId,
        channel: toolContext?.channel ?? 'web',
        channelAddress: toolContext?.channel_address ?? `aelio:${customerId}`,
        locale: toolContext?.locale,
        metadata: {
          tenant: call.tenant,
          turnId: call.turn_id,
          nodeId: call.nid,
          correlationId: call.corr,
        },
      }, call.corr);
      if (!result.ok) {
        return {
          outcome: 'err',
          error: { code: 'tool_transient', detail: result.error ?? 'SDK invocation failed' },
          usage_tokens: 0,
        };
      }
      if (process.env.AELIO_DECISION_LOG === '1') {
        request.log.info(
          {
            corr: call.corr,
            target: call.target,
            outputKeys:
              result.data && typeof result.data === 'object' && !Array.isArray(result.data)
                ? Object.keys(result.data as Record<string, unknown>).sort()
                : [],
          },
          'Aelio host result shape',
        );
      }
      return { outcome: 'ok', output: result.data ?? null, usage_tokens: 0 };
    } catch (error) {
      const detail = error instanceof Error ? error.message : 'host adapter failed';
      request.log.warn(
        { corr: call.corr, target: call.target, error: detail },
        'Aelio host target failed',
      );
      return {
        outcome: 'err',
        error: { code: error instanceof z.ZodError ? 'shape' : 'tool_transient', detail },
        usage_tokens: 0,
      };
    }
  });
}
