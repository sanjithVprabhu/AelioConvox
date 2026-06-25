const DIMENSIONS = 1536;

function hashToken(token: string): number {
  let hash = 0;
  for (let i = 0; i < token.length; i += 1) {
    hash = (hash * 31 + token.charCodeAt(i)) >>> 0;
  }
  return hash;
}

export function embedText(text: string, dimensions = DIMENSIONS): number[] {
  const vector = new Array<number>(dimensions).fill(0);
  const tokens = text
    .toLowerCase()
    .replace(/[^a-z0-9\s]/g, ' ')
    .split(/\s+/)
    .filter(Boolean);

  if (tokens.length === 0) {
    return vector;
  }

  for (const token of tokens) {
    const hash = hashToken(token);
    const index = hash % dimensions;
    const sign = hash % 2 === 0 ? 1 : -1;
    vector[index] = (vector[index] ?? 0) + sign;
  }

  const magnitude = Math.sqrt(vector.reduce((sum, value) => sum + value * value, 0));
  if (magnitude === 0) {
    return vector;
  }

  return vector.map((value) => value / magnitude);
}

// A pluggable embedder. Defaults to the local hash embedding above; the server
// can swap in a real model (OpenAI/Ollama) via configureEmbedder(). Any failure
// in the real embedder falls back to the hash so the system never hard-breaks.
type AsyncEmbedder = (text: string) => Promise<number[]>;
let configuredEmbedder: AsyncEmbedder | null = null;

export function configureEmbedder(embedder: AsyncEmbedder | null): void {
  configuredEmbedder = embedder;
}

export function hasConfiguredEmbedder(): boolean {
  return configuredEmbedder != null;
}

export async function embed(text: string): Promise<number[]> {
  if (configuredEmbedder) {
    try {
      const vector = await configuredEmbedder(text);
      if (Array.isArray(vector) && vector.length > 0) {
        return vector;
      }
    } catch {
      // fall through to the local hash embedding
    }
  }
  return embedText(text);
}

export function cosineSimilarity(a: number[], b: number[]): number {
  const length = Math.min(a.length, b.length);
  let dot = 0;
  let magA = 0;
  let magB = 0;

  for (let i = 0; i < length; i += 1) {
    const av = a[i] ?? 0;
    const bv = b[i] ?? 0;
    dot += av * bv;
    magA += av * av;
    magB += bv * bv;
  }

  if (magA === 0 || magB === 0) {
    return 0;
  }

  return dot / (Math.sqrt(magA) * Math.sqrt(magB));
}
