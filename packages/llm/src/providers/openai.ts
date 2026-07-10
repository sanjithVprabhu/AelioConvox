import { OpenAICompatibleProvider } from './openai-compatible.js';

export class OpenAIProvider extends OpenAICompatibleProvider {
  constructor(apiKey: string, defaultMaxTokens = 4096, baseUrl = 'https://api.openai.com/v1/chat/completions') {
    super(
      baseUrl,
      {
        authorization: `Bearer ${apiKey}`,
      },
      defaultMaxTokens,
    );
  }
}
