import assert from 'node:assert/strict';
import test from 'node:test';
import type {
  EmbeddingProvider,
  LLMCompleteOptions,
  LLMCompleteResult,
  LLMProvider,
} from '@aelio/llm';
import {
  GatewayError,
  IdempotencyCache,
  handleComplete,
  handleEmbed,
  type EmbedRequest,
  type GatewayRequest,
} from './gateway.js';
import type { ResolvedEmbeddingRoute, ResolvedRoute } from './routing.js';

/** A provider that records how it was called and returns a canned result. */
class SpyProvider implements LLMProvider {
  calls: LLMCompleteOptions[] = [];
  constructor(private readonly result: LLMCompleteResult) {}
  async complete(opts: LLMCompleteOptions): Promise<LLMCompleteResult> {
    this.calls.push(opts);
    return this.result;
  }
}

function routeWith(provider: LLMProvider): (model: string) => ResolvedRoute {
  return (model) => ({
    provider: 'openai',
    providerModel: model === 'tier:cheap' ? 'gpt-4o-mini' : model,
    make: () => provider,
  });
}

const closedSchemaRequest = (requestId = 'r1'): GatewayRequest => ({
  request_id: requestId,
  model: 'tier:cheap',
  temperature: 0,
  prompt: { text: 'give the ten most avaricious clients', spec_id: 'aelio.synthesize' },
  response_format: {
    type: 'json_schema',
    json_schema: {
      name: 'aelio_synthesize',
      strict: true,
      schema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] },
    },
  },
});

test('forces the closed-schema tool and returns its args as JSON content', async () => {
  const provider = new SpyProvider({
    text: '',
    toolCalls: [{ id: 't1', name: 'aelio_synthesize', args: { text: 'Here are your clients.' } }],
    stopReason: 'tool_use',
    usage: { inputTokens: 12, outputTokens: 5 },
  });

  const res = await handleComplete(closedSchemaRequest(), { resolveRoute: routeWith(provider) });

  assert.equal(res.content, JSON.stringify({ text: 'Here are your clients.' }));
  assert.deepEqual(res.usage, { input_tokens: 12, output_tokens: 5 });
  assert.equal(res.provider, 'openai');
  assert.equal(res.model, 'gpt-4o-mini');
  // The single tool it built is the structured-output tool, forced.
  assert.equal(provider.calls.length, 1);
  const [call] = provider.calls;
  assert.ok(call);
  assert.equal(call.tools.length, 1);
  assert.deepEqual(call.toolChoice, { type: 'tool', name: 'aelio_synthesize' });
});

test('idempotent request_id dedups within TTL (provider called once)', async () => {
  const provider = new SpyProvider({
    text: '',
    toolCalls: [{ id: 't1', name: 'aelio_synthesize', args: { text: 'ok' } }],
    stopReason: 'tool_use',
    usage: { inputTokens: 1, outputTokens: 1 },
  });
  const cache = new IdempotencyCache(60_000);
  const deps = { resolveRoute: routeWith(provider), cache };

  const first = await handleComplete(closedSchemaRequest('same'), deps);
  const second = await handleComplete(closedSchemaRequest('same'), deps);

  assert.equal(provider.calls.length, 1, 'second identical call must not hit the provider');
  assert.equal(second.deduped, true);
  assert.equal(first.content, second.content);
});

test('rejects a request missing prompt text', async () => {
  const provider = new SpyProvider({ text: 'x', toolCalls: [], stopReason: 'stop' });
  await assert.rejects(
    () =>
      handleComplete({ model: 'tier:cheap', prompt: { text: '' } } as GatewayRequest, {
        resolveRoute: routeWith(provider),
      }),
    (error: unknown) => error instanceof GatewayError && error.status === 400,
  );
});

test('refuses caller-supplied tools — the gateway is completion-only', async () => {
  const provider = new SpyProvider({ text: 'x', toolCalls: [], stopReason: 'stop' });
  const body = {
    model: 'tier:cheap',
    prompt: { text: 'hi' },
    tools: [{ name: 'cancelOrder' }],
  } as unknown as GatewayRequest;
  await assert.rejects(
    () => handleComplete(body, { resolveRoute: routeWith(provider) }),
    (error: unknown) => error instanceof GatewayError && error.status === 400,
  );
});

test('free-text completion (no response_format) passes through text with tool choice none', async () => {
  const provider = new SpyProvider({ text: 'just words', toolCalls: [], stopReason: 'stop' });
  const res = await handleComplete(
    { model: 'gpt-4o-mini', prompt: { text: 'say hi' } },
    { resolveRoute: routeWith(provider) },
  );
  assert.equal(res.content, 'just words');
  const [call] = provider.calls;
  assert.ok(call);
  assert.deepEqual(call.toolChoice, { type: 'none' });
  assert.equal(call.tools.length, 0);
});

class StubEmbedder implements EmbeddingProvider {
  seen: string[] = [];
  async embed(text: string): Promise<number[]> {
    this.seen.push(text);
    // deterministic tiny vector keyed on length so the test can assert order preserved
    return [text.length, 0, 0];
  }
}

function embedRouteWith(provider: EmbeddingProvider): (model: string) => ResolvedEmbeddingRoute {
  return () => ({ provider: 'openai', providerModel: 'text-embedding-3-small', make: () => provider });
}

test('embed returns one vector per input, order preserved, with provider/model', async () => {
  const embedder = new StubEmbedder();
  const res = await handleEmbed(
    { request_id: 'e1', model: 'tier:embed', input: ['greedy', 'avaricious'] },
    { resolveRoute: embedRouteWith(embedder) },
  );
  assert.equal(res.vectors.length, 2);
  assert.equal(res.vectors[0]?.[0], 'greedy'.length);
  assert.equal(res.vectors[1]?.[0], 'avaricious'.length);
  assert.equal(res.provider, 'openai');
  assert.equal(res.model, 'text-embedding-3-small');
  assert.deepEqual(embedder.seen, ['greedy', 'avaricious']);
});

test('embed rejects an empty input array', async () => {
  const embedder = new StubEmbedder();
  await assert.rejects(
    () => handleEmbed({ model: 'tier:embed', input: [] } as EmbedRequest, {
      resolveRoute: embedRouteWith(embedder),
    }),
    (error: unknown) => error instanceof GatewayError && error.status === 400,
  );
});
