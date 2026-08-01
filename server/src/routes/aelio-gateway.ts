import { embed } from '@aelio/core/edge';
import type { FastifyInstance, FastifyRequest } from 'fastify';
import { z } from 'zod';
import { secretsMatch } from '../auth.js';
import type { RuntimeDeps } from '../runtime-deps.js';

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

function authorized(request: FastifyRequest, secret: string): boolean {
  const header = request.headers.authorization;
  const candidate = header?.startsWith('Bearer ') ? header.slice('Bearer '.length) : undefined;
  return Boolean(candidate && secretsMatch(candidate, secret));
}

/** Stateless model/embedding modem for the Rust adaptive agent. */
export async function registerAelioGatewayRoutes(app: FastifyInstance, deps: RuntimeDeps) {
  const token = process.env.AELIO_HOST_TOKEN;
  if (!token) return;

  app.post('/internal/aelio/llm/complete', async (request, reply) => {
    if (!authorized(request, token)) return reply.code(401).send({ error: 'unauthorized' });
    const parsed = CompletionRequestSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.code(400).send({ error: 'shape', detail: parsed.error.message });
    }
    const call = parsed.data;
    const outputTool = `emit_${call.response_format.json_schema.name}`.slice(0, 64);
    const deterministicMock =
      deps.config.llm.provider === 'mock'
        ? mockStructuredResponse(
            call.prompt.spec_id,
            call.prompt.text,
            call.response_format.json_schema.schema,
          )
        : null;
    if (deterministicMock) {
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
    const declarations = (between(prompt, '<abilities>\n', '\n</abilities>') ?? '')
      .split('\n')
      .flatMap((line) => {
        try {
          const parsed: unknown = JSON.parse(line);
          return parsed && typeof parsed === 'object' ? [parsed as Record<string, unknown>] : [];
        } catch {
          return [];
        }
      });
    const wanted =
      request.includes('cancel')
        ? 'cancel_order'
        : request.includes('list') || request.includes('show all')
          ? 'list_orders'
          : request.includes('order') || request.includes('status') || request.includes('ship')
            ? 'get_order_status'
            : null;
    const selected =
      declarations.find((declaration) => declaration.ability_id === wanted) ??
      declarations.find((declaration) => declaration.ability_id === 'Express.Template');
    if (!selected || typeof selected.ability_id !== 'string') return null;
    const args: Record<string, unknown> = {};
    if (wanted === 'get_order_status' || wanted === 'cancel_order') args.orderId = 'last';
    return { steps: [{ ability_id: selected.ability_id, args }] };
  }
  if (specId === 'aelio.synthesize') {
    const rawEvidence = between(prompt, '<evidence>\n', '\n</evidence>') ?? '';
    const evidence = rawEvidence.toLowerCase();
    const response =
      evidence.includes('metric')
        ? { text: 'You prefer metric units.' }
        : evidence.includes('cancel')
        ? { text: 'Your order has been cancelled. Is there anything else I can help with?' }
        : evidence.includes('ship')
          ? { text: 'Your last order shipped today. Tracking: 1Z999AA10123456784' }
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

function between(text: string, start: string, end: string): string | null {
  const from = text.indexOf(start);
  if (from < 0) return null;
  const rest = text.slice(from + start.length);
  const to = rest.indexOf(end);
  return to < 0 ? null : rest.slice(0, to);
}
