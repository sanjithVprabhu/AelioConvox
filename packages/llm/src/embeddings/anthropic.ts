import type { EmbeddingProvider } from './index.js';

/**
 * Anthropic does not ship a first-party embedding API. The Anthropic ecosystem
 * standard for semantic vectors is Voyage AI — this provider calls Voyage using
 * the configured api_key (VOYAGE_API_KEY or embeddings.api_key in config).
 */
export class AnthropicEmbeddingProvider implements EmbeddingProvider {
  constructor(
    private readonly apiKey: string,
    private readonly model: string,
    private readonly baseUrl = 'https://api.voyageai.com/v1/embeddings',
    private readonly outputDimension?: number,
  ) {}

  async embed(text: string): Promise<number[]> {
    const body: Record<string, unknown> = {
      model: this.model,
      input: text,
      input_type: 'document',
    };
    if (this.outputDimension) {
      body.output_dimension = this.outputDimension;
    }

    const response = await fetch(this.baseUrl, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        authorization: `Bearer ${this.apiKey}`,
      },
      body: JSON.stringify(body),
    });

    if (!response.ok) {
      throw new Error(
        `Voyage embedding API error (${response.status}): ${(await response.text()).slice(0, 200)}`,
      );
    }

    const data = (await response.json()) as { data?: Array<{ embedding?: number[] }> };
    const embedding = data.data?.[0]?.embedding;
    if (!Array.isArray(embedding)) {
      throw new Error('Voyage embedding API returned no vector');
    }
    return embedding;
  }
}