// The gateway core: turn a closed-schema completion request from the Rust brain into a
// single vendor model call and shape the reply back. This is a stateless "modem".
//
// Guardrails (see README):
//   • Completion only. The gateway never exposes SaaS/tenant tool-calling — the only tool
//     it constructs is the internal structured-output tool derived from `response_format`.
//   • Closed output only. It forces the model to that schema and returns the structured args
//     as a JSON string; Rust re-validates against the same schema and has the final say.
//   • Idempotent. Identical `request_id`s dedup within a short TTL.
//   • No secrets in logs. Callers log request_id / spec / provider, never prompt text.

import type { EmbeddingProvider, LLMProvider, LLMCompleteResult, ToolDefinition } from '@aelio/llm';
import type { ResolvedEmbeddingRoute, ResolvedRoute } from './routing.js';

export type GatewayRequest = {
  request_id?: string;
  model: string;
  temperature?: number;
  prompt: {
    text: string;
    spec_id?: string;
    spec_version?: string;
    prompt_hash?: string;
  };
  response_format?: {
    type?: string;
    json_schema?: {
      name: string;
      strict?: boolean;
      schema: Record<string, unknown>;
    };
  };
};

export type GatewayResponse = {
  content: string;
  usage: { input_tokens: number; output_tokens: number };
  provider: string;
  model: string;
  request_id: string;
  deduped?: boolean;
};

export class GatewayError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = 'GatewayError';
  }
}

export type GatewayDeps = {
  resolveRoute: (model: string) => ResolvedRoute;
  cache?: IdempotencyCache;
  now?: () => number;
};

/** Tiny in-memory idempotency cache. A real deployment would back this with a shared store. */
export class IdempotencyCache {
  private readonly entries = new Map<string, { at: number; value: GatewayResponse }>();

  constructor(private readonly ttlMs = 60_000) {}

  get(key: string, now: number): GatewayResponse | undefined {
    const hit = this.entries.get(key);
    if (!hit) return undefined;
    if (now - hit.at > this.ttlMs) {
      this.entries.delete(key);
      return undefined;
    }
    return hit.value;
  }

  set(key: string, value: GatewayResponse, now: number): void {
    this.entries.set(key, { at: now, value });
  }
}

function validate(req: GatewayRequest): void {
  if (!req || typeof req !== 'object') throw new GatewayError(400, 'request body must be an object');
  if (typeof req.model !== 'string' || req.model.trim() === '') {
    throw new GatewayError(400, '`model` is required');
  }
  if (!req.prompt || typeof req.prompt.text !== 'string' || req.prompt.text.trim() === '') {
    throw new GatewayError(400, '`prompt.text` is required');
  }
  // The gateway is completion-only. If a caller ever tries to smuggle executable tools through,
  // refuse — tool/flow decisioning stays in Rust.
  if ('tools' in req || 'tool_choice' in (req as Record<string, unknown>)) {
    throw new GatewayError(400, 'the gateway is completion-only; tool calling is not accepted');
  }
}

/** Build the single structured-output tool from the closed schema, or none for free text. */
function structuredTool(req: GatewayRequest): ToolDefinition | undefined {
  const schema = req.response_format?.json_schema;
  if (!schema) return undefined;
  if (!schema.name || !schema.schema) {
    throw new GatewayError(400, 'response_format.json_schema requires `name` and `schema`');
  }
  return {
    name: schema.name,
    description: 'Return the result as a single object matching this schema exactly.',
    input_schema: schema.schema,
  };
}

/** Extract the model's answer: forced-tool args serialized to JSON, else raw text. */
function extractContent(result: LLMCompleteResult, tool: ToolDefinition | undefined): string {
  if (tool) {
    const call = result.toolCalls.find((c) => c.name === tool.name) ?? result.toolCalls[0];
    if (call) return JSON.stringify(call.args);
  }
  return result.text;
}

export async function handleComplete(
  req: GatewayRequest,
  deps: GatewayDeps,
): Promise<GatewayResponse> {
  validate(req);
  const now = deps.now ?? Date.now;
  const requestId = req.request_id ?? '';

  if (requestId && deps.cache) {
    const cached = deps.cache.get(requestId, now());
    if (cached) return { ...cached, deduped: true };
  }

  const route = deps.resolveRoute(req.model);
  const tool = structuredTool(req);

  let provider: LLMProvider;
  try {
    provider = route.make();
  } catch (error) {
    // Missing vendor key etc. — misconfiguration, not the caller's fault.
    throw new GatewayError(502, `provider unavailable: ${(error as Error).message}`);
  }

  let result: LLMCompleteResult;
  try {
    result = await provider.complete({
      messages: [{ role: 'user', content: req.prompt.text }],
      tools: tool ? [tool] : [],
      model: route.providerModel,
      temperature: req.temperature ?? 0,
      // Force the structured-output tool so the reply obeys the closed schema.
      toolChoice: tool ? { type: 'tool', name: tool.name } : { type: 'none' },
    });
  } catch (error) {
    throw new GatewayError(502, `provider call failed: ${(error as Error).message}`);
  }

  const response: GatewayResponse = {
    content: extractContent(result, tool),
    usage: {
      input_tokens: result.usage?.inputTokens ?? 0,
      output_tokens: result.usage?.outputTokens ?? 0,
    },
    provider: route.provider,
    model: route.providerModel,
    request_id: requestId,
  };

  if (requestId && deps.cache) deps.cache.set(requestId, response, now());
  return response;
}

export type EmbedRequest = {
  request_id?: string;
  model: string;
  input: string[];
};

export type EmbedResponse = {
  vectors: number[][];
  model: string;
  provider: string;
  request_id: string;
};

export type EmbedDeps = {
  resolveRoute: (model: string) => ResolvedEmbeddingRoute;
};

function validateEmbed(req: EmbedRequest): void {
  if (!req || typeof req !== 'object') throw new GatewayError(400, 'request body must be an object');
  if (typeof req.model !== 'string' || req.model.trim() === '') {
    throw new GatewayError(400, '`model` is required');
  }
  if (!Array.isArray(req.input) || req.input.length === 0) {
    throw new GatewayError(400, '`input` must be a non-empty string array');
  }
  if (!req.input.every((text) => typeof text === 'string')) {
    throw new GatewayError(400, '`input` must contain only strings');
  }
  if (req.input.length > 256) {
    throw new GatewayError(413, '`input` exceeds the 256-item batch limit');
  }
}

export async function handleEmbed(req: EmbedRequest, deps: EmbedDeps): Promise<EmbedResponse> {
  validateEmbed(req);
  const route = deps.resolveRoute(req.model);

  let provider: EmbeddingProvider;
  try {
    provider = route.make();
  } catch (error) {
    throw new GatewayError(502, `embedding provider unavailable: ${(error as Error).message}`);
  }

  let vectors: number[][];
  try {
    vectors = await Promise.all(req.input.map((text) => provider.embed(text)));
  } catch (error) {
    throw new GatewayError(502, `embedding call failed: ${(error as Error).message}`);
  }

  return {
    vectors,
    model: route.providerModel,
    provider: route.provider,
    request_id: req.request_id ?? '',
  };
}
