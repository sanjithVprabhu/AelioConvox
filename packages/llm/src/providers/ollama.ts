import { OpenAICompatibleProvider } from './openai-compatible.js';

export class OllamaProvider extends OpenAICompatibleProvider {
  constructor(defaultMaxTokens = 4096, baseUrl = 'http://127.0.0.1:11434/v1/chat/completions') {
    super(baseUrl, {}, defaultMaxTokens);
  }
}
