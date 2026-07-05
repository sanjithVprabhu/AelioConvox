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

// Memoize text → vector. One turn embeds the SAME text several times (cache
// lookup, memory recall, message storage, conversation archive), which with a
// remote provider is several identical paid API calls. Embeddings are
// deterministic per provider, so an LRU collapses them into one real call.
// Only successful remote results (or pure-hash mode) are cached, so a transient
// remote failure never pins a fallback hash vector for that text.
const EMBED_CACHE_MAX = 512;
const embedCache = new Map<string, number[]>();

function embedCacheGet(text: string): number[] | undefined {
  const hit = embedCache.get(text);
  if (hit) {
    // refresh recency
    embedCache.delete(text);
    embedCache.set(text, hit);
  }
  return hit;
}

function embedCacheSet(text: string, vector: number[]): void {
  if (embedCache.size >= EMBED_CACHE_MAX) {
    const oldest = embedCache.keys().next().value;
    if (oldest !== undefined) {
      embedCache.delete(oldest);
    }
  }
  embedCache.set(text, vector);
}

export function configureEmbedder(embedder: AsyncEmbedder | null): void {
  configuredEmbedder = embedder;
  // Vectors from a different provider live in a different space — never mix.
  embedCache.clear();
}

export function hasConfiguredEmbedder(): boolean {
  return configuredEmbedder != null;
}

export type EmbedPurpose =
  | 'memory_recall'
  | 'memory_extract'
  | 'response_cache_lookup'
  | 'response_cache_store'
  | 'message_storage'
  | 'conversation_archive'
  | 'reflection_insight';

export type EmbedOptions = {
  purpose: EmbedPurpose;
  model?: string;
  turnId?: string;
  sessionId?: string;
  customerId?: string;
  database?: import('@aelio/db').AelioDatabase;
};

export async function embed(text: string, options?: EmbedOptions): Promise<number[]> {
  const started = Date.now();
  let vector: number[] | null = embedCacheGet(text) ?? null;
  const cacheHit = vector != null;
  let usedRemote = false;

  if (!vector && configuredEmbedder) {
    try {
      const remote = await configuredEmbedder(text);
      if (Array.isArray(remote) && remote.length > 0) {
        vector = remote;
        usedRemote = true;
      }
    } catch {
      // fall through to the local hash embedding
    }
  }

  if (!vector) {
    vector = embedText(text);
  }

  if (!cacheHit && (usedRemote || !configuredEmbedder)) {
    embedCacheSet(text, vector);
  }

  if (options?.purpose) {
    const provider = cacheHit ? 'cache' : usedRemote ? 'remote' : 'hash';
    const { recordTurnApiCall } = await import('../telemetry/turn-calls.js');
    await recordTurnApiCall({
      callType: 'embed',
      purpose: options.purpose,
      model: options.model ?? (cacheHit ? 'cache' : usedRemote ? 'configured' : 'hash'),
      promptSummary: `purpose=${options.purpose} | provider=${provider}`,
      inputPreview: text,
      durationMs: Date.now() - started,
      turnId: options.turnId,
      sessionId: options.sessionId,
      customerId: options.customerId,
      database: options.database,
    });
  }

  return vector;
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
