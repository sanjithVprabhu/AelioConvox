import type { LLMCompleteOptions, LLMCompleteResult, LLMProvider } from './types.js';

export class FallbackProvider implements LLMProvider {
  constructor(private readonly providers: LLMProvider[]) {}

  async complete(opts: LLMCompleteOptions): Promise<LLMCompleteResult> {
    let lastError: unknown;

    for (const provider of this.providers) {
      try {
        return await provider.complete(opts);
      } catch (error) {
        lastError = error;
      }
    }

    throw lastError instanceof Error
      ? lastError
      : new Error('All configured LLM providers failed');
  }
}
