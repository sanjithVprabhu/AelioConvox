import { OpenAICompatibleProvider } from './openai-compatible.js';

export class GroqProvider extends OpenAICompatibleProvider {
  constructor(apiKey: string, defaultMaxTokens = 4096, baseUrl = 'https://api.groq.com/openai/v1/chat/completions') {
    super(
      baseUrl,
      {
        authorization: `Bearer ${apiKey}`,
      },
      defaultMaxTokens,
    );
  }
}
