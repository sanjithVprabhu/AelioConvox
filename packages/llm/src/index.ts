// @aelio/llm — the single place all LLM + embedding provider logic lives.
//
//   src/
//   ├── types.ts          shared LLMProvider / ChatMessage / tool-choice types
//   ├── providers/        one file per chat provider (anthropic, openai, gemini,
//   │                     groq, ollama, mock, scripted) + fallback chain
//   └── embeddings/       embedding providers (openai, anthropic/voyage, gemini,
//                         ollama) behind one factory
//
// Consumers only ever import from this barrel; the factories below turn a plain
// config object (provider name + key + model) into a working provider, so any
// of the three first-class chat providers — Anthropic (Claude), OpenAI (GPT),
// Gemini — is selected purely by config. See config.{anthropic,openai,gemini}.yaml.
import {
  AnthropicProvider,
  FallbackProvider,
  GeminiProvider,
  GroqProvider,
  MockProvider,
  OllamaProvider,
  OpenAIProvider,
} from './providers/index.js';
import type { LLMProvider, LLMProviderConfig } from './types.js';

export * from './types.js';
export { FallbackProvider, ScriptedProvider, MockProvider } from './providers/index.js';
export {
  createEmbeddingProvider,
  type EmbeddingProvider,
  type EmbeddingProviderConfig,
} from './embeddings/index.js';

/** The chat providers a config may select. */
export const SUPPORTED_LLM_PROVIDERS = [
  'anthropic',
  'openai',
  'gemini',
  'groq',
  'ollama',
  'mock',
] as const;

/** Build a single chat provider from config. Throws with a clear message when a
 *  key is required but absent, so misconfiguration fails loudly at boot. */
export function createLLMProvider(config: LLMProviderConfig): LLMProvider {
  switch (config.provider) {
    case 'mock':
      return new MockProvider();
    case 'anthropic':
      if (!config.apiKey) {
        throw new Error('Anthropic (Claude) provider requires api_key — set ANTHROPIC_API_KEY');
      }
      return new AnthropicProvider(config.apiKey, config.maxTokens);
    case 'openai':
      if (!config.apiKey) {
        throw new Error('OpenAI (GPT) provider requires api_key — set OPENAI_API_KEY');
      }
      return new OpenAIProvider(config.apiKey, config.maxTokens, config.baseUrl);
    case 'gemini':
      if (!config.apiKey) {
        throw new Error('Gemini provider requires api_key — set GEMINI_API_KEY');
      }
      return new GeminiProvider(config.apiKey, config.maxTokens, config.baseUrl);
    case 'groq':
      if (!config.apiKey) {
        throw new Error('Groq provider requires api_key — set GROQ_API_KEY');
      }
      return new GroqProvider(config.apiKey, config.maxTokens, config.baseUrl);
    case 'ollama':
      return new OllamaProvider(config.maxTokens, config.baseUrl);
    default:
      throw new Error(`LLM provider "${config.provider}" is not implemented yet`);
  }
}

/** Build a primary provider with optional fallbacks (first success wins). */
export function createLLMProviderChain(configs: [LLMProviderConfig, ...LLMProviderConfig[]]): LLMProvider {
  if (configs.length === 1) {
    return createLLMProvider(configs[0]);
  }

  return new FallbackProvider(configs.map((config) => createLLMProvider(config)));
}
