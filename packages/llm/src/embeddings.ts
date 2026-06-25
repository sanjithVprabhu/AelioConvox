export interface EmbeddingProvider {
  embed(text: string): Promise<number[]>;
}

export type EmbeddingProviderConfig = {
  provider: 'openai' | 'ollama';
  model: string;
  apiKey?: string;
  baseUrl?: string;
};

class OpenAIEmbeddingProvider implements EmbeddingProvider {
  constructor(
    private readonly apiKey: string,
    private readonly model: string,
    private readonly baseUrl = 'https://api.openai.com/v1/embeddings',
  ) {}

  async embed(text: string): Promise<number[]> {
    const response = await fetch(this.baseUrl, {
      method: 'POST',
      headers: { 'content-type': 'application/json', authorization: `Bearer ${this.apiKey}` },
      body: JSON.stringify({ model: this.model, input: text }),
    });
    if (!response.ok) {
      throw new Error(`Embedding API error (${response.status}): ${(await response.text()).slice(0, 200)}`);
    }
    const data = (await response.json()) as { data?: Array<{ embedding?: number[] }> };
    const embedding = data.data?.[0]?.embedding;
    if (!Array.isArray(embedding)) {
      throw new Error('Embedding API returned no vector');
    }
    return embedding;
  }
}

class OllamaEmbeddingProvider implements EmbeddingProvider {
  constructor(
    private readonly model: string,
    private readonly baseUrl = 'http://127.0.0.1:11434/api/embeddings',
  ) {}

  async embed(text: string): Promise<number[]> {
    const response = await fetch(this.baseUrl, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ model: this.model, prompt: text }),
    });
    if (!response.ok) {
      throw new Error(`Ollama embedding error (${response.status}): ${(await response.text()).slice(0, 200)}`);
    }
    const data = (await response.json()) as { embedding?: number[] };
    if (!Array.isArray(data.embedding)) {
      throw new Error('Ollama returned no embedding');
    }
    return data.embedding;
  }
}

export function createEmbeddingProvider(config: EmbeddingProviderConfig): EmbeddingProvider {
  switch (config.provider) {
    case 'openai':
      if (!config.apiKey) {
        throw new Error('OpenAI embedding provider requires api_key');
      }
      return new OpenAIEmbeddingProvider(config.apiKey, config.model, config.baseUrl);
    case 'ollama':
      return new OllamaEmbeddingProvider(config.model, config.baseUrl);
    default:
      throw new Error(`Embedding provider "${(config as { provider: string }).provider}" is not implemented`);
  }
}
