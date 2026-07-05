import { AnthropicProvider } from './anthropic.js';
import { FallbackProvider } from './fallback.js';
import { GeminiProvider } from './gemini.js';
import { GroqProvider } from './groq.js';
import { MockProvider } from './mock.js';
import { OllamaProvider } from './ollama.js';
import { OpenAIProvider } from './openai.js';
import type { LLMProvider, LLMProviderConfig } from './types.js';

export * from './types.js';
export { FallbackProvider } from './fallback.js';
export {
  createEmbeddingProvider,
  type EmbeddingProvider,
  type EmbeddingProviderConfig,
} from './embeddings.js';

export function createLLMProvider(config: LLMProviderConfig): LLMProvider {
  switch (config.provider) {
    case 'mock':
      return new MockProvider();
    case 'anthropic':
      if (!config.apiKey) {
        throw new Error('Anthropic provider requires api_key');
      }
      return new AnthropicProvider(config.apiKey, config.maxTokens);
    case 'openai':
      if (!config.apiKey) {
        throw new Error('OpenAI provider requires api_key');
      }
      return new OpenAIProvider(config.apiKey, config.maxTokens, config.baseUrl);
    case 'gemini':
      if (!config.apiKey) {
        throw new Error('Gemini provider requires api_key');
      }
      return new GeminiProvider(config.apiKey, config.maxTokens, config.baseUrl);
    case 'groq':
      if (!config.apiKey) {
        throw new Error('Groq provider requires api_key');
      }
      return new GroqProvider(config.apiKey, config.maxTokens, config.baseUrl);
    case 'ollama':
      return new OllamaProvider(config.maxTokens, config.baseUrl);
    default:
      throw new Error(`LLM provider "${config.provider}" is not implemented yet`);
  }
}

export function createLLMProviderChain(configs: [LLMProviderConfig, ...LLMProviderConfig[]]): LLMProvider {
  if (configs.length === 1) {
    return createLLMProvider(configs[0]);
  }

  return new FallbackProvider(configs.map((config) => createLLMProvider(config)));
}
