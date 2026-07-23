// Model routing — the ONE place vendor selection lives on the TS side.
//
// The Rust brain sends either a concrete model id ("gpt-4o-mini", "claude-sonnet-5")
// or an abstract tier alias ("tier:cheap", "tier:smart"). This module maps that to a
// concrete `@aelio/llm` provider config using environment configuration. It makes no
// product decisions — it only answers "which vendor + model + key runs this call".

import {
  createEmbeddingProvider,
  createLLMProvider,
  type EmbeddingProvider,
  type EmbeddingProviderConfig,
  type LLMProvider,
  type LLMProviderConfig,
} from '@aelio/llm';

export type ProviderName = LLMProviderConfig['provider'];

export type ResolvedRoute = {
  provider: ProviderName;
  /** The concrete vendor model id the alias resolved to. */
  providerModel: string;
  /** Lazily construct the provider so validation of the key happens per-call. */
  make: () => LLMProvider;
};

/** Vendor key for a provider, from the gateway's own environment (never a tenant key). */
function apiKeyFor(provider: ProviderName, env: NodeJS.ProcessEnv): string | undefined {
  switch (provider) {
    case 'openai':
      return env.OPENAI_API_KEY;
    case 'anthropic':
      return env.ANTHROPIC_API_KEY;
    case 'gemini':
      return env.GEMINI_API_KEY ?? env.GOOGLE_API_KEY;
    case 'groq':
      return env.GROQ_API_KEY;
    case 'ollama':
      return undefined; // local, no key
    case 'mock':
      return undefined;
  }
}

/** Infer a vendor from a concrete model id by its well-known prefix. */
function providerForModelId(modelId: string): ProviderName | undefined {
  const id = modelId.toLowerCase();
  if (id.startsWith('gpt') || id.startsWith('o1') || id.startsWith('o3') || id.startsWith('o4')) {
    return 'openai';
  }
  if (id.startsWith('claude')) return 'anthropic';
  if (id.startsWith('gemini')) return 'gemini';
  if (id.startsWith('llama') || id.startsWith('mixtral') || id.startsWith('qwen')) return 'groq';
  return undefined;
}

/**
 * Resolve a Rust-supplied model string to a concrete provider + model.
 *
 * Tier aliases are read from env so ops can retarget "cheap"/"smart" without a redeploy:
 *   LLM_TIER_CHEAP  (default "gpt-4o-mini")
 *   LLM_TIER_SMART  (default "claude-sonnet-5")
 * The default provider for an unrecognised bare model is LLM_GATEWAY_DEFAULT_PROVIDER
 * (default "openai").
 */
export function resolveRoute(model: string, env: NodeJS.ProcessEnv = process.env): ResolvedRoute {
  const tierCheap = env.LLM_TIER_CHEAP ?? 'gpt-4o-mini';
  const tierSmart = env.LLM_TIER_SMART ?? 'claude-sonnet-5';

  let modelId = model;
  if (model === 'tier:cheap') modelId = tierCheap;
  else if (model === 'tier:smart') modelId = tierSmart;
  else if (model === 'default') modelId = tierCheap;

  const provider =
    providerForModelId(modelId) ??
    ((env.LLM_GATEWAY_DEFAULT_PROVIDER as ProviderName | undefined) ?? 'openai');

  const config: LLMProviderConfig = {
    provider,
    model: modelId,
    apiKey: apiKeyFor(provider, env),
    baseUrl: provider === 'openai' ? env.OPENAI_BASE_URL : undefined,
  };

  return {
    provider,
    providerModel: modelId,
    make: () => createLLMProvider(config),
  };
}

export type ResolvedEmbeddingRoute = {
  provider: EmbeddingProviderConfig['provider'];
  providerModel: string;
  make: () => EmbeddingProvider;
};

function embeddingProviderForModelId(
  modelId: string,
): EmbeddingProviderConfig['provider'] | undefined {
  const id = modelId.toLowerCase();
  if (id.startsWith('text-embedding') || id.startsWith('openai')) return 'openai';
  if (id.startsWith('voyage')) return 'anthropic';
  if (id.startsWith('gemini') || id.startsWith('models/embedding')) return 'gemini';
  if (id.startsWith('nomic') || id.startsWith('mxbai')) return 'ollama';
  return undefined;
}

/**
 * Resolve a Rust-supplied embedding model string to a concrete provider + model. The `tier:embed`
 * alias is read from env so ops can retarget it without a redeploy:
 *   LLM_TIER_EMBED (default "text-embedding-3-small")
 */
export function resolveEmbeddingRoute(
  model: string,
  env: NodeJS.ProcessEnv = process.env,
): ResolvedEmbeddingRoute {
  const tierEmbed = env.LLM_TIER_EMBED ?? 'text-embedding-3-small';
  let modelId = model;
  if (model === 'tier:embed' || model === 'default') modelId = tierEmbed;

  const provider =
    embeddingProviderForModelId(modelId) ??
    ((env.LLM_GATEWAY_DEFAULT_EMBED_PROVIDER as EmbeddingProviderConfig['provider'] | undefined) ??
      'openai');

  const config: EmbeddingProviderConfig = {
    provider,
    model: modelId,
    apiKey: apiKeyFor(provider, env),
    baseUrl: provider === 'openai' ? env.OPENAI_EMBED_BASE_URL : undefined,
  };

  return {
    provider,
    providerModel: modelId,
    make: () => createEmbeddingProvider(config),
  };
}
