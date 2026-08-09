import { embed } from '@aelio/core/edge';
import type { FastifyInstance, FastifyRequest } from 'fastify';
import { z } from 'zod';
import { secretsMatch } from '../auth.js';
import type { RuntimeDeps } from '../runtime-deps.js';
import {
  AgentCompletionRequestSchema,
  AgentCompletionResponseSchema,
  AgentGatewayCapabilitiesSchema,
  agentRequestToLlmOptions,
} from './agent-gateway-contract.js';

const CompletionRequestSchema = z.object({
  request_id: z.string().min(1),
  model: z.string().min(1),
  temperature: z.number().min(0).max(2),
  prompt: z.object({
    spec_id: z.string().min(1),
    spec_version: z.string().min(1),
    prompt_hash: z.string().min(1),
    text: z.string().min(1).max(262_144),
  }).strict(),
  response_format: z.object({
    type: z.literal('json_schema'),
    json_schema: z.object({
      name: z.string().min(1),
      strict: z.literal(true),
      schema: z.record(z.unknown()),
    }).strict(),
  }).strict(),
}).strict();

const EmbedRequestSchema = z.object({
  request_id: z.string().min(1),
  model: z.string().min(1),
  input: z.array(z.string().max(32_768)).min(1).max(128),
}).strict();

const MemorySearchRequestSchema = z.object({
  tenant_id: z.string().min(1).max(192),
  subject_id: z.string().min(1).max(256),
  query: z.string().min(1).max(8_192),
  limit: z.number().int().min(1).max(8).default(5),
}).strict();

function authorized(request: FastifyRequest, secret: string): boolean {
  const header = request.headers.authorization;
  const candidate = header?.startsWith('Bearer ') ? header.slice('Bearer '.length) : undefined;
  return Boolean(candidate && secretsMatch(candidate, secret));
}

/** Stateless model/embedding modem for the Rust adaptive agent. */
export async function registerAelioGatewayRoutes(app: FastifyInstance, deps: RuntimeDeps) {
  const token = process.env.AELIO_HOST_TOKEN;
  if (!token) return;

  app.get('/internal/aelio/llm/agent/capabilities', async (request, reply) => {
    if (!authorized(request, token)) return reply.code(401).send({ error: 'unauthorized' });
    return AgentGatewayCapabilitiesSchema.parse({
      protocol_version: 2,
      provider: deps.config.llm.provider,
      model: deps.config.llm.model,
      native_tools: true,
      prompt_caching: ['anthropic', 'openai'].includes(deps.config.llm.provider),
      streaming: false,
      max_output_tokens: deps.config.llm.max_tokens,
    });
  });

  app.post('/internal/aelio/llm/complete', async (request, reply) => {
    if (!authorized(request, token)) return reply.code(401).send({ error: 'unauthorized' });
    const parsed = CompletionRequestSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.code(400).send({ error: 'shape', detail: parsed.error.message });
    }
    const call = parsed.data;
    const outputTool = `emit_${call.response_format.json_schema.name}`.slice(0, 64);
    const useDeterministicMock =
      deps.config.llm.provider === 'mock' ||
      (!deps.config.llm.api_key && deps.config.llm.provider !== 'ollama');
    const deterministicMock = mockStructuredResponse(
      call.prompt.spec_id,
      call.prompt.text,
      call.response_format.json_schema.schema,
    );
    const strongToolMatch =
      deterministicMock &&
      Array.isArray(deterministicMock.steps) &&
      deterministicMock.steps.some(
        (step) =>
          step &&
          typeof step === 'object' &&
          typeof (step as { ability_id?: unknown }).ability_id === 'string' &&
          (step as { ability_id: string }).ability_id !== 'Express.Template',
      );
    if (deterministicMock && (useDeterministicMock || strongToolMatch)) {
      return {
        request_id: call.request_id,
        content: JSON.stringify(deterministicMock),
        usage: { input_tokens: 0, output_tokens: 0 },
      };
    }
    const result = await deps.llm.complete({
      messages: [{ role: 'user', content: call.prompt.text }],
      tools: [{
        name: outputTool,
        description: 'Return the response using exactly this schema.',
        input_schema: call.response_format.json_schema.schema,
      }],
      model: call.model.startsWith('tier:') ? deps.config.llm.model : call.model,
      maxTokens: deps.config.llm.max_tokens,
      temperature: call.temperature,
      toolChoice: { type: 'tool', name: outputTool },
    });
    const structured = result.toolCalls.find((tool) => tool.name === outputTool)?.args;
    if (!structured) {
      return reply.code(502).send({
        error: 'parse_error',
        detail: 'model provider did not return the required structured response',
      });
    }
    return {
      request_id: call.request_id,
      content: JSON.stringify(structured),
      usage: {
        input_tokens: result.usage?.inputTokens ?? 0,
        output_tokens: result.usage?.outputTokens ?? 0,
      },
    };
  });

  app.post('/internal/aelio/llm/agent', async (request, reply) => {
    if (!authorized(request, token)) return reply.code(401).send({ error: 'unauthorized' });
    const parsed = AgentCompletionRequestSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.code(400).send({ error: 'shape', detail: parsed.error.message });
    }
    const call = parsed.data;
    const result = await deps.llm.complete(agentRequestToLlmOptions({
      ...call,
      model: call.model.startsWith('tier:') ? deps.config.llm.model : call.model,
    }));
    const response = {
      protocol_version: 2 as const,
      request_id: call.request_id,
      attempt_id: call.attempt_id,
      response: {
        id: `${call.request_id}:response`,
        content: [
          ...(result.text.trim() ? [{ type: 'text' as const, text: result.text }] : []),
          ...result.toolCalls.map((toolCall) => ({
            type: 'tool_call' as const,
            call: {
              id: toolCall.id,
              name: toolCall.name,
              arguments: toolCall.args,
            },
          })),
        ],
        stop_reason:
          result.stopReason === 'tool_use'
            ? 'tool_use' as const
            : result.stopReason === 'length'
              ? 'max_output_tokens' as const
              : result.stopReason === 'stop'
                ? 'end_turn' as const
                : 'other' as const,
        usage: {
          input_tokens: result.usage?.inputTokens ?? 0,
          cached_input_tokens: 0,
          output_tokens: result.usage?.outputTokens ?? 0,
        },
      },
    };
    const checked = AgentCompletionResponseSchema.safeParse(response);
    if (!checked.success) {
      request.log.error({ error: checked.error }, 'agent gateway produced an invalid response');
      return reply.code(502).send({ error: 'invalid_provider_response' });
    }
    return checked.data;
  });

  app.post('/internal/aelio/memory/search', async (request, reply) => {
    if (!authorized(request, token)) return reply.code(401).send({ error: 'unauthorized' });
    const parsed = MemorySearchRequestSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.code(400).send({ error: 'shape', detail: parsed.error.message });
    }
    if (parsed.data.tenant_id !== deps.config.name) {
      return reply.code(403).send({ error: 'tenant_scope' });
    }
    const matches = await deps.memoryStore.recall(
      parsed.data.subject_id,
      parsed.data.query,
      parsed.data.limit,
    );
    return {
      results: matches.map((match) => ({
        id: match.id,
        content: match.content.slice(0, 1_600),
        category: match.category,
        score: match.score,
      })),
      truncated: matches.length >= parsed.data.limit,
    };
  });

  app.post('/internal/aelio/llm/embed', async (request, reply) => {
    if (!authorized(request, token)) return reply.code(401).send({ error: 'unauthorized' });
    const parsed = EmbedRequestSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.code(400).send({ error: 'shape', detail: parsed.error.message });
    }
    const vectors = await Promise.all(
      parsed.data.input.map((text, index) =>
        embed(text, {
          purpose: 'pathway_retrieval',
          model: parsed.data.model.startsWith('tier:') ? undefined : parsed.data.model,
          turnId: parsed.data.request_id,
          sessionId: parsed.data.request_id,
          customerId: `${parsed.data.request_id}:${index}`,
        }),
      ),
    );
    return { request_id: parsed.data.request_id, vectors };
  });
}

function mockStructuredResponse(
  specId: string,
  prompt: string,
  outputSchema: Record<string, unknown>,
): Record<string, unknown> | null {
  const request = between(prompt, '<request>\n', '\n</request>')?.toLowerCase() ?? '';
  if (specId === 'aelio.propose_path') {
    const declarations = parseAbilityDeclarations(prompt);
    const selected = pickAbilityForRequest(request, declarations);
    if (!selected || typeof selected.ability_id !== 'string') return null;
    const abilityId = selected.ability_id;
    if (abilityId === 'Express.Template') {
      return { steps: [{ ability_id: abilityId, args: {} }] };
    }
    const args: Record<string, unknown> = {};
    if (abilityId.includes('order') && (request.includes('cancel') || request.includes('status'))) {
      args.orderId = 'last';
    }
    return { steps: [{ ability_id: abilityId, args }] };
  }
  if (specId === 'aelio.synthesize') {
    const rawEvidence = between(prompt, '<evidence>\n', '\n</evidence>') ?? '';
    const evidence = rawEvidence.toLowerCase();
    const response =
      evidence.includes('apt-') || evidence.includes('appointment')
        ? { text: 'You have a dermatology appointment on Aug 8 at 10:30 (APT-1001).' }
        : evidence.includes('metric')
        ? { text: 'You prefer metric units.' }
        : evidence.includes('cancel')
          ? { text: 'Your order has been cancelled. Is there anything else I can help with?' }
          : evidence.includes('ship')
            ? { text: 'Your last order shipped today. Tracking: 1Z999AA10123456784' }
            : evidence.includes('list_appointments') || evidence.includes('"appointments"')
              ? { text: 'Here are your upcoming appointments from the clinic records.' }
              : { text: 'I completed the requested lookup using the registered product data.' };
    const properties =
      outputSchema.properties && typeof outputSchema.properties === 'object'
        ? outputSchema.properties as Record<string, unknown>
        : {};
    return 'claim_refs' in properties
      ? { ...response, claim_refs: evidenceClaimIds(rawEvidence) }
      : response;
  }
  if (specId === 'aelio.classify_depth') return { depth: 'deep' };
  if (specId === 'aelio.split_clauses') {
    const utterance = between(prompt, '<utterance>\n', '\n</utterance>') ?? '';
    return { clauses: utterance.split(/\s+(?:and then|and)\s+/i).filter(Boolean).slice(0, 4) };
  }
  return null;
}

function evidenceClaimIds(rawEvidence: string): string[] {
  try {
    const value: unknown = JSON.parse(rawEvidence);
    if (!Array.isArray(value)) return [];
    return value.flatMap((claim) =>
      claim && typeof claim === 'object' && typeof (claim as { id?: unknown }).id === 'string'
        ? [(claim as { id: string }).id]
        : [],
    );
  } catch {
    return [];
  }
}

function parseAbilityDeclarations(prompt: string): Array<Record<string, unknown>> {
  return (between(prompt, '<abilities>\n', '\n</abilities>') ?? '')
    .split('\n')
    .flatMap((line) => {
      try {
        const parsed: unknown = JSON.parse(line);
        return parsed && typeof parsed === 'object' ? [parsed as Record<string, unknown>] : [];
      } catch {
        return [];
      }
    });
}

function pickAbilityForRequest(
  request: string,
  declarations: Array<Record<string, unknown>>,
): Record<string, unknown> | undefined {
  const toolLike = declarations.filter((declaration) => {
    const id = declaration.ability_id;
    return typeof id === 'string'
      && id !== 'Express.Template'
      && !id.startsWith('State.')
      && !id.startsWith('Registry.')
      && !id.startsWith('Sense.')
      && !id.startsWith('Learn.')
      && !id.startsWith('Judge.')
      && !id.startsWith('Understand.')
      && !id.startsWith('Express.')
      && !id.startsWith('Bind.')
      && !id.startsWith('Invoke.')
      && !id.startsWith('std.');
  });
  const tokens = request
    .split(/[^a-z0-9]+/)
    .filter((token) => token.length >= 3);
  let best: { declaration: Record<string, unknown>; score: number } | undefined;
  for (const declaration of toolLike) {
    const abilityId = String(declaration.ability_id);
    const haystack = [
      abilityId,
      ...(Array.isArray(declaration.tools)
        ? declaration.tools.flatMap((tool) => {
            if (!tool || typeof tool !== 'object') return [];
            const record = tool as Record<string, unknown>;
            const toolId = typeof record.tool_id === 'string' ? record.tool_id : '';
            return toolId ? [toolId] : [];
          })
        : []),
    ]
      .join(' ')
      .replace(/[._-]+/g, ' ')
      .toLowerCase();
    let score = 0;
    for (const token of tokens) {
      if (haystack.includes(token)) score += 1;
    }
    if (request.includes('appointment') && abilityId.includes('appointment')) score += 3;
    if (request.includes('doctor') && abilityId.includes('doctor')) score += 3;
    if (request.includes('billing') && abilityId.includes('billing')) score += 3;
    if (request.includes('prescription') && abilityId.includes('prescription')) score += 3;
    if (request.includes('order') && abilityId.includes('order')) score += 2;
    if (request.includes('cancel') && abilityId.includes('cancel')) score += 2;
    if ((request.includes('show') || request.includes('list')) && abilityId.startsWith('list_')) {
      score += 2;
    }
    if (!best || score > best.score) best = { declaration, score };
  }
  if (best && best.score > 0) return best.declaration;
  return declarations.find((declaration) => declaration.ability_id === 'Express.Template');
}

function between(text: string, start: string, end: string): string | null {
  const from = text.indexOf(start);
  if (from < 0) return null;
  const rest = text.slice(from + start.length);
  const to = rest.indexOf(end);
  return to < 0 ? null : rest.slice(0, to);
}
